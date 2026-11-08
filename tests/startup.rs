//! The real binary, run the way the app's mod runs it.

use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_herdr-app-kiosk");

/// Stand-ins for `herdr-app` and `hc` with no spaces open, on a `PATH` of their own.
fn fake_app_programs(dir: &Path) -> OsString {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let spaces = r#"{"spaces":[]}"#;
    #[cfg(unix)]
    for name in ["herdr-app", "hc"] {
        use std::os::unix::fs::PermissionsExt;
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\nprintf '%s' '{spaces}'\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    for name in ["herdr-app", "hc"] {
        fs::write(
            bin.join(format!("{name}.cmd")),
            format!("@echo {spaces}\r\n"),
        )
        .unwrap();
    }
    let mut path = vec![bin];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(path).unwrap()
}

struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    fn send(&mut self, message: &Value) {
        self.writer
            .write_all(format!("{message}\n").as_bytes())
            .unwrap();
    }

    /// The next message other than a heartbeat.
    fn receive(&mut self) -> Value {
        loop {
            let mut line = String::new();
            assert!(
                self.reader.read_line(&mut line).unwrap() > 0,
                "connection closed"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["t"] != "ping" {
                return message;
            }
        }
    }

    /// Views until one other, which ends the session.
    fn end(&mut self) -> Value {
        loop {
            let message = self.receive();
            if message["t"] != "view" {
                return message;
            }
        }
    }
}

/// Runs `serve` with its config, state and fake programs under `root`, checks it returns
/// at once with a handshake, and connects to the picker it left listening.
fn serve(root: &Path) -> Client {
    let started = Instant::now();
    let output = Command::new(BINARY)
        .args(["serve", "--context", r#"{"focused_pane_cwd":null}"#])
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("PATH", fake_app_programs(root))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the command returns once the picker listens, not when it ends"
    );
    let handshake: Value = serde_json::from_slice(&output.stdout).unwrap();
    let port = u16::try_from(handshake["port"].as_u64().unwrap()).unwrap();
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut client = Client {
        reader: BufReader::new(stream.try_clone().unwrap()),
        writer: stream,
    };
    client.send(&json!({
        "t": "hello",
        "token": handshake["token"],
        "protocol": handshake["protocol"],
    }));
    client
}

fn write_config(root: &Path, contents: &str) {
    let config = root.join("config/herdr-app-kiosk");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), contents).unwrap();
}

#[test]
fn serve_runs_the_picker_in_the_background_for_the_mod() {
    let temp = tempfile::tempdir().unwrap();
    let code = temp.path().join("code");
    fs::create_dir_all(code.join("alpha/.git")).unwrap();
    write_config(
        temp.path(),
        &format!("search_dirs = [{:?}]\n", code.display().to_string()),
    );
    let mut client = serve(temp.path());
    let hello = client.receive();
    assert_eq!(hello["t"], "hello", "{hello}");
    assert_eq!(hello["product"], "herdr-app-kiosk");
    let rows = loop {
        let message = client.receive();
        assert_eq!(message["t"], "view", "{message}");
        let rows = &message["view"]["body"]["main"]["rows"];
        if rows.as_array().is_some_and(|rows| !rows.is_empty()) {
            break rows.clone();
        }
    };
    assert_eq!(rows[0]["name"]["text"], "alpha");
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
}

#[test]
fn an_invalid_config_is_reported_to_the_mod() {
    let temp = tempfile::tempdir().unwrap();
    write_config(temp.path(), "search_dirs = 42");
    let mut client = serve(temp.path());
    let error = client.end();
    assert_eq!(error["t"], "error", "{error}");
    assert!(
        error["body"]
            .as_str()
            .unwrap()
            .contains("invalid config TOML"),
        "{error}"
    );
}

#[test]
fn the_version_is_printed_for_installers_to_check() {
    let output = Command::new(BINARY).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("herdr-app-kiosk {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn an_unknown_command_exits_with_its_usage() {
    let output = Command::new(BINARY).arg("app").output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown command app"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("usage: herdr-app-kiosk"),
        "stderr was: {stderr}"
    );
}
