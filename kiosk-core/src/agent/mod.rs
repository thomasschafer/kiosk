use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents the kind of AI coding agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    CursorAgent,
    OpenCode,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::ClaudeCode => write!(f, "Claude Code"),
            AgentKind::Codex => write!(f, "Codex"),
            AgentKind::CursorAgent => write!(f, "Cursor"),
            AgentKind::OpenCode => write!(f, "OpenCode"),
        }
    }
}

/// Represents the current state of an AI coding agent.
///
/// Variants are ordered by attention priority (highest first): a Waiting agent
/// needs user action most urgently, an Idle agent may need a nudge, and a
/// Running agent is already doing work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    /// Agent is actively working (spinner, processing)
    Running,
    /// Agent needs user action (permission prompt, input prompt)
    Waiting,
    /// Agent is at prompt, not doing anything
    Idle,
    /// Terminal content not yet recognised as any known pattern
    Unknown,
}

impl AgentState {
    /// Attention priority: higher means the user should look at this agent first.
    fn attention_priority(self) -> u8 {
        match self {
            AgentState::Waiting => 3,
            AgentState::Idle => 2,
            AgentState::Running => 1,
            AgentState::Unknown => 0,
        }
    }
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentState::Running => write!(f, "Running"),
            AgentState::Waiting => write!(f, "Waiting"),
            AgentState::Idle => write!(f, "Idle"),
            AgentState::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Combined agent kind + state, attached to branches with detected agents
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatus {
    pub kind: AgentKind,
    pub state: AgentState,
}

pub mod detect;

/// Detect agent status for a tmux session by inspecting its panes.
/// Returns `None` if no agent is found in any pane. When multiple agents are
/// present, returns the one with the highest attention priority (Waiting >
/// Idle > Running) so the user sees the status that most needs their action.
pub fn detect_for_session(
    tmux: &(impl crate::tmux::TmuxProvider + ?Sized),
    session_name: &str,
) -> Option<AgentStatus> {
    let panes = tmux.list_panes_detailed(session_name);

    let mut best: Option<AgentStatus> = None;

    for pane in panes {
        let kind = detect::detect_agent_kind(&pane.command, None).or_else(|| {
            // Only walk the process tree for shell commands where an agent
            // might be running as a child. Skipping editors, TUI apps, etc.
            // avoids unnecessary /proc reads and pgrep calls every poll cycle.
            if may_host_agent(&pane.command) {
                get_child_process_args(pane.pid)
                    .as_deref()
                    .and_then(|args| detect::detect_agent_kind(&pane.command, Some(args)))
            } else {
                None
            }
        });

        if let Some(kind) = kind
            && let Some(content) = tmux.capture_pane_content(&pane.pane_id, 30)
        {
            let state = detect::detect_state(&content, kind);
            let status = AgentStatus { kind, state };
            if best
                .as_ref()
                .is_none_or(|b| status.state.attention_priority() > b.state.attention_priority())
            {
                best = Some(status);
            }
        }
    }

    best
}

/// Commands that may host an agent as a child process. We walk the process
/// tree for these to check if an agent binary is running underneath.
/// Includes shells (where users launch agents) and `node` (which hosts
/// Node.js-based agents like `OpenCode` and Cursor Agent).
const AGENT_HOST_COMMANDS: &[&str] = &[
    "bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "csh", "nu", "nushell", "pwsh", "node",
];

fn may_host_agent(command: &str) -> bool {
    let cmd_lower = command.to_lowercase();
    AGENT_HOST_COMMANDS.iter().any(|s| cmd_lower == *s)
}

/// Maximum depth when recursively walking child processes, to prevent
/// infinite loops in case of unexpected process tree cycles.
const MAX_CHILD_DEPTH: usize = 8;

/// Get command-line arguments of all descendant processes for a given PID.
/// Walks the process tree recursively (depth-first) up to [`MAX_CHILD_DEPTH`].
/// Portable across Linux (incl. WSL) and macOS.
fn get_child_process_args(pid: u32) -> Option<String> {
    let mut args = String::new();

    // Try /proc first (Linux, WSL)
    if get_child_args_procfs(pid, &mut args, 0) {
        if !args.is_empty() {
            return Some(args);
        }
    } else {
        // Fallback: use pgrep + ps (works on Linux and macOS)
        get_child_args_pgrep(pid, &mut args, 0);
        if !args.is_empty() {
            return Some(args);
        }
    }

    None
}

/// Recursively collect descendant command lines via `/proc`.
/// Returns `true` if `/proc` is available (even if no children found).
fn get_child_args_procfs(pid: u32, args: &mut String, depth: usize) -> bool {
    if depth >= MAX_CHILD_DEPTH {
        return true;
    }
    let children_path = format!("/proc/{pid}/task/{pid}/children");
    let Ok(children) = std::fs::read_to_string(&children_path) else {
        return false; // /proc not available
    };
    for child_pid_str in children.split_whitespace() {
        if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{child_pid_str}/cmdline")) {
            let readable = cmdline.replace('\0', " ");
            args.push_str(&readable);
            args.push('\n');
        }
        // Recurse into this child's children
        if let Ok(child_pid) = child_pid_str.parse::<u32>() {
            get_child_args_procfs(child_pid, args, depth + 1);
        }
    }
    true
}

