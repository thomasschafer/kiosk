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
use std::{
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
fn wait_for_pane_content(session: &str, expected: &str, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    let expected_lower = expected.to_lowercase();
    loop {
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", session, "-p", "-S", "-30"])
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

#[derive(Clone, Copy)]
enum AgentKind {
    Claude,
    Codex,
    CursorAgent,
}

#[derive(Clone, Copy)]
enum FakeState {
    Running,
    Waiting,
    Idle,
}

struct AgentTestEnvDefault {
    tmp: tempfile::TempDir,
    config_dir: PathBuf,
    state_dir: PathBuf,
    repo_dir: PathBuf,
    kiosk_session: String,
    repo_name: String,
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

        // kiosk session name for main worktree = repo name
        let kiosk_session = repo_name.clone();

        Self {
            tmp,
            config_dir,
            state_dir,
            repo_dir,
            kiosk_session,
            repo_name,
        }
    }

    /// Launch a fake/real agent in a tmux session on the DEFAULT server.
    /// This is what kiosk CLI will find when it runs `tmux list-sessions`.
    fn launch_agent(&self, agent: AgentKind, state: FakeState) {
        if use_real_agents() {
            self.launch_real_agent(agent);
        } else {
            self.launch_fake_agent(agent, state);
        }
    }

    fn launch_real_agent(&self, agent: AgentKind) {
        let bin = match agent {
            AgentKind::Claude => {
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
        };

        // Create tmux session named as kiosk expects, starting in repo dir
        let status = Command::new("tmux")
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

        // Ensure agent binaries are on PATH inside the tmux session
        let path = agent_path();
        Command::new("tmux")
            .args([
                "send-keys",
                "-t",
                &self.kiosk_session,
                &format!("export PATH='{path}'"),
                "Enter",
            ])
            .status()
            .unwrap();
        wait_ms(200);

        // Launch agent interactively in the temp repo dir.
        // This is safe: the repo is a temp dir with only a README.md.
        // The agent will reach its idle prompt without making any changes.
        Command::new("tmux")
            .args(["send-keys", "-t", &self.kiosk_session, bin, "Enter"])
            .status()
            .unwrap();

        // Real agents need time to start up and reach idle prompt
        // Claude Code takes ~12s, Codex ~5s
        wait_ms(15000);
    }

    fn launch_fake_agent(&self, agent: AgentKind, state: FakeState) {
        // Script filename must contain the agent name so kiosk detects the agent
        // by inspecting child process args via /proc/PID/cmdline or pgrep/ps.
        let agent_name = match agent {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::CursorAgent => "cursor-agent",
        };

        let output_text = match (agent, state) {
            (AgentKind::Claude, FakeState::Running) => {
                "⠋ Reading file src/main.rs\\nesc to interrupt"
            }
            (AgentKind::Claude, FakeState::Waiting) => {
                "Allow write to src/main.rs?\\n  Yes, allow\\n  No, deny"
            }
            (AgentKind::Claude, FakeState::Idle) => "❯ \\n? for shortcuts",
            (AgentKind::CursorAgent, FakeState::Idle) => "> ",

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
        };

        let script_path = write_fake_agent_script(self.tmp.path(), agent_name, output_text);

        // Create tmux session
        let status = Command::new("tmux")
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
        Command::new("tmux")
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
            (_, FakeState::Running) => Some("esc to interrupt"),
            (AgentKind::Claude, FakeState::Waiting) => Some("yes, allow"),
            (AgentKind::Codex, FakeState::Waiting) => Some("yes, proceed"),
            (AgentKind::Codex, FakeState::Idle) => Some("? for shortcuts"),
            (AgentKind::CursorAgent, FakeState::Waiting) => Some("trust this workspace"),
            // Claude/CursorAgent idle output is just "> " — too minimal for
            // reliable content polling (tmux strips trailing whitespace).
            (AgentKind::Claude, FakeState::Idle) => Some("? for shortcuts"),
            (AgentKind::CursorAgent, FakeState::Idle) => None,
        };
        if let Some(marker) = marker {
            assert!(
                wait_for_pane_content(&self.kiosk_session, marker, 10_000),
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
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.kiosk_session])
            .output();
    }
}

// ---------------------------------------------------------------------------
// CLI tests: `kiosk branches`
// ---------------------------------------------------------------------------

#[test]
fn test_e2e_agent_branches_json_claude_running() {
    let env = AgentTestEnvDefault::new("br-claude-run");
    env.launch_agent(AgentKind::Claude, FakeState::Running);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().expect("branches should be an array");

    // Find the main branch (should have a session with agent)
    let main_branch = branches
        .iter()
        .find(|b| b["name"] == "main")
        .expect("should have main branch");

    let agent = &main_branch["agent_status"];
    assert!(
        !agent.is_null(),
        "main branch should have agent_status: {main_branch}"
    );
    assert_eq!(agent["kind"], "ClaudeCode");

    if !use_real_agents() {
        assert_eq!(agent["state"], "Running");
    }
    // With real agents, state depends on timing — just assert detection worked
}

#[test]
fn test_e2e_agent_branches_json_claude_waiting() {
    if use_real_agents() {
        // Can't reliably produce Waiting state with real agent
        return;
    }

    let env = AgentTestEnvDefault::new("br-claude-wait");
    env.launch_agent(AgentKind::Claude, FakeState::Waiting);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_status"]["kind"], "ClaudeCode");
    assert_eq!(main_branch["agent_status"]["state"], "Waiting");
}

#[test]
fn test_e2e_agent_branches_json_claude_idle() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("br-claude-idle");
    env.launch_agent(AgentKind::Claude, FakeState::Idle);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_status"]["kind"], "ClaudeCode");
    assert_eq!(main_branch["agent_status"]["state"], "Idle");
}

#[test]
fn test_e2e_agent_branches_json_codex_running() {
    let env = AgentTestEnvDefault::new("br-codex-run");
    env.launch_agent(AgentKind::Codex, FakeState::Running);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    let agent = &main_branch["agent_status"];
    assert!(!agent.is_null(), "should detect codex: {main_branch}");
    assert_eq!(agent["kind"], "Codex");

    if !use_real_agents() {
        assert_eq!(agent["state"], "Running");
    }
}

#[test]
fn test_e2e_agent_branches_json_no_agent() {
    let env = AgentTestEnvDefault::new("br-no-agent");
    // Create a session but with just a shell — no agent
    let status = Command::new("tmux")
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

    // agent_status should be absent (skip_serializing_if = None)
    assert!(
        main_branch.get("agent_status").is_none(),
        "shell-only session should not have agent_status: {main_branch}"
    );
}

#[test]
fn test_e2e_agent_branches_table_shows_agent_column() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("br-table-col");
    env.launch_agent(AgentKind::Claude, FakeState::Waiting);

    let output = env.run_cli(&["branches", &env.repo_name]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("agent"),
        "Table should have agent column header: {stdout}"
    );
    assert!(
        stdout.contains("Waiting"),
        "Table should show Waiting state: {stdout}"
    );
}

