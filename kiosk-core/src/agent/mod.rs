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
}

impl AgentState {
    /// Attention priority: higher means the user should look at this agent first.
    fn attention_priority(self) -> u8 {
        match self {
            AgentState::Waiting => 2,
            AgentState::Idle => 1,
            AgentState::Running => 0,
        }
    }
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
        let mut kind = detect::detect_agent_kind(&pane.command, None);

        if kind == AgentKind::Unknown
            && let Some(ref args) = get_child_process_args(pane.pid)
        {
            kind = detect::detect_agent_kind(&pane.command, Some(args));
        }

        if kind != AgentKind::Unknown
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
                pid: 90000 + i as u32,
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
                ("claude", "$ "),
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
            &[("claude", "⠋ Reading file src/main.rs"), ("claude", "$ ")],
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
}
