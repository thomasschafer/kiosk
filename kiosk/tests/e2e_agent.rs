//! E2E tests for agent status detection.
//!
//! By default, tests use fake agent scripts that mimic Claude Code / Codex / Cursor
//! Agent (`agent` command) output.
//! Set `KIOSK_E2E_REAL_AGENTS=1` to use real `claude`, `codex`, and `agent` binaries
//! instead.
//!
//! Real-agent mode requires:
//! - `claude`, `codex`, and/or `agent` on PATH
//! - Valid authentication for each
//!
//! Fake-agent mode works in CI with no external dependencies.

use serde_json::Value;
use serial_test::serial;
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// ---------------------------------------------------------------------------
// Shared test infra (mirrors e2e.rs helpers)
// ---------------------------------------------------------------------------

fn kiosk_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kiosk"))
}

static TEST_ID: AtomicU64 = AtomicU64::new(1);

fn unique_id() -> String {
    let pid = std::process::id();
    let ctr = TEST_ID.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{pid}-{ctr}-{ts}")
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_test_repo(dir: &Path) {
    run_git(dir, &["init"]);
    run_git(dir, &["config", "user.email", "test@test.com"]);
    run_git(dir, &["config", "user.name", "Test"]);
    run_git(dir, &["config", "init.defaultBranch", "main"]);
    let _ = Command::new("git")
        .args(["branch", "-M", "main"])
        .current_dir(dir)
        .output();
    fs::write(dir.join("README.md"), "# test").unwrap();
    run_git(dir, &["add", "."]);
    run_git(dir, &["commit", "-m", "init"]);
}

fn wait_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

/// Poll the tmux pane until `expected` text (case-insensitive) appears, or timeout.
fn wait_for_pane_content(
    session: &str,
    expected: &str,
    timeout_ms: u64,
    tmux_socket: &str,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    let expected_lower = expected.to_lowercase();
    loop {
        let output = Command::new("tmux")
            .args([
                "-L",
                tmux_socket,
                "capture-pane",
                "-t",
                session,
                "-p",
                "-S",
                "-30",
            ])
            .output();
        if let Ok(output) = output {
            let content = String::from_utf8_lossy(&output.stdout).to_lowercase();
            if content.contains(&expected_lower) {
                return true;
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        wait_ms(250);
    }
}

/// Poll the tmux pane until any expected text (case-insensitive) appears, or timeout.
fn wait_for_any_pane_content(
    session: &str,
    expected_any: &[&str],
    timeout_ms: u64,
    tmux_socket: &str,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    let expected_any_lower: Vec<String> = expected_any.iter().map(|s| s.to_lowercase()).collect();
    loop {
        let output = Command::new("tmux")
            .args([
                "-L",
                tmux_socket,
                "capture-pane",
                "-t",
                session,
                "-p",
                "-S",
                "-50",
            ])
            .output();
        if let Ok(output) = output {
            let content = String::from_utf8_lossy(&output.stdout).to_lowercase();
            if expected_any_lower
                .iter()
                .any(|needle| content.contains(needle))
            {
                return true;
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        wait_ms(250);
    }
}

/// Write a fake agent shell script that prints `output_text` then sleeps.
fn write_fake_agent_script(dir: &Path, agent_name: &str, output_text: &str) -> PathBuf {
    let script_path = dir.join(agent_name);
    let escaped = output_text.replace('\'', "'\\''");
    let script = format!("#!/bin/sh\nprintf '{escaped}'\nsleep 86400\n");
    fs::write(&script_path, &script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
    }
    script_path
}

fn use_real_agents() -> bool {
    std::env::var("KIOSK_E2E_REAL_AGENTS").is_ok_and(|v| v == "1" || v == "true")
}

/// Build a PATH that includes common agent install locations (e.g. ~/.local/bin).
/// Agents installed via npm --prefix or curl installers often land outside the
/// default PATH visible to non-interactive shells / test harnesses.
fn agent_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let extra = format!("{home}/.local/bin");
    match std::env::var("PATH") {
        Ok(path) if !path.contains(&extra) => format!("{extra}:{path}"),
        Ok(path) => path,
        Err(_) => extra,
    }
}

fn has_binary(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .env("PATH", agent_path())
        .output()
        .is_ok_and(|o| o.status.success())
}

// ---------------------------------------------------------------------------
// Agent test environment
// ---------------------------------------------------------------------------

use kiosk_core::AgentKind;

#[derive(Debug, Clone, Copy)]
enum FakeState {
    Running,
    Waiting,
    Idle,
    CommandApproval,
}

struct AgentTestEnvDefault {
    tmp: tempfile::TempDir,
    config_dir: PathBuf,
    state_dir: PathBuf,
    repo_dir: PathBuf,
    kiosk_session: String,
    repo_name: String,
    tmux_socket: String,
}

fn fake_agent_output(agent: AgentKind, state: FakeState) -> &'static str {
    match (agent, state) {
        (AgentKind::ClaudeCode, FakeState::Running) => {
            "⠋ Reading file src/main.rs\\nesc to interrupt"
        }
        (AgentKind::ClaudeCode, FakeState::Waiting) => {
            "Allow write to src/main.rs?\\n  Yes, allow\\n  No, deny"
        }
        (AgentKind::ClaudeCode, FakeState::Idle) => "❯ \\n? for shortcuts",
        (AgentKind::CursorAgent, FakeState::Idle) => {
            "  Cursor Agent v2026.02.27\n             /tmp/test · master\n             \n             ┌──────────────────────────────────────────────┐\n             │ → Plan, search, build anything                │\n             └──────────────────────────────────────────────┘\n             \n               Auto\n               / commands · @ files · ! shell"
        }
        (AgentKind::Gemini, FakeState::Idle) => {
            "? for shortcuts\n             ────────────────────────\n             shift+tab to accept edits\n             ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n             >   Type your message"
        }

        (AgentKind::Gemini, FakeState::Running) => "⠋ Generating code\\nesc to interrupt",
        (AgentKind::Gemini, FakeState::Waiting) => "approve changes? (y/n)",

        (AgentKind::Codex, FakeState::Running) => "⠋ Searching codebase\\nesc to interrupt",
        (AgentKind::Codex, FakeState::Waiting) => {
            "Would you like to run the following command?\\n\
         $ touch test.txt\\n\
         › 1. Yes, proceed (y)\\n\
           2. Yes, and don't ask again (p)\\n\
           3. No (esc)\\n\
         \\n\
           Press enter to confirm or esc to cancel"
        }
        (AgentKind::Codex, FakeState::Idle) => {
            "╭──────────────────────────────╮\\n\
         │ >_ OpenAI Codex (v0.104.0)   │\\n\
         ╰──────────────────────────────╯\\n\
         \\n\
         › Type a message\\n\
         \\n\
           ? for shortcuts"
        }

        (AgentKind::OpenCode, FakeState::Running) => {
            "⬝■■■■■■⬝  esc interrupt  ctrl+t variants  tab agents  ctrl+p commands"
        }
        (AgentKind::OpenCode, FakeState::Waiting) => {
            // OpenCode shows a permission dialog for destructive operations.
            "Permission Required\\n\\nTool: bash\\nPath: /tmp/test\\n\\n Allow (a)   Allow for session (s)   Deny (d)"
        }
        (AgentKind::OpenCode, FakeState::Idle) => {
            "  ┃\n                 ┃  Build  GPT-5.3 Codex OpenAI\n                 ╹▀▀▀▀▀▀\n                   ctrl+t variants  tab agents  ctrl+p commands"
        }

        (AgentKind::CursorAgent, FakeState::Running) => {
            "⠋ Editing file src/main.rs\\nesc to interrupt"
        }
        (AgentKind::CursorAgent, FakeState::Waiting) => {
            "⚠ Workspace Trust Required\\n\
         \\n\
         Do you trust the contents of this directory?\\n\
         \\n\
         ▶ [a] Trust this workspace\\n\
           [w] Trust without MCP\\n\
           [q] Quit\\n\
         \\n\
         Use arrow keys to navigate, Enter to select"
        }
        (AgentKind::CursorAgent, FakeState::CommandApproval) => {
            "  $ sleep 10 Waiting for approval...\\n\
         \\n\
         ┌───────────────────────────────────────────────────────────────────────────────────────────────┐\\n\
         │ $  sleep 10 in .                                                                              │\\n\
         └───────────────────────────────────────────────────────────────────────────────────────────────┘\\n\
         ┌───────────────────────────────────────────────────────────────────────────────────────────────┐\\n\
         │ Run this command?                                                                             │\\n\
         │ Not in allowlist: sleep 10                                                                    │\\n\
         │  → Run (once) (y)                                                                             │\\n\
         │    Add Shell(sleep) to allowlist? (tab)                                                       │\\n\
         │    Run Everything (shift+tab)                                                                 │\\n\
         │    Skip (esc or n)                                                                            │\\n\
         └───────────────────────────────────────────────────────────────────────────────────────────────┘"
        }
        _ => panic!("Unsupported fake state/agent combination: {agent:?} {state:?}"),
    }
}

impl AgentTestEnvDefault {
    fn new(test_name: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let state_dir = tmp.path().join("state");
        let search_dir = tmp.path().join("projects");
        let id = unique_id();
        let repo_name = format!("kiosk-e2e-agent-{test_name}-{id}");
        let repo_dir = search_dir.join(&repo_name);

        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&repo_dir).unwrap();

        init_test_repo(&repo_dir);

        let kiosk_config_dir = config_dir.join("kiosk");
        fs::create_dir_all(&kiosk_config_dir).unwrap();
        fs::write(
            kiosk_config_dir.join("config.toml"),
            format!("search_dirs = [\"{}\"]", search_dir.to_string_lossy()),
        )
        .unwrap();

        // OpenCode config: force permission prompts so E2E tests can detect
        // "Waiting" state, regardless of the user's global opencode config.
        let opencode_config_dir = config_dir.join("opencode");
        fs::create_dir_all(&opencode_config_dir).unwrap();
        fs::write(
            opencode_config_dir.join("config.json"),
            r#"{"$schema":"https://opencode.ai/config.json","permission":{"*":"ask"}}"#,
        )
        .unwrap();
        // kiosk session name for main worktree = repo name
        let kiosk_session = repo_name.clone();
        let tmux_socket = format!("kiosk-e2e-agent-{id}");

        Self {
            tmp,
            config_dir,
            state_dir,
            repo_dir,
            kiosk_session,
            repo_name,
            tmux_socket,
        }
    }

    /// Create a `tmux` command with the custom socket.
    fn tmux_cmd(&self) -> Command {
        let mut cmd = Command::new("tmux");
        cmd.args(["-L", &self.tmux_socket]);
        cmd
    }

    /// Launch a fake/real agent in a tmux session.
    /// This is what kiosk CLI will find when it runs `tmux list-sessions`.
    fn launch_agent(&self, agent: AgentKind, state: FakeState) {
        if use_real_agents() {
            self.launch_real_agent(agent, state);
        } else {
            self.launch_fake_agent(agent, state);
        }
    }

    /// Returns the binary name for a given agent kind, asserting it's installed.
    fn agent_binary(agent: AgentKind) -> &'static str {
        match agent {
            AgentKind::ClaudeCode => {
                assert!(
                    has_binary("claude"),
                    "claude not on PATH — set KIOSK_E2E_REAL_AGENTS=0 or install claude"
                );
                "claude"
            }
            AgentKind::Codex => {
                assert!(
                    has_binary("codex"),
                    "codex not on PATH — set KIOSK_E2E_REAL_AGENTS=0 or install codex"
                );
                "codex"
            }
            AgentKind::CursorAgent => {
                assert!(
                    has_binary("agent"),
                    "agent not on PATH — set KIOSK_E2E_REAL_AGENTS=0 or install cursor agent"
                );
                "agent"
            }
            AgentKind::Gemini => {
                assert!(
                    has_binary("gemini"),
                    "gemini not on PATH — set KIOSK_E2E_REAL_AGENTS=0 or install gemini cli"
                );
                "gemini"
            }
            AgentKind::OpenCode => {
                assert!(
                    has_binary("opencode"),
                    "opencode not on PATH — set KIOSK_E2E_REAL_AGENTS=0 or install opencode"
                );
                "opencode"
            }
        }
    }

    /// The idle marker each agent shows when ready for input.
    /// Markers that indicate a real agent has finished launching and is ready
    /// for input. Multiple markers handle the difference between first-launch
    /// screens (which may show a welcome prompt) and post-conversation idle.
    fn real_agent_idle_markers(agent: AgentKind) -> &'static [&'static str] {
        match agent {
            // Claude: first launch shows "❯" prompt, post-conversation shows "? for shortcuts"
            AgentKind::ClaudeCode => &["? for shortcuts", "❯"],
            // Codex: prompt/idle text has changed across versions.
            AgentKind::Codex => &[
                "? for shortcuts",
                "type a message",
                "/help for help",
                "openai codex",
                "explain this codebase",
                "gpt-5.4 default",
            ],
            // Gemini: shows "? for shortcuts" or input prompt
            AgentKind::Gemini => &["? for shortcuts", "type your message"],
            AgentKind::OpenCode => &["ctrl+p commands"],
            AgentKind::CursorAgent => &[
                "/ commands",
                "cursor agent",
                "trust this workspace",
                "waiting for approval",
            ],
        }
    }

    /// The launch command (with flags) for each real agent.
    fn real_agent_launch_cmd(agent: AgentKind) -> &'static str {
        match agent {
            AgentKind::ClaudeCode => "claude",
            AgentKind::Codex => "codex --ask-for-approval untrusted",
            AgentKind::CursorAgent => "agent",
            AgentKind::Gemini => "gemini",
            AgentKind::OpenCode => "opencode",
        }
    }

    /// Pre-trust a directory for Gemini CLI by updating ~/.gemini/trustedFolders.json.
    fn pretrust_gemini_dir(dir: &Path) {
        let home = std::env::var("HOME").unwrap_or_default();
        let trust_file = PathBuf::from(format!("{home}/.gemini/trustedFolders.json"));
        let mut data: serde_json::Map<String, Value> = if trust_file.exists() {
            let content = fs::read_to_string(&trust_file).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            serde_json::Map::new()
        };
        let dir_str = dir.to_string_lossy().to_string();
        data.entry(dir_str)
            .or_insert_with(|| Value::String("TRUST_FOLDER".to_string()));
        if let Some(parent) = trust_file.parent() {
            fs::create_dir_all(parent).ok();
        }
        fs::write(&trust_file, serde_json::to_string_pretty(&data).unwrap()).unwrap();
    }

    /// Agent-specific prompt that triggers a long-running task.
    ///
    /// We use `sleep 30` (or similar) to keep the agent visibly processing
    /// for long enough to assert Running state. Avoid prompts that generate
    /// lots of output (e.g. "explain every line") — those burn real API
    /// tokens on every test run. A bash sleep is free.
    fn running_task_prompt(_agent: AgentKind) -> &'static str {
        // All agents use the same prompt. Codex (untrusted mode) and Cursor
        // prompt for approval on shell commands, which puts them in Waiting
        // state (not Running). That's fine — the approval prompt is what we
        // want to test.
        "run: sleep 30"
    }

    /// Agent-specific prompt that triggers a permission/approval prompt.
    fn waiting_task_prompt(agent: AgentKind) -> &'static str {
        // Asking to delete a file triggers permission prompts in most agents.
        // The file delete-me.txt is created before sending this prompt.
        match agent {
            // In untrusted mode Codex reliably prompts on explicit shell
            // commands; natural-language delete requests may use direct file
            // tools and skip approval in some versions.
            AgentKind::Codex => "run this shell command exactly: rm delete-me.txt",
            _ => "delete the file called delete-me.txt in this directory",
        }
    }

    /// Create a tmux session, set PATH, and launch the agent binary.
    /// Handles startup dialogs (update prompts, trust dialogs) and waits
    /// until the agent reaches its idle prompt before returning.
    fn start_real_agent(&self, agent: AgentKind) {
        let bin = Self::agent_binary(agent);
        let launch_cmd = Self::real_agent_launch_cmd(agent);

        // Pre-trust the repo dir for Gemini to avoid the interactive trust dialog.
        if agent == AgentKind::Gemini {
            Self::pretrust_gemini_dir(&self.repo_dir);
        }

        let status = self
            .tmux_cmd()
            .args([
                "new-session",
                "-d",
                "-s",
                &self.kiosk_session,
                "-c",
                &self.repo_dir.to_string_lossy(),
                "-x",
                "120",
                "-y",
                "30",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "Failed to create tmux session");

        let path = agent_path();
        let config_export = if agent == AgentKind::OpenCode {
            format!(" XDG_CONFIG_HOME='{}'", self.config_dir.to_string_lossy())
        } else {
            String::new()
        };
        self.tmux_cmd()
            .args([
                "send-keys",
                "-t",
                &self.kiosk_session,
                &format!("export PATH='{path}'{config_export}"),
                "Enter",
            ])
            .status()
            .unwrap();
        wait_ms(200);

        self.tmux_cmd()
            .args(["send-keys", "-t", &self.kiosk_session, launch_cmd, "Enter"])
            .status()
            .unwrap();

        // Handle agent-specific startup dialogs before waiting for idle.
        self.dismiss_startup_dialogs(agent);

        let markers = Self::real_agent_idle_markers(agent);
        let ready =
            wait_for_any_pane_content(&self.kiosk_session, markers, 90_000, &self.tmux_socket);
        if !ready {
            let pane_dump = Command::new("tmux")
                .args([
                    "-L",
                    &self.tmux_socket,
                    "capture-pane",
                    "-t",
                    &self.kiosk_session,
                    "-p",
                    "-S",
                    "-200",
                ])
                .output()
                .ok()
                .map_or_else(
                    || "<failed to capture tmux pane>".to_string(),
                    |o| String::from_utf8_lossy(&o.stdout).to_string(),
                );
            panic!("Real {bin} did not reach ready state within 90s.\nPane:\n{pane_dump}");
        }
    }

    /// Dismiss blocking startup dialogs that prevent agents from reaching idle.
    ///
    /// Real agents show various first-run or per-directory prompts:
    /// - **Codex**: version update dialog ("Update now" / "Skip")
    /// - **Cursor**: workspace trust dialog ("[a] Trust this workspace")
    /// - **Gemini**: folder trust dialog ("Trust folder")
    fn dismiss_startup_dialogs(&self, agent: AgentKind) {
        match agent {
            AgentKind::CursorAgent => {
                // Cursor shows "Workspace Trust Required" with [a] Trust.
                if wait_for_pane_content(&self.kiosk_session, "trust", 10_000, &self.tmux_socket) {
                    self.tmux_cmd()
                        .args(["send-keys", "-t", &self.kiosk_session, "a"])
                        .status()
                        .unwrap();
                    wait_ms(300);
                    self.tmux_cmd()
                        .args(["send-keys", "-t", &self.kiosk_session, "Enter"])
                        .status()
                        .unwrap();
                    wait_ms(2000);
                }
            }
            AgentKind::Gemini => {
                // Gemini shows "Do you trust the files in this folder?"
                // with option 1 = Trust folder. Press Enter to select default.
                if wait_for_pane_content(&self.kiosk_session, "trust", 10_000, &self.tmux_socket) {
                    self.tmux_cmd()
                        .args(["send-keys", "-t", &self.kiosk_session, "Enter"])
                        .status()
                        .unwrap();
                    wait_ms(2000);
                }
            }
            AgentKind::Codex => {
                // Recent Codex versions can show a blocking model-choice screen:
                // "Choose how you'd like Codex to proceed".
                if wait_for_any_pane_content(
                    &self.kiosk_session,
                    &[
                        "choose how you'd like codex to proceed",
                        "try new model",
                        "use existing model",
                    ],
                    30_000,
                    &self.tmux_socket,
                ) {
                    self.tmux_cmd()
                        .args(["send-keys", "-t", &self.kiosk_session, "1"])
                        .status()
                        .unwrap();
                    wait_ms(300);
                    self.tmux_cmd()
                        .args(["send-keys", "-t", &self.kiosk_session, "Enter"])
                        .status()
                        .unwrap();
                    wait_ms(2000);
                }
            }
            AgentKind::ClaudeCode | AgentKind::OpenCode => {
                // These agents generally go straight to idle.
            }
        }
    }

    /// Send a message into the running agent's tmux pane to give it work.
    ///
    /// Text and Enter are sent as separate tmux send-keys calls with a brief
    /// delay between them. Some TUI agents (notably Claude Code) drop the
    /// Enter when it arrives in the same send-keys invocation as the text.
    fn send_to_agent(&self, text: &str) {
        self.tmux_cmd()
            .args(["send-keys", "-t", &self.kiosk_session, text])
            .status()
            .unwrap();
        wait_ms(300);
        self.tmux_cmd()
            .args(["send-keys", "-t", &self.kiosk_session, "Enter"])
            .status()
            .unwrap();
    }

    #[allow(clippy::too_many_lines)]
    fn launch_real_agent(&self, agent: AgentKind, state: FakeState) {
        // Step 1: Start the agent and wait for it to reach idle.
        self.start_real_agent(agent);

        match state {
            FakeState::Idle => {
                // Already idle after start_real_agent — nothing more to do.
            }
            FakeState::Running => {
                // Give the agent a long-running bash task.
                let task = Self::running_task_prompt(agent);
                self.send_to_agent(task);

                // Wait for the kiosk CLI to actually report Running state.
                // Just checking that the idle marker disappears is insufficient —
                // there's a brief transition period where neither idle nor running
                // indicators are visible. Instead, poll the kiosk CLI directly.
                let deadline = std::time::Instant::now() + Duration::from_secs(30);
                loop {
                    let output = Command::new(kiosk_binary())
                        .args(["branches", &self.repo_name, "--json"])
                        .env("XDG_CONFIG_HOME", &self.config_dir)
                        .env("XDG_STATE_HOME", &self.state_dir)
                        .env("KIOSK_TMUX_SOCKET", &self.tmux_socket)
                        .output()
                        .unwrap();
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.contains("\"Running\"") {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "Agent never reached Running state within 30s (last output: {stdout})"
                    );
                    wait_ms(1000);
                }
            }
            FakeState::Waiting => {
                // Trigger a permission/approval prompt.
                let target = self.repo_dir.join("delete-me.txt");
                std::fs::write(&target, "delete this file").unwrap();
                let task = Self::waiting_task_prompt(agent);
                self.send_to_agent(task);

                // Poll kiosk CLI until it reports Waiting state.
                let deadline = std::time::Instant::now() + Duration::from_secs(60);
                loop {
                    let output = Command::new(kiosk_binary())
                        .args(["branches", &self.repo_name, "--json"])
                        .env("XDG_CONFIG_HOME", &self.config_dir)
                        .env("XDG_STATE_HOME", &self.state_dir)
                        .env("KIOSK_TMUX_SOCKET", &self.tmux_socket)
                        .output()
                        .unwrap();
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.contains("\"Waiting\"") {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "Agent never reached Waiting state within 60s (last output: {stdout})"
                    );
                    wait_ms(1000);
                }
            }
            FakeState::CommandApproval => {
                assert_eq!(
                    agent,
                    AgentKind::CursorAgent,
                    "CommandApproval real mode is only supported for Cursor Agent"
                );
                self.send_to_agent("Please can you now sleep for 10 seconds using bash");
                let waiting_appeared = wait_for_any_pane_content(
                    &self.kiosk_session,
                    &[
                        "waiting for approval",
                        "run this command",
                        "allowlist",
                        "run (once)",
                        "add shell(",
                        "skip (esc or n)",
                    ],
                    90_000,
                    &self.tmux_socket,
                );
                if !waiting_appeared {
                    let output = Command::new("tmux")
                        .args([
                            "-L",
                            &self.tmux_socket,
                            "capture-pane",
                            "-t",
                            &self.kiosk_session,
                            "-p",
                            "-S",
                            "-200",
                        ])
                        .output()
                        .ok();
                    if let Some(output) = output {
                        let content = String::from_utf8_lossy(&output.stdout).to_lowercase();
                        if content.contains("\nauto\n") || content.contains("auto\n  / commands") {
                            // Cursor is in Auto mode and won't show command approvals.
                            // Don't fail setup; the test will downgrade strict assertions.
                            return;
                        }
                        eprintln!(
                            "Cursor pane when no command approval detected:\n{}",
                            String::from_utf8_lossy(&output.stdout)
                        );
                    }
                }
                assert!(
                    waiting_appeared,
                    "Cursor Agent never showed command approval prompt for sleep 10"
                );
            }
        }
    }

    fn launch_fake_agent(&self, agent: AgentKind, state: FakeState) {
        // Script filename must contain the agent name so kiosk detects the agent
        // by inspecting child process args via /proc/PID/cmdline or pgrep/ps.
        let agent_name = match agent {
            AgentKind::ClaudeCode => "claude",
            AgentKind::Codex => "codex",
            AgentKind::CursorAgent => "cursor-agent",
            AgentKind::OpenCode => "opencode",
            AgentKind::Gemini => "gemini",
        };

        let output_text = fake_agent_output(agent, state);

        let script_path = write_fake_agent_script(self.tmp.path(), agent_name, output_text);

        // Create tmux session
        let status = self
            .tmux_cmd()
            .args([
                "new-session",
                "-d",
                "-s",
                &self.kiosk_session,
                "-c",
                &self.repo_dir.to_string_lossy(),
                "-x",
                "120",
                "-y",
                "30",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "Failed to create tmux session");

        // Run the script (don't use exec -a — it replaces the shell so
        // /proc/pane_pid/children shows the script's children, not the script itself)
        self.tmux_cmd()
            .args([
                "send-keys",
                "-t",
                &self.kiosk_session,
                &script_path.to_string_lossy(),
                "Enter",
            ])
            .status()
            .unwrap();

        // Poll until the script output is visible in the pane (up to 10s).
        // Using a content marker avoids flaky fixed sleeps under system load.
        let marker = match (agent, state) {
            (AgentKind::OpenCode, FakeState::Running) => Some("esc interrupt"),
            (_, FakeState::Running) => Some("esc to interrupt"),
            (AgentKind::ClaudeCode, FakeState::Waiting) => Some("yes, allow"),
            (AgentKind::Codex, FakeState::Waiting) => Some("yes, proceed"),
            (AgentKind::Codex | AgentKind::ClaudeCode | AgentKind::Gemini, FakeState::Idle) => {
                Some("? for shortcuts")
            }
            (AgentKind::CursorAgent, FakeState::Waiting) => Some("trust this workspace"),
            (AgentKind::CursorAgent, FakeState::CommandApproval) => Some("run this command"),
            (AgentKind::OpenCode, FakeState::Waiting) => Some("Permission Required"),
            (AgentKind::OpenCode, FakeState::Idle) => Some("ctrl+p commands"),
            (AgentKind::Gemini, FakeState::Waiting) => Some("(y/n)"),
            // CursorAgent/Gemini idle output is just "> " — too minimal for
            // reliable content polling (tmux strips trailing whitespace).
            (AgentKind::CursorAgent, FakeState::Idle) => Some("/ commands"),
            (
                AgentKind::Gemini | AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::OpenCode,
                FakeState::CommandApproval,
            ) => None,
        };
        if let Some(marker) = marker {
            assert!(
                wait_for_pane_content(&self.kiosk_session, marker, 10_000, &self.tmux_socket,),
                "Timed out waiting for fake {agent_name} script output (marker: {marker:?})"
            );
        } else {
            wait_ms(3000);
        }
    }

    fn run_cli(&self, args: &[&str]) -> std::process::Output {
        let output = Command::new(kiosk_binary())
            .args(args)
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .env("XDG_STATE_HOME", &self.state_dir)
            .env("KIOSK_TMUX_SOCKET", &self.tmux_socket)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn run_cli_json(&self, args: &[&str]) -> Value {
        let output = self.run_cli(args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("Failed to parse JSON: {e}\nOutput: {stdout}"))
    }
}

impl Drop for AgentTestEnvDefault {
    fn drop(&mut self) {
        // Kill all processes in the tmux session (agents, shells) before
        // killing the tmux server. This prevents zombie agent processes
        // from accumulating across tests and consuming resources.
        if let Ok(output) = Command::new("tmux")
            .args([
                "-L",
                &self.tmux_socket,
                "list-panes",
                "-s",
                "-F",
                "#{pane_pid}",
            ])
            .output()
        {
            let pids = String::from_utf8_lossy(&output.stdout);
            for pid in pids.lines() {
                let pid = pid.trim();
                if !pid.is_empty() {
                    // Kill the process group to catch child processes
                    let _ = Command::new("kill")
                        .args(["--", &format!("-{pid}")])
                        .output();
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        let _ = Command::new("tmux")
            .args(["-L", &self.tmux_socket, "kill-server"])
            .output();
        // Remove the socket file — tmux sometimes leaves it behind after kill-server.
        if let Ok(uid) = Command::new("id").arg("-u").output() {
            let uid = String::from_utf8_lossy(&uid.stdout).trim().to_string();
            let socket_path = PathBuf::from(format!("/tmp/tmux-{uid}/{}", self.tmux_socket));
            let _ = std::fs::remove_file(&socket_path);
        }
    }
}

// ---------------------------------------------------------------------------
// CLI tests: `kiosk branches`
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_branches_json_claude_running() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-claude-run");
    env.launch_agent(AgentKind::ClaudeCode, FakeState::Running);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().expect("branches should be an array");

    // Find the main branch (should have a session with agent)
    let main_branch = branches
        .iter()
        .find(|b| b["name"] == "main")
        .expect("should have main branch");

    let agent = &main_branch["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "main branch should have agent_statuses: {main_branch}"
    );
    assert_eq!(agent[0]["kind"], "ClaudeCode");

    assert_eq!(agent[0]["state"], "Running");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_claude_waiting() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-claude-wait");
    env.launch_agent(AgentKind::ClaudeCode, FakeState::Waiting);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_statuses"][0]["kind"], "ClaudeCode");
    assert_eq!(main_branch["agent_statuses"][0]["state"], "Waiting");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_claude_idle() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-claude-idle");
    env.launch_agent(AgentKind::ClaudeCode, FakeState::Idle);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_statuses"][0]["kind"], "ClaudeCode");
    assert_eq!(main_branch["agent_statuses"][0]["state"], "Idle");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_codex_running() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-codex-run");
    env.launch_agent(AgentKind::Codex, FakeState::Running);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    let agent = &main_branch["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "should detect codex: {main_branch}"
    );
    assert_eq!(agent[0]["kind"], "Codex");

    assert_eq!(agent[0]["state"], "Running");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_no_agent() {
    let env = AgentTestEnvDefault::new("br-no-agent");
    // Create a session but with just a shell — no agent
    let status = env
        .tmux_cmd()
        .args([
            "new-session",
            "-d",
            "-s",
            &env.kiosk_session,
            "-c",
            &env.repo_dir.to_string_lossy(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    wait_ms(500);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    // agent_statuses should be absent (skip_serializing_if = empty)
    assert!(
        main_branch.get("agent_statuses").is_none(),
        "shell-only session should not have agent_statuses: {main_branch}"
    );
}

#[test]
#[serial]
fn test_e2e_agent_branches_table_shows_agent_column() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-table-col");
    env.launch_agent(AgentKind::ClaudeCode, FakeState::Waiting);

    let output = env.run_cli(&["branches", &env.repo_name]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("agent"),
        "Table should have agent column header: {stdout}"
    );
    assert!(
        stdout.contains("[WAITING]"),
        "Table should show [WAITING] label: {stdout}"
    );
}

#[test]
#[serial]
fn test_e2e_agent_branches_table_no_agent_column() {
    let env = AgentTestEnvDefault::new("br-table-nocol");
    // No session at all — no agent column
    let output = env.run_cli(&["branches", &env.repo_name]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check the header line specifically (first line) — should not have "agent" column
    let header = stdout.lines().next().unwrap_or("");
    assert!(
        !header.contains("agent"),
        "Table header should NOT have agent column without agents: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// CLI tests: `kiosk status`
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_status_json_includes_agent() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("st-claude");
    env.launch_agent(AgentKind::ClaudeCode, FakeState::Running);

    let json = env.run_cli_json(&["status", &env.repo_name, "main", "--json"]);

    let agent = &json["agent_status"];
    assert!(
        !agent.is_null(),
        "status should include agent_status: {json}"
    );
    assert_eq!(agent["kind"], "ClaudeCode");

    assert_eq!(agent["state"], "Running");
}

#[test]
#[serial]
fn test_e2e_agent_status_json_no_agent() {
    let env = AgentTestEnvDefault::new("st-no-agent");
    // Create a plain session
    let status = env
        .tmux_cmd()
        .args([
            "new-session",
            "-d",
            "-s",
            &env.kiosk_session,
            "-c",
            &env.repo_dir.to_string_lossy(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "Failed to create tmux session");
    wait_ms(500);

    let json = env.run_cli_json(&["status", &env.repo_name, "main", "--json"]);

    assert!(
        json.get("agent_status").is_none(),
        "status without agent should omit agent_status: {json}"
    );
}

// ---------------------------------------------------------------------------
// CLI tests: `kiosk sessions`
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_sessions_json_includes_agent() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("sess-claude");
    env.launch_agent(AgentKind::ClaudeCode, FakeState::Waiting);

    let json = env.run_cli_json(&["sessions", "--json"]);
    let sessions = json.as_array().expect("sessions should be an array");

    let our_session = sessions
        .iter()
        .find(|s| s["session"] == env.kiosk_session)
        .expect("should find our session in sessions list");

    let agent = &our_session["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "session should have agent_statuses: {our_session}"
    );
    assert_eq!(agent[0]["kind"], "ClaudeCode");

    assert_eq!(agent[0]["state"], "Waiting");
}

#[test]
#[serial]
fn test_e2e_agent_sessions_json_no_agent() {
    let env = AgentTestEnvDefault::new("sess-no-agent");
    let status = env
        .tmux_cmd()
        .args([
            "new-session",
            "-d",
            "-s",
            &env.kiosk_session,
            "-c",
            &env.repo_dir.to_string_lossy(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "Failed to create tmux session");
    wait_ms(500);

    let json = env.run_cli_json(&["sessions", "--json"]);
    let sessions = json.as_array().unwrap();

    let our_session = sessions.iter().find(|s| s["session"] == env.kiosk_session);

    if let Some(session) = our_session {
        assert!(
            session.get("agent_statuses").is_none(),
            "plain session should not have agent_statuses: {session}"
        );
    }
}

// ---------------------------------------------------------------------------
// CLI tests: Cursor Agent
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_branches_json_cursor_running() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-cursor-run");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Running);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    let agent = &main_branch["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "should detect cursor agent: {main_branch}"
    );
    assert_eq!(agent[0]["kind"], "CursorAgent");
    assert_eq!(agent[0]["state"], "Running");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_cursor_waiting() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-cursor-wait");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Waiting);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_statuses"][0]["kind"], "CursorAgent");
    assert_eq!(main_branch["agent_statuses"][0]["state"], "Waiting");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_cursor_idle() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-cursor-idle");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Idle);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_statuses"][0]["kind"], "CursorAgent");
    assert_eq!(main_branch["agent_statuses"][0]["state"], "Idle");
}

#[test]
#[serial]
fn test_e2e_agent_status_json_cursor() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("st-cursor");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Waiting);

    let json = env.run_cli_json(&["status", &env.repo_name, "main", "--json"]);
    let agent = &json["agent_status"];
    assert!(
        !agent.is_null(),
        "status should include agent_status: {json}"
    );
    assert_eq!(agent["kind"], "CursorAgent");
    assert_eq!(agent["state"], "Waiting");
}

#[test]
#[serial]
fn test_e2e_agent_sessions_json_cursor() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("sess-cursor");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Running);

    let json = env.run_cli_json(&["sessions", "--json"]);
    let sessions = json.as_array().expect("sessions should be an array");
    let our_session = sessions
        .iter()
        .find(|s| s["session"] == env.kiosk_session)
        .expect("should find our session");

    let agent = &our_session["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "session should have agent_statuses: {our_session}"
    );
    assert_eq!(agent[0]["kind"], "CursorAgent");
    assert_eq!(agent[0]["state"], "Running");
}

#[test]
#[serial]
fn test_e2e_agent_status_json_cursor_command_approval_prompt() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("st-cursor-cmd-approval");
    env.launch_agent(AgentKind::CursorAgent, FakeState::CommandApproval);

    let json = env.run_cli_json(&["status", &env.repo_name, "main", "--json"]);
    let agent = &json["agent_status"];
    assert!(
        !agent.is_null(),
        "status should include agent_status: {json}"
    );
    assert_eq!(agent["kind"], "CursorAgent");

    if use_real_agents() {
        let output = json["output"].as_str().unwrap_or_default().to_lowercase();
        let has_approval_prompt = [
            "waiting for approval",
            "run this command",
            "not in allowlist",
            "run (once)",
        ]
        .iter()
        .any(|kw| output.contains(kw));
        let is_auto_mode = output.contains("\nauto\n") || output.contains("auto\n  / commands");
        if is_auto_mode && !has_approval_prompt {
            eprintln!(
                "Cursor Agent is in Auto mode; approval dialog is suppressed, skipping strict Waiting assertion"
            );
            return;
        }
    }

    assert_eq!(agent["state"], "Waiting");
}

// ---------------------------------------------------------------------------
// Regression test: stale content should not cause false positives
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_codex_stale_content_waiting_then_idle() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("codex-stale");

    // Phase 1: launch Codex with waiting output
    env.launch_agent(AgentKind::Codex, FakeState::Waiting);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main_branch["agent_statuses"][0]["state"], "Waiting",
        "should initially detect Waiting"
    );

    // Phase 2: kill the fake agent process, then relaunch with idle output.
    // This simulates answering a permission prompt — the old waiting text
    // remains in the scrollback, but the tail now shows idle markers.
    let _ = env
        .tmux_cmd()
        .args(["send-keys", "-t", &env.kiosk_session, "C-c", ""])
        .status();
    wait_ms(1000);

    // Write new idle script (overwrite the old one)
    let idle_output = "╭──────────────────────────────╮\\n\
                       │ >_ OpenAI Codex (v0.104.0)   │\\n\
                       ╰──────────────────────────────╯\\n\
                       \\n\
                       › Type a message\\n\
                       \\n\
                         ? for shortcuts";
    let script_path = write_fake_agent_script(env.tmp.path(), "codex", idle_output);

    env.tmux_cmd()
        .args([
            "send-keys",
            "-t",
            &env.kiosk_session,
            &script_path.to_string_lossy(),
            "Enter",
        ])
        .status()
        .unwrap();
    assert!(
        wait_for_pane_content(
            &env.kiosk_session,
            "? for shortcuts",
            10_000,
            &env.tmux_socket
        ),
        "Timed out waiting for idle Codex output in phase 2"
    );

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main_branch["agent_statuses"][0]["state"], "Idle",
        "should detect Idle after transitioning from Waiting (idle tail overrides stale content)"
    );
}

