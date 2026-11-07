//! The serve command's two processes, for real: this test runs itself as the command a
//! frontend runs, which starts a copy of itself in the background. The command's output
//! must end once it has printed the handshake, while the background copy still waits for
//! a client; on Windows a background copy holding the command's own standard handles kept
//! that output open for as long as it ran.

use std::{
    io::Write,
    net::{Ipv4Addr, TcpStream},
    process::{Command, ExitCode, Stdio},
    time::{Duration, Instant},
};

use kiosk_serve::{
    launch::{self, Handshake, Stage},
    protocol::PROTOCOL_VERSION,
};

/// Run as the command: the argument this test passes itself.
const FOREGROUND: &str = "--as-serve-command";

fn main() -> ExitCode {
    match launch::stage() {
        Stage::Background => {
            let listening = launch::listen().expect("listening");
            listening
                .announce(&mut std::io::stdout().lock())
                .expect("announcing");
            listening
                .accept(Duration::from_secs(20))
                .expect("listening")
                .expect("a client");
            launch::remove_log();
            ExitCode::SUCCESS
        }
        Stage::Foreground if std::env::args().any(|arg| arg == FOREGROUND) => {
            let handshake = launch::start_background("kiosk-serve-detach-test").expect("starting");
            println!(
                "{}",
                serde_json::to_string(&handshake).expect("a handshake")
            );
            ExitCode::SUCCESS
        }
        Stage::Foreground => the_command_returns_while_the_picker_waits(),
    }
}

fn the_command_returns_while_the_picker_waits() -> ExitCode {
    let started = Instant::now();
    let output = Command::new(std::env::current_exe().expect("this test"))
        .arg(FOREGROUND)
        .stdin(Stdio::null())
        .output()
        .expect("running the command");
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the command's output stayed open for {elapsed:?}, as long as the picker ran"
    );
    let handshake: Handshake = serde_json::from_slice(&output.stdout).expect("a handshake");
    assert_eq!(handshake.protocol, PROTOCOL_VERSION);
    let mut client =
        TcpStream::connect((Ipv4Addr::LOCALHOST, handshake.port)).expect("the picker listens");
    writeln!(
        client,
        r#"{{"t":"hello","token":"{}","protocol":{PROTOCOL_VERSION}}}"#,
        handshake.token
    )
    .expect("saying hello");
    println!("test the_command_returns_while_the_picker_waits ... ok ({elapsed:?})");
    ExitCode::SUCCESS
}