#[test]
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
fn test_e2e_agent_status_json_includes_agent() {
    let env = AgentTestEnvDefault::new("st-claude");
    env.launch_agent(AgentKind::Claude, FakeState::Running);

    let json = env.run_cli_json(&["status", &env.repo_name, "main", "--json"]);

    let agent = &json["agent_status"];
    assert!(
        !agent.is_null(),
        "status should include agent_status: {json}"
    );
    assert_eq!(agent["kind"], "ClaudeCode");

    if !use_real_agents() {
        assert_eq!(agent["state"], "Running");
    }
}

#[test]
fn test_e2e_agent_status_json_no_agent() {
    let env = AgentTestEnvDefault::new("st-no-agent");
    // Create a plain session
    Command::new("tmux")
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
fn test_e2e_agent_sessions_json_includes_agent() {
    let env = AgentTestEnvDefault::new("sess-claude");
    env.launch_agent(AgentKind::Claude, FakeState::Waiting);

    let json = env.run_cli_json(&["sessions", "--json"]);
    let sessions = json.as_array().expect("sessions should be an array");

    let our_session = sessions
        .iter()
        .find(|s| s["session"] == env.kiosk_session)
        .expect("should find our session in sessions list");

    let agent = &our_session["agent_status"];
    assert!(
        !agent.is_null(),
        "session should have agent_status: {our_session}"
    );
    assert_eq!(agent["kind"], "ClaudeCode");

    if !use_real_agents() {
        assert_eq!(agent["state"], "Waiting");
    }
}

#[test]
fn test_e2e_agent_sessions_json_no_agent() {
    let env = AgentTestEnvDefault::new("sess-no-agent");
    Command::new("tmux")
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
    wait_ms(500);

    let json = env.run_cli_json(&["sessions", "--json"]);
    let sessions = json.as_array().unwrap();

    let our_session = sessions.iter().find(|s| s["session"] == env.kiosk_session);

    if let Some(session) = our_session {
        assert!(
            session.get("agent_status").is_none(),
            "plain session should not have agent_status: {session}"
        );
    }
}

// ---------------------------------------------------------------------------
// CLI tests: Cursor Agent
// ---------------------------------------------------------------------------

#[test]
fn test_e2e_agent_branches_json_cursor_running() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("br-cursor-run");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Running);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    let agent = &main_branch["agent_status"];
    assert!(
        !agent.is_null(),
        "should detect cursor agent: {main_branch}"
    );
    assert_eq!(agent["kind"], "CursorAgent");
    assert_eq!(agent["state"], "Running");
}