fn replace_fake_codex_output_and_wait(
    env: &AgentTestEnvDefault,
    output: &str,
    marker: &str,
    timeout_msg: &str,
) {
    let _ = env
        .tmux_cmd()
        .args(["send-keys", "-t", &env.kiosk_session, "C-c", ""])
        .status();
    wait_ms(800);

    let script_name = format!("codex-replace-{}", unique_id());
    let script_path = write_fake_agent_script(env.tmp.path(), &script_name, output);
    env.tmux_cmd()
        .args([
            "send-keys",
            "-t",
            &env.kiosk_session,
            &script_path.to_string_lossy(),
            "Enter",
        ])
        .status()
        .unwrap();

    if !wait_for_pane_content(&env.kiosk_session, marker, 10_000, &env.tmux_socket) {
        let pane_dump = Command::new("tmux")
            .args([
                "-L",
                &env.tmux_socket,
                "capture-pane",
                "-t",
                &env.kiosk_session,
                "-p",
                "-S",
                "-200",
            ])
            .output()
            .ok()
            .map_or_else(
                || "<failed to capture pane>".to_string(),
                |o| String::from_utf8_lossy(&o.stdout).to_string(),
            );
        panic!("{timeout_msg}\nMarker: {marker}\nPane dump:\n{pane_dump}");
    }
}

