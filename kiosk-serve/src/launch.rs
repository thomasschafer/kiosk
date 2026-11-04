//! Starting the picker for a frontend that can only run a program to completion and then
//! connect to it.
//!
//! The frontend runs the product's serve command. That process starts a copy of itself in
//! the background, which listens on a loopback port, waits for its port and a secret to
//! come back on a pipe, prints them as one JSON line and exits, so the frontend's run
//! finishes at once. The frontend then connects and presents the secret within
//! [`ACCEPT_TIMEOUT`]. The background copy takes exactly one connection: once a client has
//! presented the secret it stops listening, and it exits when that client goes.

use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::protocol::{ClientMessage, PROTOCOL_VERSION};

/// Set on the background copy, so it listens rather than starting another.
const STAGE_VAR: &str = "KIOSK_SERVE_STAGE";
/// Tells the background copy where its standard error goes, so it can remove the file
/// once it has ended cleanly.
const LOG_VAR: &str = "KIOSK_SERVE_LOG";
const BACKGROUND_STAGE: &str = "background";

/// How long the background copy waits for a client to connect and present the secret.
pub const ACCEPT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a connected client has to present the secret.
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the foreground process waits for the background copy to start listening.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// What the foreground process prints: where to connect and what to present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    pub port: u16,
    pub token: String,
    pub protocol: u32,
}

/// Where this run is.
pub enum Stage {
    /// The process the frontend ran: [`start_background`] and report its handshake.
    Foreground,
    /// The copy in the background: [`listen`] for the frontend.
    Background,
}

pub fn stage() -> Stage {
    match std::env::var_os(STAGE_VAR) {
        Some(value) if value == BACKGROUND_STAGE => Stage::Background,
        _ => Stage::Foreground,
    }
}

/// Starts this program again in the background with the same arguments and returns its
/// handshake once it listens. Its standard error goes to a new log file named after
/// `product_name`, which the error quotes if it fails to start.
pub fn start_background(product_name: &str) -> Result<Handshake> {
    let program = std::env::current_exe().context("finding this program to start it again")?;
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (log, stderr) = create_log(product_name)?;
    let log = log.as_path();
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env(STAGE_VAR, BACKGROUND_STAGE)
        .env(LOG_VAR, log)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr);
    detach(&mut command);
    let mut child = command
        .spawn()
        .context("starting the picker in the background")?;
    let stdout = child
        .stdout
        .take()
        .context("the background picker has no output pipe")?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = sender.send(read);
    });
    let mut failure = |reason: String| {
        let _ = child.kill();
        let log_text = std::fs::read_to_string(log).unwrap_or_default();
        let log_text = log_text.trim();
        if log_text.is_empty() {
            anyhow::anyhow!("{reason}")
        } else {
            anyhow::anyhow!("{reason}: {log_text}")
        }
    };
    let line = match receiver.recv_timeout(START_TIMEOUT) {
        Ok(Ok(line)) if !line.trim().is_empty() => line,
        Ok(Ok(_)) => {
            return Err(failure(
                "the background picker exited before listening".into(),
            ));
        }
        Ok(Err(error)) => return Err(failure(format!("reading the background picker: {error}"))),
        Err(_) => {
            return Err(failure(format!(
                "the background picker did not start listening within {} s",
                START_TIMEOUT.as_secs()
            )));
        }
    };
    serde_json::from_str(line.trim()).map_err(|error| {
        failure(format!(
            "the background picker sent an invalid handshake ({error}): {line}"
        ))
    })
}

/// Keeps the background copy out of the frontend's process group and, on Windows, off any
/// console window, so it outlives the run that started it without showing anything.
fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}

/// A loopback listener waiting for the one client that knows its secret.
pub struct Listening {
    listener: TcpListener,
    handshake: Handshake,
}

/// Listens on a free loopback port with a fresh secret.
pub fn listen() -> Result<Listening> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .context("listening on a loopback port")?;
    let port = listener.local_addr()?.port();
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|error| anyhow::anyhow!("making a secret: {error}"))?;
    let token = secret
        .iter()
        .fold(String::with_capacity(64), |mut token, byte| {
            use std::fmt::Write as _;
            let _ = write!(token, "{byte:02x}");
            token
        });
    Ok(Listening {
        listener,
        handshake: Handshake {
            port,
            token,
            protocol: PROTOCOL_VERSION,
        },
    })
}

impl Listening {
    pub fn handshake(&self) -> &Handshake {
        &self.handshake
    }

    /// Prints the handshake for the foreground process and stops writing to standard
    /// output, whose reader is about to exit.
    pub fn announce(&self, out: &mut impl Write) -> Result<()> {
        writeln!(out, "{}", serde_json::to_string(&self.handshake)?)?;
        out.flush()?;
        Ok(())
    }

    /// Waits until `timeout` for a client that presents the secret, refusing any that does
    /// not, and stops listening once one has. Returns the client's connection and the
    /// protocol it speaks.
    pub fn accept(self, timeout: Duration) -> Result<Accepted> {
        let deadline = Instant::now() + timeout;
        self.listener.set_nonblocking(true)?;
        loop {
            if Instant::now() >= deadline {
                bail!("no client connected within {} s", timeout.as_secs());
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let hello_deadline = deadline.min(Instant::now() + HELLO_TIMEOUT);
                    if let Some(accepted) = self.verify(stream, hello_deadline) {
                        return Ok(accepted);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error).context("accepting a client"),
            }
        }
    }