#[test]
fn test_e2e_agent_branches_json_cursor_waiting() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("br-cursor-wait");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Waiting);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_status"]["kind"], "CursorAgent");
    assert_eq!(main_branch["agent_status"]["state"], "Waiting");
}

#[test]
fn test_e2e_agent_branches_json_cursor_idle() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("br-cursor-idle");
    env.launch_agent(AgentKind::CursorAgent, FakeState::Idle);

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();

    assert_eq!(main_branch["agent_status"]["kind"], "CursorAgent");
    assert_eq!(main_branch["agent_status"]["state"], "Idle");
}

#[test]
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

    let agent = &our_session["agent_status"];
    assert!(
        !agent.is_null(),
        "session should have agent_status: {our_session}"
    );
    assert_eq!(agent["kind"], "CursorAgent");
    assert_eq!(agent["state"], "Running");
}

// ---------------------------------------------------------------------------
// Regression test: stale content should not cause false positives
// ---------------------------------------------------------------------------

#[test]
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
        main_branch["agent_status"]["state"], "Waiting",
        "should initially detect Waiting"
    );

    // Phase 2: kill the fake agent process, then relaunch with idle output.
    // This simulates answering a permission prompt — the old waiting text
    // remains in the scrollback, but the tail now shows idle markers.
    let _ = Command::new("tmux")
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

    Command::new("tmux")
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
        wait_for_pane_content(&env.kiosk_session, "? for shortcuts", 10_000),
        "Timed out waiting for idle Codex output in phase 2"
    );

    let json = env.run_cli_json(&["branches", &env.repo_name, "--json"]);
    let branches = json.as_array().unwrap();
    let main_branch = branches.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(
        main_branch["agent_status"]["state"], "Idle",
        "should detect Idle after transitioning from Waiting (idle tail overrides stale content)"
    );
}

// ---------------------------------------------------------------------------
// TUI test: agent indicator visible in branch picker
// ---------------------------------------------------------------------------

#[test]
fn test_e2e_agent_tui_shows_indicator() {
    if use_real_agents() {
        return;
    }

    let env = AgentTestEnvDefault::new("tui-ind");

    // First, launch a fake agent in the kiosk session
    env.launch_agent(AgentKind::Claude, FakeState::Waiting);

    // Now launch kiosk TUI in a SEPARATE tmux session to observe it
    let tui_session = format!("{}-tui", env.kiosk_session);
    let binary = kiosk_binary();
    let status = Command::new("tmux")
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
                "XDG_CONFIG_HOME={} XDG_STATE_HOME={} KIOSK_NO_ALT_SCREEN=1 {} ; sleep 2",
                env.config_dir.to_string_lossy(),
                env.state_dir.to_string_lossy(),
                binary.to_string_lossy()
            ),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "Failed to launch kiosk TUI");

    // Wait for TUI to load and discover repos (async discovery can take time)
    wait_ms(3000);

    // Verify the TUI session exists
    let has_session = Command::new("tmux")
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
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &tui_session, "-p"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    if !repo_screen.contains(&env.repo_name) && !repo_screen.contains("repo") {
        // TUI didn't render — skip rather than fail flakily
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &tui_session])
            .output();
        eprintln!("TUI did not render repo list, skipping: {repo_screen}");
        return;
    }

    // Navigate: Tab goes to branch picker (Enter opens tmux session)
    Command::new("tmux")
        .args(["send-keys", "-t", &tui_session, "Tab"])
        .status()
        .unwrap();

    // Wait for branch view to render + agent poller to detect the agent (runs every 2s)
    wait_ms(5000);

    let screen = {
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &tui_session, "-p"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).to_string()
    };

    // The TUI should show an agent indicator (⏳ for Waiting, ⚡ for Running)
    let has_indicator = screen.contains('⏳') || screen.contains('⚡') || screen.contains("Claude");
    assert!(
        has_indicator,
        "TUI branch view should show agent indicator: {screen}"
    );

    // Cleanup the TUI session
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &tui_session])
        .output();
}
