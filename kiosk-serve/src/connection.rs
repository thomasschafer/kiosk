//! The frontend's connection: messages in on a thread of their own, messages out from
//! whoever has something to say.

use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    net::{Shutdown, TcpStream},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};

use crate::protocol::{ClientMessage, NoticeLevel, ServerMessage};

/// The longest line a client may send; a paste is the longest thing it sends.
const MAX_LINE: u64 = 1 << 20;
/// How long a send may wait on a client that has stopped reading.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Names a file to append every message to, `> ` before what the frontend sent and `< `
/// before what the picker sent, for debugging a frontend and for its end-to-end tests.
pub const TRACE_VAR: &str = "KIOSK_SERVE_TRACE";

/// Where messages are traced, if anywhere.
#[derive(Clone)]
struct Trace(Option<Arc<Mutex<File>>>);

impl Trace {
    fn from_env() -> Result<Self> {
        let Some(path) = std::env::var_os(TRACE_VAR) else {
            return Ok(Self(None));
        };
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {TRACE_VAR} file {}", path.to_string_lossy()))?;
        Ok(Self(Some(Arc::new(Mutex::new(file)))))
    }

    fn record(&self, marker: &str, line: &[u8]) {
        let Some(file) = &self.0 else {
            return;
        };
        if let Ok(mut file) = file.lock() {
            let line = String::from_utf8_lossy(line);
            let _ = writeln!(file, "{marker} {}", line.trim_end());
        }
    }
}

pub(crate) enum Incoming {
    Message(ClientMessage),
    /// A line that is not a message, which ends the session.
    Invalid(String),
    /// The client went away.
    Closed,
}

struct Writer {
    stream: BufWriter<TcpStream>,
    last_sent: Instant,
    trace: Trace,
}

impl Writer {
    fn send(&mut self, message: &ServerMessage) -> Result<()> {
        let mut line = serde_json::to_vec(message)?;
        self.trace.record("<", &line);
        line.push(b'\n');
        self.stream.write_all(&line)?;
        self.stream.flush().context("sending to the frontend")?;
        self.last_sent = Instant::now();
        Ok(())
    }
}

pub struct Connection {
    writer: Arc<Mutex<Writer>>,
    incoming: mpsc::Receiver<Incoming>,
    stream: TcpStream,
}

impl Connection {
    pub fn new(stream: TcpStream) -> Result<Self> {
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        stream.set_nodelay(true)?;
        let reader = stream.try_clone()?;
        let trace = Trace::from_env()?;
        let writer = Writer {
            stream: BufWriter::new(stream.try_clone()?),
            last_sent: Instant::now(),
            trace: trace.clone(),
        };
        let (sender, incoming) = mpsc::channel();
        thread::spawn(move || read_messages(reader, &sender, &trace));
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            incoming,
            stream,
        })
    }

    pub(crate) fn send(&self, message: &ServerMessage) -> Result<()> {
        self.writer
            .lock()
            .map_err(|_| anyhow!("the connection's writer panicked"))?
            .send(message)
    }

    /// Sends a heartbeat if nothing has been sent for `interval`.
    pub(crate) fn keep_alive(&self, interval: Duration) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("the connection's writer panicked"))?;
        if writer.last_sent.elapsed() >= interval {
            writer.send(&ServerMessage::Ping)?;
        }
        Ok(())
    }

    pub(crate) fn receive(&self, timeout: Duration) -> Option<Incoming> {
        match self.incoming.recv_timeout(timeout) {
            Ok(incoming) => Some(incoming),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => Some(Incoming::Closed),
        }
    }

    /// A handle for posting notifications to the frontend's host, for backends.
    pub fn notifier(&self) -> Notifier {
        Notifier(Arc::clone(&self.writer))
    }

    /// Tells the frontend the picker has stopped because of `error`.
    pub fn fail(&self, title: &str, error: &anyhow::Error) {
        let body = format!("{error:#}");
        if let Err(send_error) = self.send(&ServerMessage::Error { title, body: &body }) {
            eprintln!("{title}: {body} (telling the frontend failed: {send_error:#})");
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn read_messages(stream: TcpStream, sender: &mpsc::Sender<Incoming>, trace: &Trace) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = Vec::new();
        let read = (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line);
        if matches!(read, Ok(read) if read > 0) {
            trace.record(">", &line);
        }
        let incoming = match read {
            Ok(0) | Err(_) => Incoming::Closed,
            Ok(_) if line.last() != Some(&b'\n') => Incoming::Invalid(format!(
                "a message longer than {MAX_LINE} bytes, or cut short"
            )),
            Ok(_) => match serde_json::from_slice::<ClientMessage>(&line) {
                Ok(message) => Incoming::Message(message),
                Err(error) => Incoming::Invalid(format!(
                    "{error}: {}",
                    String::from_utf8_lossy(&line).trim()
                )),
            },
        };
        let last = !matches!(incoming, Incoming::Message(_));
        if sender.send(incoming).is_err() || last {
            return;
        }
    }
}

/// Posts notifications to the frontend's host, which shows them even after the picker has
/// closed. Clones share the connection.
#[derive(Clone)]
pub struct Notifier(Arc<Mutex<Writer>>);

impl Notifier {
    pub fn notify(&self, title: &str, body: &str, level: NoticeLevel) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| anyhow!("the connection's writer panicked"))?
            .send(&ServerMessage::Notify { title, body, level })
    }
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Notifier")
    }
}