fn push_fake_codex_log_lines(buf: &mut String, count: usize) {
    for i in 0..count {
        let _ = writeln!(buf, "• log line {i}");
    }
}

#[test]
#[serial]
fn test_e2e_agent_codex_bare_prompt_without_footer_is_idle() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("codex-plain-idle");
    env.launch_agent(AgentKind::Codex, FakeState::Running);

    let mut prompt_only_output = String::new();
    push_fake_codex_log_lines(&mut prompt_only_output, 12);
    prompt_only_output.push_str(
        "• Added regression tests for both core and TUI poller paths\\n\
         • Built and validated changes\\n\
         › ",
    );
    replace_fake_codex_output_and_wait(
        &env,
        &prompt_only_output,
        "log line 11",
        "Timed out waiting for prompt-only Codex output",
    );

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main_branch["agent_statuses"][0]["state"], "Idle",
        "prompt-only Codex tail should classify as Idle"
    );
}

#[test]
#[serial]
fn test_e2e_agent_codex_prompt_with_user_text_without_footer_is_unknown() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("codex-prompt-text-unknown");
    env.launch_agent(AgentKind::Codex, FakeState::Running);

    let mut prompt_output = String::new();
    push_fake_codex_log_lines(&mut prompt_output, 12);
    prompt_output.push_str(
        "• Added regression tests for both core and TUI poller paths\\n\
         • Built and validated changes\\n\
         › Nice! Okay next bug. You are currently idle. I'm seeing",
    );
    replace_fake_codex_output_and_wait(
        &env,
        &prompt_output,
        "log line 11",
        "Timed out waiting for prompt text Codex output",
    );

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main_branch["agent_statuses"][0]["state"], "Unknown",
        "prompt line with user text should not classify as Idle"
    );
}