    /// The client, set up for the session, if its first line, sent by `deadline`, presents
    /// the secret.
    fn verify(&self, stream: TcpStream, deadline: Instant) -> Option<Accepted> {
        stream.set_nonblocking(false).ok()?;
        stream.set_nodelay(true).ok()?;
        let line = read_line_unbuffered(&stream, 4096, deadline)?;
        let hello = serde_json::from_slice::<ClientMessage>(&line).ok()?;
        let ClientMessage::Hello { token, protocol } = hello else {
            return None;
        };
        if !constant_time_eq(token.as_bytes(), self.handshake.token.as_bytes()) {
            return None;
        }
        stream.set_read_timeout(None).ok()?;
        Some(Accepted { stream, protocol })
    }
}

/// A client that presented the secret.
#[derive(Debug)]
pub struct Accepted {
    pub stream: TcpStream,
    /// The protocol version it said it speaks.
    pub protocol: u32,
}

/// Reads one line a byte at a time, so nothing the client sends after it is taken from
/// the session that reads next. Gives up at `deadline`, however slowly bytes arrive.
fn read_line_unbuffered(
    mut stream: &TcpStream,
    limit: usize,
    deadline: Instant,
) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0_u8];
    while line.len() < limit {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        if remaining.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(remaining)).ok()?;
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        if byte[0] == b'\n' {
            return Some(line);
        }
        line.push(byte[0]);
    }
    None
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

/// A log file of this run's own: created new, under a name no one can have chosen in
/// advance, and on Unix readable by this user only, as the temporary directory may be
/// shared.
fn create_log(product_name: &str) -> Result<(PathBuf, File)> {
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(|error| anyhow::anyhow!("naming a log file: {error}"))?;
    let suffix = suffix.iter().fold(String::new(), |mut name, byte| {
        use std::fmt::Write as _;
        let _ = write!(name, "{byte:02x}");
        name
    });
    let path = std::env::temp_dir().join(format!("{product_name}-serve-{suffix}.log"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("creating {}", path.display()))?;
    Ok((path, file))
}

/// Removes the background copy's log once it has ended cleanly; a failed run keeps it.
pub fn remove_log() {
    if let Some(path) = std::env::var_os(LOG_VAR) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(token: &str) -> String {
        format!(r#"{{"t":"hello","token":"{token}","protocol":{PROTOCOL_VERSION}}}"#) + "\n"
    }

    #[test]
    fn a_client_with_the_secret_is_accepted_and_others_are_refused() {
        let listening = listen().unwrap();
        let Handshake { port, token, .. } = listening.handshake().clone();
        let clients = thread::spawn(move || {
            let mut impostor = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            impostor
                .write_all(hello("not-the-secret").as_bytes())
                .unwrap();
            let mut refused = Vec::new();
            impostor.read_to_end(&mut refused).unwrap();
            assert!(refused.is_empty(), "an impostor is sent nothing");
            let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            client.write_all(hello(&token).as_bytes()).unwrap();
            client
        });
        let accepted = listening.accept(Duration::from_secs(5)).unwrap();
        let client = clients.join().unwrap();
        assert_eq!(
            accepted.stream.peer_addr().unwrap(),
            client.local_addr().unwrap()
        );
        assert_eq!(accepted.protocol, PROTOCOL_VERSION);
        assert!(
            TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err(),
            "the listener stops once a client is accepted"
        );
    }

    #[test]
    fn a_client_trickling_bytes_cannot_hold_the_listener_past_its_deadline() {
        let listening = listen().unwrap();
        let port = listening.handshake().port;
        let trickle = thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            for _ in 0..40 {
                if stream.write_all(b"{").is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let started = Instant::now();
        let error = listening.accept(Duration::from_millis(300)).unwrap_err();
        assert!(error.to_string().contains("no client connected"), "{error}");
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "gave up after {:?}",
            started.elapsed()
        );
        trickle.join().unwrap();
    }

    #[test]
    fn waiting_for_a_client_gives_up() {
        let listening = listen().unwrap();
        let error = listening.accept(Duration::from_millis(50)).unwrap_err();
        assert!(error.to_string().contains("no client connected"), "{error}");
    }

    #[test]
    fn the_handshake_is_one_json_line() {
        let listening = listen().unwrap();
        let mut out = Vec::new();
        listening.announce(&mut out).unwrap();
        let line = String::from_utf8(out).unwrap();
        assert_eq!(line.matches('\n').count(), 1);
        let handshake: Handshake = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(&handshake, listening.handshake());
        assert_eq!(handshake.token.len(), 64);
        assert_eq!(handshake.protocol, PROTOCOL_VERSION);
    }

    #[test]
    fn each_run_logs_to_a_new_file_of_its_own() {
        let (first, _) = create_log("kiosk-test").unwrap();
        let (second, _) = create_log("kiosk-test").unwrap();
        assert_ne!(first, second);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }

    #[test]
    fn secrets_are_compared_in_full() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
