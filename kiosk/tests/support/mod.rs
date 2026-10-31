//! A throwaway tmux server for each test, with a client attached through a second
//! throwaway server so kiosk has a client to switch. Both use sockets named for the test
//! run, never the user's own server, and are killed when the test ends.

#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use kiosk_core::git::test_fixtures::repo_fixture;
use serde_json::Value;

pub const KIOSK: &str = env!("CARGO_BIN_EXE_kiosk");
const WAIT: Duration = Duration::from_secs(10);
/// The session the picker runs in.
pub const HARNESS: &str = "harness";

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct TestEnv {
    pub dir: tempfile::TempDir,
    /// The server kiosk works in.
    pub socket: String,
    /// The server whose pane holds the client attached to `socket`.
    client_socket: String,
}

impl TestEnv {
    /// A config searching `projects/`, with worktrees in `trees/` and `extra` appended.
    pub fn new(extra: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let id = format!(
            "{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let env = Self {
            socket: format!("kiosk-test-e2e-{id}"),
            client_socket: format!("kiosk-test-e2e-client-{id}"),
            dir,
        };
        for sub in ["projects", "config", "state", "trees", "bin"] {
            fs::create_dir_all(env.path(sub)).unwrap();
        }
        env.write_config(extra);
        env
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        kiosk_core::path::canonical_or_original(self.dir.path()).join(relative)
    }

    pub fn write_config(&self, extra: &str) {
        fs::write(
            self.path("config/config.toml"),
            format!(
                "search_dirs = [\"{}\"]\n[worktrees]\ndirectory = \"{}\"\n{extra}",
                self.path("projects").display(),
                self.path("trees").display()
            ),
        )
        .unwrap();
    }

    pub fn repo(&self, name: &str) -> PathBuf {
        repo_fixture(&self.path("projects"), name)
    }

    /// Environment assignments for commands run in a pane of the test server.
    fn pane_env(&self) -> String {
        format!(
            "KIOSK_CONFIG_DIR='{}' KIOSK_STATE_DIR='{}' PATH='{}':\"$PATH\"",
            self.path("config").display(),
            self.path("state").display(),
            self.path("bin").display()
        )
    }