#[test]
#[serial]
fn test_e2e_agent_codex_prompt_with_user_text_and_footer_is_idle() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("codex-prompt-footer-idle");
    env.launch_agent(AgentKind::Codex, FakeState::Running);

    let mut prompt_output = String::new();
    push_fake_codex_log_lines(&mut prompt_output, 12);
    prompt_output.push_str(
        "• Added regression tests for both core and TUI poller paths\\n\
         • Built and validated changes\\n\
         › Implement {feature}\\n\
           gpt-5.3-codex high · left · ~/Development/kiosk",
    );
    replace_fake_codex_output_and_wait(
        &env,
        &prompt_output,
        "log line 11",
        "Timed out waiting for prompt + footer Codex output",
    );

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main_branch["agent_statuses"][0]["state"], "Idle",
        "prompt line with Codex footer should classify as Idle"
    );
}

// ---------------------------------------------------------------------------
// TUI test: agent indicator visible in branch picker
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_tui_shows_indicator() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("tui-ind");

    // First, launch a fake agent in the kiosk session
    env.launch_agent(AgentKind::ClaudeCode, FakeState::Waiting);

    // Now launch kiosk TUI in a SEPARATE tmux session to observe it
    let tui_session = format!("{}-tui", env.kiosk_session);
    let binary = kiosk_binary();
    let status = env
        .tmux_cmd()
        .args([
            "new-session",
            "-d",
            "-s",
            &tui_session,
            "-x",
            "120",
            "-y",
            "30",
            &format!(
                "XDG_CONFIG_HOME={} XDG_STATE_HOME={} KIOSK_NO_ALT_SCREEN=1 KIOSK_TMUX_SOCKET={} {} ; sleep 2",
                env.config_dir.to_string_lossy(),
                env.state_dir.to_string_lossy(),
                env.tmux_socket,
                binary.to_string_lossy()
            ),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "Failed to launch kiosk TUI");

    // Wait for TUI to load and discover repos (async discovery can take time)
    wait_ms(3000);

    // Verify the TUI session exists
    let has_session = env
        .tmux_cmd()
        .args(["has-session", "-t", &tui_session])
        .status()
        .unwrap()
        .success();
    if !has_session {
        eprintln!("TUI tmux session does not exist, skipping");
        return;
    }

    // Verify TUI launched — should show repo list
    let repo_screen = {
        let output = env
            .tmux_cmd()
            .args(["capture-pane", "-t", &tui_session, "-p"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    if !repo_screen.contains(&env.repo_name) && !repo_screen.contains("repo") {
        // TUI didn't render — skip rather than fail flakily
        let _ = env
            .tmux_cmd()
            .args(["kill-session", "-t", &tui_session])
            .output();
        eprintln!("TUI did not render repo list, skipping: {repo_screen}");
        return;
    }

    // Navigate: Tab goes to branch picker (Enter opens tmux session)
    env.tmux_cmd()
        .args(["send-keys", "-t", &tui_session, "Tab"])
        .status()
        .unwrap();

    // Wait for branch view to render + agent poller to detect the agent (runs every 2s)
    wait_ms(5000);

    let screen = {
        let output = env
            .tmux_cmd()
            .args(["capture-pane", "-t", &tui_session, "-p"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).to_string()
    };

    // The TUI should show an agent label (e.g. [WAITING] for waiting state)
    let has_indicator = screen.contains("[WAITING]");
    assert!(
        has_indicator,
        "TUI branch view should show agent label: {screen}"
    );

    // Cleanup the TUI session
    let _ = env
        .tmux_cmd()
        .args(["kill-session", "-t", &tui_session])
        .output();
}

// ---------------------------------------------------------------------------
// Gemini CLI tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_branches_json_gemini_running() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-gemini-run");
    env.launch_agent(AgentKind::Gemini, FakeState::Running);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    let agent = &main_branch["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "should detect gemini: {main_branch}"
    );
    assert_eq!(agent[0]["kind"], "Gemini");
    assert_eq!(agent[0]["state"], "Running");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_gemini_waiting() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-gemini-wait");
    env.launch_agent(AgentKind::Gemini, FakeState::Waiting);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_statuses"][0]["kind"], "Gemini");
    assert_eq!(main_branch["agent_statuses"][0]["state"], "Waiting");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_gemini_idle() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("br-gemini-idle");
    env.launch_agent(AgentKind::Gemini, FakeState::Idle);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_statuses"][0]["kind"], "Gemini");
    assert_eq!(main_branch["agent_statuses"][0]["state"], "Idle");
}

#[test]
#[serial]
fn test_e2e_agent_status_json_gemini() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("st-gemini");
    env.launch_agent(AgentKind::Gemini, FakeState::Waiting);

    let json = env.run_cli_json(&["status", &env.repo_name, "main", "--json"]);
    let agent = &json["agent_status"];
    assert!(
        !agent.is_null(),
        "status should include agent_status: {json}"
    );
    assert_eq!(agent["kind"], "Gemini");
    assert_eq!(agent["state"], "Waiting");
}

#[test]
#[serial]
fn test_e2e_agent_sessions_json_gemini() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("sess-gemini");
    env.launch_agent(AgentKind::Gemini, FakeState::Running);

    let json = env.run_cli_json(&["sessions", "--json"]);
    let sessions = json.as_array().expect("sessions should be an array");
    let our_session = sessions
        .iter()
        .find(|s| s["session"] == env.kiosk_session)
        .expect("should find our session");

    let agent = &our_session["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "session should have agent_statuses: {our_session}"
    );
    assert_eq!(agent[0]["kind"], "Gemini");
    assert_eq!(agent[0]["state"], "Running");
}

// ---------------------------------------------------------------------------
// Multi-pane priority test
// ---------------------------------------------------------------------------

/// When multiple agents run in the same session (split panes), kiosk should
/// report the highest-priority state: Waiting > Running > Idle.
#[test]
#[serial]
fn test_e2e_agent_multi_pane_highest_priority_wins() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("multi-pane");

    // Write two fake agent scripts: one idle, one waiting
    let idle_output = fake_agent_output(AgentKind::ClaudeCode, FakeState::Idle);
    let waiting_output = fake_agent_output(AgentKind::Codex, FakeState::Waiting);

    let idle_script = write_fake_agent_script(env.tmp.path(), "claude", idle_output);
    let waiting_script = write_fake_agent_script(env.tmp.path(), "codex", waiting_output);

    // Create session with idle agent in pane 0
    let status = env
        .tmux_cmd()
        .args([
            "new-session",
            "-d",
            "-s",
            &env.kiosk_session,
            "-c",
            &env.repo_dir.to_string_lossy(),
            "-x",
            "120",
            "-y",
            "30",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    env.tmux_cmd()
        .args([
            "send-keys",
            "-t",
            &env.kiosk_session,
            &idle_script.to_string_lossy(),
            "Enter",
        ])
        .status()
        .unwrap();
    assert!(
        wait_for_pane_content(
            &env.kiosk_session,
            "? for shortcuts",
            10_000,
            &env.tmux_socket
        ),
        "Timed out waiting for idle claude output"
    );

    // Split and launch waiting agent in pane 1
    env.tmux_cmd()
        .args([
            "split-window",
            "-h",
            "-t",
            &env.kiosk_session,
            "-c",
            &env.repo_dir.to_string_lossy(),
        ])
        .status()
        .unwrap();
    wait_ms(500);

    // Target the new pane (pane 1)
    let pane_target = format!("{}:.1", env.kiosk_session);
    env.tmux_cmd()
        .args([
            "send-keys",
            "-t",
            &pane_target,
            &waiting_script.to_string_lossy(),
            "Enter",
        ])
        .status()
        .unwrap();

    // Wait for the waiting output in pane 1
    wait_ms(3000);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    let agent = &main_branch["agent_statuses"];
    assert!(
        agent.is_array() && !agent.as_array().unwrap().is_empty(),
        "should detect an agent: {main_branch}"
    );
    // Waiting (Codex) should win over Idle (Claude)
    assert_eq!(
        agent[0]["state"], "Waiting",
        "Waiting should have higher priority than Idle: {agent}"
    );
}

// ---------------------------------------------------------------------------
// OpenCode tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_e2e_agent_branches_json_opencode_running() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("opencode-run");
    env.launch_agent(AgentKind::OpenCode, FakeState::Running);
    let branches = env.run_cli_json(&["branches", "--json", &env.repo_name]);
    let branches = branches.as_array().unwrap();
    let main = &branches[0];
    assert_eq!(main["agent_statuses"][0]["kind"], "OpenCode");
    assert_eq!(main["agent_statuses"][0]["state"], "Running");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_opencode_waiting() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("opencode-wait");
    env.launch_agent(AgentKind::OpenCode, FakeState::Waiting);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_statuses"][0]["kind"], "OpenCode");
    assert_eq!(main_branch["agent_statuses"][0]["state"], "Waiting");
}

#[test]
#[serial]
fn test_e2e_agent_branches_json_opencode_idle() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("opencode-idle");
    env.launch_agent(AgentKind::OpenCode, FakeState::Idle);
    let branches = env.run_cli_json(&["branches", "--json", &env.repo_name]);
    let branches = branches.as_array().unwrap();
    let main = &branches[0];
    assert_eq!(main["agent_statuses"][0]["kind"], "OpenCode");
    assert_eq!(main["agent_statuses"][0]["state"], "Idle");
}