/// Recursively collect descendant command lines via `pgrep` + `ps`.
fn get_child_args_pgrep(pid: u32, args: &mut String, depth: usize) {
    if depth >= MAX_CHILD_DEPTH {
        return;
    }
    let Ok(pgrep_output) = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
    else {
        return;
    };
    if !pgrep_output.status.success() {
        return;
    }

    let pgrep_str = String::from_utf8_lossy(&pgrep_output.stdout);
    let child_pids: Vec<&str> = pgrep_str.lines().filter(|s| !s.is_empty()).collect();
    if child_pids.is_empty() {
        return;
    }

    let mut ps_cmd = std::process::Command::new("ps");
    ps_cmd.args(["-o", "args="]);
    for cpid in &child_pids {
        ps_cmd.args(["-p", cpid]);
    }
    if let Ok(output) = ps_cmd.output()
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        if !text.trim().is_empty() {
            args.push_str(&text);
        }
    }

    // Recurse into each child
    for cpid_str in &child_pids {
        if let Ok(cpid) = cpid_str.parse::<u32>() {
            get_child_args_pgrep(cpid, args, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::mock::MockTmuxProvider;
    use crate::tmux::provider::PaneInfo;

    fn mock_with_agent(session: &str, command: &str, pane_content: &str) -> MockTmuxProvider {
        let mut tmux = MockTmuxProvider::default();
        let pane_id = "%0";
        tmux.pane_info.insert(
            session.to_string(),
            vec![PaneInfo {
                pane_id: pane_id.to_string(),
                command: command.to_string(),
                pid: 99999, // Fake PID — child process lookup will fail gracefully
            }],
        );
        tmux.pane_content
            .insert(pane_id.to_string(), pane_content.to_string());
        tmux
    }

    #[test]
    fn detect_claude_code_running() {
        let tmux = mock_with_agent("my-session", "claude", "⠋ Reading file src/main.rs");
        let status = detect_for_session(&tmux, "my-session").unwrap();
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn detect_claude_code_waiting() {
        let tmux = mock_with_agent(
            "my-session",
            "claude",
            "Allow write to src/main.rs?\n  Yes, allow\n  No, deny",
        );
        let status = detect_for_session(&tmux, "my-session").unwrap();
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Waiting);
    }

    #[test]
    fn detect_claude_code_idle() {
        let tmux = mock_with_agent("my-session", "claude", "❯ \n? for shortcuts");
        let status = detect_for_session(&tmux, "my-session").unwrap();
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Idle);
    }

    #[test]
    fn detect_codex_running() {
        let tmux = mock_with_agent(
            "codex-session",
            "codex",
            "⠋ Searching codebase\nesc to interrupt",
        );
        let status = detect_for_session(&tmux, "codex-session").unwrap();
        assert_eq!(status.kind, AgentKind::Codex);
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn detect_codex_waiting() {
        let tmux = mock_with_agent(
            "codex-session",
            "codex",
            "Would you like to run the following command?\n$ touch test.txt\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again (p)\n  3. No (esc)\n\n  Press enter to confirm or esc to cancel",
        );
        let status = detect_for_session(&tmux, "codex-session").unwrap();
        assert_eq!(status.kind, AgentKind::Codex);
        assert_eq!(status.state, AgentState::Waiting);
    }

    #[test]
    fn detect_cursor_agent_running() {
        // Can't mock child process args, so test state detection directly
        let state = detect::detect_state(
            "⠋ Editing file src/main.rs\nesc to interrupt",
            AgentKind::CursorAgent,
        );
        assert_eq!(state, AgentState::Running);
    }

    #[test]
    fn detect_cursor_agent_waiting() {
        let state = detect::detect_state(
            "Do you trust the contents of this directory?\n\n▶ [a] Trust this workspace\n  [w] Trust without MCP\n  [q] Quit\n\nUse arrow keys to navigate, Enter to select",
            AgentKind::CursorAgent,
        );
        assert_eq!(state, AgentState::Waiting);
    }

    #[test]
    fn no_agent_in_regular_shell() {
        let tmux = mock_with_agent("shell-session", "bash", "$ ls -la\ntotal 42");
        assert!(detect_for_session(&tmux, "shell-session").is_none());
    }

    #[test]
    fn no_panes_returns_none() {
        let tmux = MockTmuxProvider::default();
        assert!(detect_for_session(&tmux, "nonexistent").is_none());
    }

    #[test]
    fn agent_found_in_second_pane() {
        let mut tmux = MockTmuxProvider::default();
        let session = "multi-pane";
        tmux.pane_info.insert(
            session.to_string(),
            vec![
                PaneInfo {
                    pane_id: "%0".to_string(),
                    command: "bash".to_string(),
                    pid: 11111,
                },
                PaneInfo {
                    pane_id: "%1".to_string(),
                    command: "claude".to_string(),
                    pid: 22222,
                },
            ],
        );
        tmux.pane_content
            .insert("%0".to_string(), "$ vim file.txt".to_string());
        tmux.pane_content
            .insert("%1".to_string(), "Esc to interrupt".to_string());

        let status = detect_for_session(&tmux, session).unwrap();
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn agent_with_ansi_codes_in_output() {
        let tmux = mock_with_agent("ansi-session", "claude", "\x1B[32m⠹ Running tool\x1B[0m");
        let status = detect_for_session(&tmux, "ansi-session").unwrap();
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn pane_has_agent_command_with_empty_content() {
        // capture_pane_content returns Some("") — agent detected but state is Unknown
        let mut tmux = MockTmuxProvider::default();
        tmux.pane_info.insert(
            "empty-content".to_string(),
            vec![PaneInfo {
                pane_id: "%0".to_string(),
                command: "claude".to_string(),
                pid: 44444,
            }],
        );
        tmux.pane_content.insert("%0".to_string(), String::new());
        let status = detect_for_session(&tmux, "empty-content").unwrap();
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Unknown);
    }

    #[test]
    fn pane_has_agent_command_but_no_content() {
        let mut tmux = MockTmuxProvider::default();
        tmux.pane_info.insert(
            "empty-pane".to_string(),
            vec![PaneInfo {
                pane_id: "%0".to_string(),
                command: "claude".to_string(),
                pid: 33333,
            }],
        );
        // No pane_content entry → capture_pane_content returns None
        assert!(detect_for_session(&tmux, "empty-pane").is_none());
    }

    /// Helper: build a mock with multiple agent panes in the same session.
    fn mock_multi_agent(session: &str, agents: &[(&str, &str)]) -> MockTmuxProvider {
        let mut tmux = MockTmuxProvider::default();
        let panes: Vec<PaneInfo> = agents
            .iter()
            .enumerate()
            .map(|(i, (command, _))| PaneInfo {
                pane_id: format!("%{i}"),
                command: command.to_string(),
                pid: 90000 + u32::try_from(i).expect("test has fewer than u32::MAX agents"),
            })
            .collect();
        tmux.pane_info.insert(session.to_string(), panes);
        for (i, (_, content)) in agents.iter().enumerate() {
            tmux.pane_content
                .insert(format!("%{i}"), content.to_string());
        }
        tmux
    }

    #[test]
    fn multi_agent_waiting_beats_running() {
        let tmux = mock_multi_agent(
            "multi",
            &[
                ("claude", "⠋ Reading file src/main.rs"),
                ("claude", "Allow write?\n  Yes, allow\n  No, deny"),
            ],
        );
        let status = detect_for_session(&tmux, "multi").unwrap();
        assert_eq!(status.state, AgentState::Waiting);
    }

    #[test]
    fn multi_agent_waiting_beats_idle() {
        let tmux = mock_multi_agent(
            "multi",
            &[
                ("claude", "❯ \n? for shortcuts"),
                ("claude", "Allow write?\n  Yes, allow\n  No, deny"),
            ],
        );
        let status = detect_for_session(&tmux, "multi").unwrap();
        assert_eq!(status.state, AgentState::Waiting);
    }

    #[test]
    fn multi_agent_idle_beats_running() {
        let tmux = mock_multi_agent(
            "multi",
            &[
                ("claude", "⠋ Reading file src/main.rs"),
                ("claude", "❯ \n? for shortcuts"),
            ],
        );
        let status = detect_for_session(&tmux, "multi").unwrap();
        assert_eq!(status.state, AgentState::Idle);
    }

    #[test]
    fn multi_agent_across_windows() {
        let mut tmux = MockTmuxProvider::default();
        let session = "multi-win";
        tmux.pane_info.insert(
            session.to_string(),
            vec![
                PaneInfo {
                    pane_id: "%10".to_string(),
                    command: "claude".to_string(),
                    pid: 80001,
                },
                PaneInfo {
                    pane_id: "%11".to_string(),
                    command: "claude".to_string(),
                    pid: 80002,
                },
            ],
        );
        tmux.pane_content
            .insert("%10".to_string(), "⠋ Reading file".to_string());
        tmux.pane_content
            .insert("%11".to_string(), "Allow write?\n  Yes, allow".to_string());

        let status = detect_for_session(&tmux, session).unwrap();
        assert_eq!(status.state, AgentState::Waiting);
    }

    #[test]
    fn may_host_agent_matches_common_shells() {
        assert!(super::may_host_agent("bash"));
        assert!(super::may_host_agent("zsh"));
        assert!(super::may_host_agent("fish"));
        assert!(super::may_host_agent("sh"));
        assert!(super::may_host_agent("dash"));
        assert!(super::may_host_agent("nu"));
        assert!(super::may_host_agent("nushell"));
    }

    #[test]
    fn may_host_agent_rejects_non_shells() {
        assert!(!super::may_host_agent("vim"));
        assert!(!super::may_host_agent("hx"));

        assert!(!super::may_host_agent("python3"));
        assert!(!super::may_host_agent("cargo"));
        assert!(!super::may_host_agent("claude"));
        assert!(!super::may_host_agent("codex"));
    }

    #[test]
    fn may_host_agent_case_insensitive() {
        assert!(super::may_host_agent("Bash"));
        assert!(super::may_host_agent("ZSH"));
        assert!(super::may_host_agent("Fish"));
    }

    #[test]
    fn attention_priority_ordering() {
        // Waiting > Idle > Running > Unknown
        assert!(AgentState::Waiting.attention_priority() > AgentState::Idle.attention_priority());
        assert!(AgentState::Idle.attention_priority() > AgentState::Running.attention_priority());
        assert!(
            AgentState::Running.attention_priority() > AgentState::Unknown.attention_priority()
        );
    }

    #[test]
    fn multi_agent_running_beats_unknown() {
        // Running should now beat Unknown (they were equal before)
        let tmux = mock_multi_agent(
            "multi",
            &[
                ("claude", ""),                           // Unknown (empty content)
                ("claude", "⠋ Reading file src/main.rs"), // Running
            ],
        );
        let status = detect_for_session(&tmux, "multi").unwrap();
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn child_process_skipped_for_non_shell() {
        // When pane command is "hx" (not a shell), child process walking
        // should be skipped entirely — no agent should be detected even if
        // a child process would match. We test this indirectly: "hx" with
        // no agent content should return None.
        let tmux = mock_with_agent("editor-session", "hx", "normal mode");
        assert!(detect_for_session(&tmux, "editor-session").is_none());
    }

    #[test]
    fn child_process_checked_for_shell() {
        // When pane command is a shell like "bash", detection should still
        // fall through to child process checking. Since we can't mock /proc,
        // verify that a shell with no agent content and no children returns None.
        let tmux = mock_with_agent("shell-session", "bash", "$ ls -la");
        assert!(detect_for_session(&tmux, "shell-session").is_none());
    }

    #[test]
    fn detect_opencode_running() {
        let tmux = mock_with_agent(
            "oc-session",
            "node",
            "⬝■■■■■■⬝  esc interrupt  ctrl+t variants  tab agents  ctrl+p commands",
        );
        // node pane won't match directly — needs child process.
        // Since we can't mock /proc, test state detection directly:
        let state = detect::detect_state(
            "⬝■■■■■■⬝  esc interrupt  ctrl+t variants  tab agents  ctrl+p commands",
            AgentKind::OpenCode,
        );
        assert_eq!(state, AgentState::Running);
    }

    #[test]
    fn detect_opencode_idle() {
        let state = detect::detect_state(
            "  ┃  Build  GPT-5.3 Codex OpenAI\n  ╹▀▀▀\n                ctrl+t variants  tab agents  ctrl+p commands",
            AgentKind::OpenCode,
        );
        assert_eq!(state, AgentState::Idle);
    }

    #[test]
    fn detect_opencode_via_command_name() {
        let tmux = mock_with_agent(
            "oc-session",
            "opencode",
            "  ctrl+t variants  tab agents  ctrl+p commands",
        );
        let status = detect_for_session(&tmux, "oc-session").unwrap();
        assert_eq!(status.kind, AgentKind::OpenCode);
        assert_eq!(status.state, AgentState::Idle);
    }

    #[test]
    fn may_host_agent_includes_node() {
        assert!(super::may_host_agent("node"));
        assert!(super::may_host_agent("Node"));
    }
}