    pub fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(["-L", &self.socket, "-f", "/dev/null"])
            .args(args)
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    /// Runs tmux and returns its output, failing the test if tmux fails.
    pub fn tmux_ok(&self, args: &[&str]) -> String {
        let output = self.tmux(args);
        assert!(
            output.status.success(),
            "tmux {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Starts the server with a `harness` session running `command` in `cwd`, and attaches
    /// a client to it.
    pub fn start(&self, cwd: &Path, command: &str) {
        self.tmux_ok(&[
            "new-session",
            "-d",
            "-s",
            HARNESS,
            "-x",
            "120",
            "-y",
            "30",
            "-c",
            cwd.to_str().unwrap(),
            &format!("{} {command}; sleep 30", self.pane_env()),
        ]);
        let attach = Command::new("tmux")
            .args([
                "-L",
                &self.client_socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-x",
                "120",
                "-y",
                "30",
                &format!("env -u TMUX tmux -L {} attach -t {HARNESS}", self.socket),
            ])
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(attach.status.success());
        wait_for("a client to attach", || {
            !self
                .tmux_ok(&["list-clients", "-F", "#{client_name}"])
                .trim()
                .is_empty()
        });
    }

    /// Replaces whatever the harness runs with `command`, run from `cwd`.
    pub fn respawn(&self, cwd: &Path, command: &str) {
        self.tmux_ok(&[
            "respawn-pane",
            "-k",
            "-t",
            HARNESS,
            "-c",
            cwd.to_str().unwrap(),
            &format!("{} {command}; sleep 30", self.pane_env()),
        ]);
    }

    /// Replaces whatever the harness runs with a new picker, from `cwd`.
    pub fn restart_picker(&self, cwd: &Path) {
        self.respawn(cwd, KIOSK);
        self.wait_for_screen("the picker", |screen| screen.contains("select repo"));
    }

    /// Starts the picker in the harness session, from `cwd`.
    pub fn start_picker(&self, cwd: &Path) {
        self.start(cwd, KIOSK);
        self.wait_for_screen("the picker", |screen| screen.contains("select repo"));
    }

    pub fn screen(&self) -> String {
        self.screen_of(HARNESS)
    }

    pub fn screen_of(&self, target: &str) -> String {
        String::from_utf8_lossy(&self.tmux(&["capture-pane", "-p", "-t", target]).stdout)
            .into_owned()
    }

    pub fn wait_for_screen(&self, what: &str, done: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let screen = self.screen();
            if done(&screen) {
                return screen;
            }
            assert!(
                start.elapsed() < WAIT,
                "timed out waiting for {what}; screen:\n{screen}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Sends keys to the harness, one tmux key name or literal text each.
    pub fn keys(&self, keys: &[&str]) {
        self.keys_to(HARNESS, keys);
    }

    /// Sends keys to `target` one at a time, as agents' TUIs drop keys sent together.
    pub fn keys_to(&self, target: &str, keys: &[&str]) {
        for key in keys {
            self.tmux_ok(&["send-keys", "-t", target, key]);
            thread::sleep(Duration::from_millis(300));
        }
    }

    pub fn type_text(&self, text: &str) {
        self.type_text_to(HARNESS, text);
    }

    pub fn type_text_to(&self, target: &str, text: &str) {
        self.tmux_ok(&["send-keys", "-t", target, "-l", text]);
        thread::sleep(Duration::from_millis(300));
    }

    /// `(id, name, kiosk path)` of every session.
    pub fn sessions(&self) -> Vec<(String, String, String)> {
        let output = self.tmux(&[
            "list-sessions",
            "-F",
            "#{session_id}\t#{session_name}\t#{@kiosk_path}",
        ]);
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let mut fields = line.split('\t').map(str::to_owned);
                (
                    fields.next().unwrap(),
                    fields.next().unwrap(),
                    fields.next().unwrap(),
                )
            })
            .collect()
    }

    pub fn client_session(&self) -> String {
        self.tmux_ok(&["list-clients", "-F", "#{session_name}"])
            .trim()
            .to_owned()
    }

    /// Runs kiosk outside tmux against the test server.
    pub fn kiosk(&self, args: &[&str]) -> Output {
        Command::new(KIOSK)
            .args(args)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env("KIOSK_TMUX_SOCKET", &self.socket)
            .env("KIOSK_CONFIG_DIR", self.path("config"))
            .env("KIOSK_STATE_DIR", self.path("state"))
            .output()
            .unwrap()
    }

    pub fn kiosk_ok(&self, args: &[&str]) -> String {
        let output = self.kiosk(args);
        assert!(
            output.status.success(),
            "kiosk {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Runs `command` in a new session `name` in `cwd`.
    pub fn session_running(&self, name: &str, cwd: &Path, command: &str) {
        self.tmux_ok(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            cwd.to_str().unwrap(),
            &format!("{} {command}", self.pane_env()),
        ]);
    }

    /// Writes an executable script into the `bin/` that panes have on `PATH`.
    pub fn script(&self, name: &str, body: &str) {
        let path = self.path("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
}

/// What `kiosk agents --json` reports for every pane with an agent.
pub fn agents(env: &TestEnv) -> Vec<Value> {
    serde_json::from_str::<Value>(&env.kiosk_ok(&["agents", "--json"]))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

pub fn agent_in<'a>(agents: &'a [Value], session: &str) -> Option<&'a Value> {
    agents.iter().find(|agent| agent["session_name"] == session)
}

/// How many of kiosk's OSC 7501 collector hooks the server has.
pub fn hooks(env: &TestEnv) -> usize {
    env.tmux_ok(&["show-hooks", "-g"])
        .lines()
        .filter(|line| line.contains("[7501]"))
        .count()
}

/// Starts the server and lets kiosk set its hooks, so new panes get collectors.
pub fn tracked_server(env: &TestEnv) {
    env.start(&env.path("projects"), "sleep 600");
    env.kiosk_ok(&["agents"]);
    assert_eq!(hooks(env), 3);
}

pub fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < WAIT, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for socket in [&self.client_socket, &self.socket] {
            let tmux = |args: &[&str]| {
                Command::new("tmux")
                    .args(["-L", socket])
                    .args(args)
                    .env_remove("TMUX")
                    .output()
            };
            let path = tmux(&["display-message", "-p", "#{socket_path}"])
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
            let _ = tmux(&["kill-server"]);
            if let Some(path) = path {
                let _ = fs::remove_file(path);
            }
        }
    }
}