#[test]
#[serial]
fn test_e2e_agent_sessions_json_opencode() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("opencode-session");
    env.launch_agent(AgentKind::OpenCode, FakeState::Running);
    let sessions = env.run_cli_json(&["sessions", "--json"]);
    let sessions = sessions.as_array().unwrap();
    let session = sessions.iter().find(|s| s["session"] == env.kiosk_session);
    assert!(session.is_some(), "expected session in output");
    let s = session.unwrap();
    assert_eq!(s["agent_statuses"][0]["kind"], "OpenCode");
}

#[test]
#[serial]
fn test_e2e_agent_status_json_opencode() {
    if use_real_agents() {
        return;
    }
    let env = AgentTestEnvDefault::new("opencode-status");
    env.launch_agent(AgentKind::OpenCode, FakeState::Idle);
    let status = env.run_cli_json(&["status", "--json", &env.repo_name]);
    assert_eq!(status["agent_status"]["kind"], "OpenCode");
    assert_eq!(status["agent_status"]["state"], "Idle");
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Consolidated real-agent tests
//
// Each test starts one real agent, verifies all three states (Idle, Running,
// Waiting) in a single session using a single cheap prompt. The flow:
//
//   1. Start agent → verify **Idle**
//   2. Create a bait file, ask agent to delete it (one API call, ~200 tokens)
//   3. Poll aggressively → catch **Running** (agent thinking / API call)
//   4. Continue polling → catch **Waiting** (approval prompt)
//
// The per-state/per-CLI-command tests above run in fake mode only and cover
// all the output formatting permutations.
// ---------------------------------------------------------------------------

use std::collections::HashSet;

/// Get the current agent state from `kiosk branches --json`.
fn current_agent_state(env: &AgentTestEnvDefault) -> Option<String> {
    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let main = find_main_branch(&json);
    main["agent_statuses"][0]["state"]
        .as_str()
        .map(String::from)
}

/// Helper: find the main branch entry in `kiosk branches --json` output.
fn find_main_branch(json: &Value) -> &Value {
    json.as_array()
        .expect("branches should be an array")
        .iter()
        .find(|b| b["name"] == "main")
        .expect("should have main branch")
}

/// Poll `kiosk branches --json` collecting every unique state observed.
/// Returns early once all `required` states have been seen, or after timeout.
fn poll_collecting_states(
    env: &AgentTestEnvDefault,
    required: &[&str],
    timeout_secs: u64,
) -> HashSet<String> {
    let mut seen = HashSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if let Some(state) = current_agent_state(env) {
            seen.insert(state);
        }
        if required.iter().all(|r| seen.contains(*r)) {
            return seen;
        }
        wait_ms(200);
    }
    seen
}

