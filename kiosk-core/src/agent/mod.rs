use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents the kind of AI coding agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Unknown,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::ClaudeCode => write!(f, "Claude Code"),
            AgentKind::Codex => write!(f, "Codex"),
            AgentKind::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Represents the current state of an AI coding agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    /// Agent is actively working (spinner, processing)
    Running,
    /// Agent needs user action (permission prompt, input prompt)
    Waiting,
    /// Agent is at prompt, not doing anything
    Idle,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentState::Running => write!(f, "Running"),
            AgentState::Waiting => write!(f, "Waiting"),
            AgentState::Idle => write!(f, "Idle"),
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
/// Returns `None` if no agent is found in any pane.
pub fn detect_for_session(
    tmux: &(impl crate::tmux::TmuxProvider + ?Sized),
    session_name: &str,
) -> Option<AgentStatus> {
    let panes = tmux.list_panes_detailed(session_name);

    for pane in panes {
        let mut kind = detect::detect_agent_kind(&pane.command, None);

        if kind == AgentKind::Unknown
            && let Some(ref args) = get_child_process_args(pane.pid)
        {
            kind = detect::detect_agent_kind(&pane.command, Some(args));
        }

        if kind != AgentKind::Unknown
            && let Some(content) = tmux.capture_pane_by_index(session_name, pane.pane_index, 30)
        {
            let state = detect::detect_state(&content, kind);
            return Some(AgentStatus { kind, state });
        }
    }

    None
}

/// Get command-line arguments of child processes for a given PID.
/// Portable across Linux (incl. WSL) and macOS.
fn get_child_process_args(pid: u32) -> Option<String> {
    // Try /proc first (Linux, WSL) — children file contains space-separated child PIDs
    if let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) {
        let mut args = String::new();
        for child_pid in children.split_whitespace() {
            if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{child_pid}/cmdline")) {
                let readable = cmdline.replace('\0', " ");
                args.push_str(&readable);
                args.push('\n');
            }
        }
        if !args.is_empty() {
            return Some(args);
        }
    }

    // Fallback: use pgrep + ps (works on Linux and macOS)
    let pgrep_output = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .ok()?;

    if !pgrep_output.status.success() {
        return None;
    }

    let pgrep_str = String::from_utf8_lossy(&pgrep_output.stdout).to_string();
    let child_pids: Vec<&str> = pgrep_str.lines().filter(|s| !s.is_empty()).collect();

    if child_pids.is_empty() {
        return None;
    }

    let mut ps_cmd = std::process::Command::new("ps");
    ps_cmd.args(["-o", "args="]);
    for cpid in &child_pids {
        ps_cmd.args(["-p", cpid]);
    }
    let output = ps_cmd.output().ok()?;

    if output.status.success() {
        let args = String::from_utf8_lossy(&output.stdout).to_string();
        if !args.trim().is_empty() {
            return Some(args);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::mock::MockTmuxProvider;
    use crate::tmux::provider::PaneInfo;

    fn mock_with_agent(session: &str, command: &str, pane_content: &str) -> MockTmuxProvider {
        let mut tmux = MockTmuxProvider::default();
        tmux.pane_info.insert(
            session.to_string(),
            vec![PaneInfo {
                pane_index: 0,
                command: command.to_string(),
                pid: 99999, // Fake PID — child process lookup will fail gracefully
            }],
        );
        tmux.pane_content
            .insert((session.to_string(), 0), pane_content.to_string());
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
        let tmux = mock_with_agent("my-session", "claude", "$ ");
        let status = detect_for_session(&tmux, "my-session").unwrap();
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Idle);
    }

    #[test]
    fn detect_codex_running() {
        let tmux = mock_with_agent("codex-session", "codex", "working on your request...");
        let status = detect_for_session(&tmux, "codex-session").unwrap();
        assert_eq!(status.kind, AgentKind::Codex);
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn detect_codex_waiting() {
        let tmux = mock_with_agent("codex-session", "codex", "Do you approve this? [y/n]");
        let status = detect_for_session(&tmux, "codex-session").unwrap();
        assert_eq!(status.kind, AgentKind::Codex);
        assert_eq!(status.state, AgentState::Waiting);
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
                    pane_index: 0,
                    command: "bash".to_string(),
                    pid: 11111,
                },
                PaneInfo {
                    pane_index: 1,
                    command: "claude".to_string(),
                    pid: 22222,
                },
            ],
        );
        tmux.pane_content
            .insert((session.to_string(), 0), "$ vim file.txt".to_string());
        tmux.pane_content.insert(
            (session.to_string(), 1),
            "Esc to interrupt".to_string(),
        );

        let status = detect_for_session(&tmux, session).unwrap();
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn agent_with_ansi_codes_in_output() {
        let tmux = mock_with_agent(
            "ansi-session",
            "claude",
            "\x1B[32m⠹ Running tool\x1B[0m",
        );
        let status = detect_for_session(&tmux, "ansi-session").unwrap();
        assert_eq!(status.state, AgentState::Running);
    }

    #[test]
    fn pane_has_agent_command_but_no_content() {
        let mut tmux = MockTmuxProvider::default();
        tmux.pane_info.insert(
            "empty-pane".to_string(),
            vec![PaneInfo {
                pane_index: 0,
                command: "claude".to_string(),
                pid: 33333,
            }],
        );
        // No pane_content entry → capture_pane_by_index returns None
        assert!(detect_for_session(&tmux, "empty-pane").is_none());
    }
}