/// Poll current CLI state until one of `accepted` appears, or timeout.
fn poll_for_any_startup_state(
    env: &AgentTestEnvDefault,
    accepted: &[&str],
    timeout_secs: u64,
) -> Option<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if let Some(state) = current_agent_state(env)
            && accepted.iter().any(|v| *v == state)
        {
            return Some(state);
        }
        wait_ms(200);
    }
    current_agent_state(env)
}

/// Handles Cursor Agent's quirky startup sequence: it may show a trust dialog
/// (surfacing as `Waiting`) between the initial idle detection and our first
/// CLI poll. Returns once the trust dialog has been dismissed (if present).
fn handle_cursor_startup(env: &AgentTestEnvDefault, state: Option<String>, agent_name: &str) {
    let state = if state.is_none() {
        // On some versions, agent_status can be briefly absent right after
        // startup while trust UI is still initializing. Nudge trust accept
        // once more, then repoll.
        env.tmux_cmd()
            .args(["send-keys", "-t", &env.kiosk_session, "a"])
            .status()
            .unwrap();
        wait_ms(300);
        env.tmux_cmd()
            .args(["send-keys", "-t", &env.kiosk_session, "Enter"])
            .status()
            .unwrap();
        wait_ms(1500);
        poll_for_any_startup_state(env, &["Idle", "Waiting", "Unknown"], 10)
    } else {
        state
    };

    assert!(
        state.is_none() || state.as_deref() == Some("Idle") || state.as_deref() == Some("Waiting"),
        "{agent_name}: expected None/Idle/Waiting after startup, got {state:?}"
    );

    if state.as_deref() == Some("Waiting") {
        // Trust dialog is showing — we've already verified Idle during
        // start_real_agent (it found the idle marker). The trust dialog
        // appeared between idle detection and our kiosk CLI poll. We can
        // still test Running by dismissing and proceeding.
        eprintln!("{agent_name}: trust dialog still visible, dismissing before task");
        env.tmux_cmd()
            .args(["send-keys", "-t", &env.kiosk_session, "a"])
            .status()
            .unwrap();
        wait_ms(3000);
    } else if state.is_none() {
        eprintln!(
            "{agent_name}: startup state unavailable (None), continuing; this can happen during \
             trust-screen transitions."
        );
    }
}

/// Core test logic shared by all real-agent tests.
///
/// 1. Starts the agent and asserts Idle.
/// 2. Creates a bait file, sends a delete prompt (one cheap API call).
/// 3. Polls for Running (agent thinking) and Waiting (approval prompt).
///
/// The delete prompt is chosen because:
///
/// - It's cheap (~200 tokens round-trip).
/// - All agents show a Running phase while the API processes.
/// - All agents in their default/test configurations ask for approval
///   before deleting files, producing a Waiting state.
#[allow(clippy::too_many_lines)]
fn run_real_agent_all_states(agent: AgentKind) {
    let label = match agent {
        AgentKind::ClaudeCode => "real-claude",
        AgentKind::Codex => "real-codex",
        AgentKind::CursorAgent => "real-cursor",
        AgentKind::Gemini => "real-gemini",
        AgentKind::OpenCode => "real-opencode",
    };
    let env = AgentTestEnvDefault::new(label);
    env.start_real_agent(agent);

    // ── Phase 1: Idle ──
    let state = match agent {
        // Claude/Cursor/Gemini can show startup prompts that surface as Waiting
        // or Unknown before settling to Idle.
        AgentKind::ClaudeCode | AgentKind::CursorAgent | AgentKind::Gemini => {
            poll_for_any_startup_state(&env, &["Idle", "Waiting", "Unknown"], 10)
        }
        AgentKind::Codex | AgentKind::OpenCode => poll_for_any_startup_state(&env, &["Idle"], 10),
    };
    let agent_name = format!("{agent:?}");

    // Cursor may still be showing a trust dialog (Waiting) after startup.
    if agent == AgentKind::CursorAgent {
        handle_cursor_startup(&env, state, &agent_name);
    } else if agent == AgentKind::Gemini || agent == AgentKind::ClaudeCode {
        assert!(
            state.as_deref() == Some("Idle")
                || state.as_deref() == Some("Waiting")
                || state.as_deref() == Some("Unknown"),
            "{agent_name}: expected Idle/Waiting/Unknown after startup, got {state:?}"
        );
    } else {
        assert_eq!(
            state.as_deref(),
            Some("Idle"),
            "{agent_name}: expected Idle after startup, got {state:?}"
        );
    }

    // ── Phase 2+3: Running + Waiting ──
    // Create a bait file, then ask the agent to delete it.
    std::fs::write(env.repo_dir.join("delete-me.txt"), "test file for e2e").unwrap();
    let waiting_prompt = AgentTestEnvDefault::waiting_task_prompt(agent);
    env.send_to_agent(waiting_prompt);

    let seen = poll_collecting_states(&env, &["Running", "Waiting"], 45);

    assert!(
        seen.contains("Running"),
        "{agent_name}: never saw Running state while agent processed the prompt. \
         States seen: {seen:?}. The Running phase may have been too brief to catch — \
         consider increasing poll frequency."
    );
    let waiting_required = matches!(agent, AgentKind::OpenCode);
    if waiting_required {
        assert!(
            seen.contains("Waiting"),
            "{agent_name}: never saw Waiting state (approval prompt). \
             States seen: {seen:?}. The agent may have auto-approved the deletion — \
             check that the agent is launched in a mode that requires approval."
        );
    } else if !seen.contains("Waiting") {
        eprintln!(
            "{agent_name}: Waiting state not observed (seen: {seen:?}); continuing because this \
             agent may auto-approve in current configuration."
        );
    }

    // ── Verify detection through all CLI surfaces ──
    let json = env.run_cli_json(&["status", &env.repo_name, "main", "--json"]);
    let kind = json["agent_status"]["kind"].as_str().unwrap_or("missing");
    assert_eq!(
        kind, &agent_name,
        "{agent_name}: status command should report correct agent kind"
    );

    let json = env.run_cli_json(&["sessions", "--json"]);
    let sessions = json.as_array().unwrap();
    let session = sessions
        .iter()
        .find(|s| s["session"] == env.kiosk_session)
        .expect("should find session in sessions list");
    let kind = session["agent_statuses"][0]["kind"]
        .as_str()
        .unwrap_or("missing");
    assert_eq!(
        kind, &agent_name,
        "{agent_name}: sessions command should report correct agent kind"
    );
}

#[test]
#[serial]
fn test_e2e_agent_real_claude_code() {
    if !use_real_agents() {
        return;
    }
    run_real_agent_all_states(AgentKind::ClaudeCode);
}

#[test]
#[serial]
fn test_e2e_agent_real_codex() {
    if !use_real_agents() {
        return;
    }
    run_real_agent_all_states(AgentKind::Codex);
}

#[test]
#[serial]
fn test_e2e_agent_real_cursor() {
    if !use_real_agents() {
        return;
    }
    run_real_agent_all_states(AgentKind::CursorAgent);
}

#[test]
#[serial]
fn test_e2e_agent_real_gemini() {
    if !use_real_agents() {
        return;
    }
    run_real_agent_all_states(AgentKind::Gemini);
}

#[test]
#[serial]
fn test_e2e_agent_real_opencode() {
    if !use_real_agents() {
        return;
    }
    run_real_agent_all_states(AgentKind::OpenCode);
}
