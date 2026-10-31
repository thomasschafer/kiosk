//! Agent status guessed from what a pane shows and runs, for agents that do not report
//! their own status over OSC 7501. Everything here is specific to the agents it knows and
//! their current screens, so it can go once they all report status themselves.

mod detect;

use super::{AgentState, PaneStatus, Source, StatusSource};
use crate::{
    args,
    tmux::{Pane, Session, Tmux},
};

/// The agents the screen patterns know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    CursorAgent,
    OpenCode,
    Gemini,
}

impl AgentKind {
    /// The name shown beside the agent's status.
    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::CursorAgent => "cursor",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
        }
    }
}

/// What the screen patterns can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeuristicState {
    Running,
    Waiting,
    Idle,
    /// The screen matched no known pattern.
    Unknown,
}

/// Lines of a pane's screen the patterns look at.
const CAPTURE_LINES: u32 = 30;

/// Commands that may host an agent as a child process: shells, where users launch agents,
/// and `node`, which runs the Node.js-based agents.
const AGENT_HOST_COMMANDS: &[&str] = &[
    "bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "csh", "nu", "nushell", "pwsh", "node",
];

/// How recently, in seconds, the session must have been active to guess that an agent on
/// an unrecognised screen is running. Kept tight, as scrolling or resizing is activity
/// too.
const ACTIVITY_RECENCY_SECS: u64 = 3;

/// Bounds the walk of a pane's process tree, in case of cycles.
const MAX_CHILD_DEPTH: usize = 8;

pub struct Heuristics<'a> {
    tmux: &'a dyn Tmux,
}

impl<'a> Heuristics<'a> {
    pub fn new(tmux: &'a dyn Tmux) -> Self {
        Self { tmux }
    }

    fn capture(&self, pane: &Pane) -> Option<String> {
        self.tmux
            .run(args![
                "capture-pane",
                "-p",
                "-J",
                "-t",
                pane.id.as_str(),
                "-S",
                format!("-{CAPTURE_LINES}")
            ])
            .ok()
    }
}

/// How the agent was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindSource {
    PaneCommand,
    ChildProcess,
    PaneTitle,
    Content,
}

impl StatusSource for Heuristics<'_> {
    fn status(&self, pane: &Pane, session: &Session) -> Option<PaneStatus> {
        if pane.dead {
            return None;
        }
        let command = pane.current_command.trim();
        let wrapper = looks_like_version_command(command);
        let host = may_host_agent(command);
        let mut recognised =
            detect::detect_agent_kind(command, None).map(|kind| (kind, KindSource::PaneCommand));
        if recognised.is_none() && (host || wrapper) {
            recognised = child_process_args(pane.pid)
                .as_deref()
                .and_then(|args| detect::detect_agent_kind(command, Some(args)))
                .map(|kind| (kind, KindSource::ChildProcess));
        }
        if recognised.is_none() && wrapper {
            recognised = detect::detect_agent_kind_from_title(&pane.title)
                .map(|kind| (kind, KindSource::PaneTitle));
        }
        let mut content = None;
        // Process lookups can miss agents under a host command, so fall back to their
        // screens' markers.
        if recognised.is_none() && (host || wrapper) {
            content = self.capture(pane);
            recognised = content
                .as_deref()
                .and_then(|content| {
                    if wrapper {
                        detect::detect_agent_kind_from_wrapper_content(content)
                    } else {
                        detect::detect_agent_kind_from_content(content)
                    }
                })
                .map(|kind| (kind, KindSource::Content));
        }
        let (kind, source) = recognised?;
        let content = content.or_else(|| self.capture(pane))?;
        let mut state = detect::detect_state(&content, kind);
        let mut rule = detect::detect_state_rule(&content, kind, state);
        // Codex sessions often hold other busy panes, so session activity says nothing
        // about Codex itself. Kinds guessed from a title or old screen text could be an
        // agent that has exited, which activity must not turn into a running one.
        if state == HeuristicState::Unknown
            && kind != AgentKind::Codex
            && !content.trim().is_empty()
            && matches!(source, KindSource::PaneCommand | KindSource::ChildProcess)
            && recently_active(session.activity)
        {
            state = HeuristicState::Running;
            rule = "activity.recent";
        }
        // An unknown screen in a pane recognised only by its screen is most likely an old
        // transcript, not an agent.
        if state == HeuristicState::Unknown && source == KindSource::Content {
            return None;
        }
        Some(PaneStatus {
            agent: Some(kind.name().into()),
            state: match state {
                HeuristicState::Running => AgentState::Running,
                HeuristicState::Waiting => AgentState::Waiting,
                HeuristicState::Idle => AgentState::Idle,
                HeuristicState::Unknown => AgentState::Unknown,
            },
            detail: None,
            source: Source::Heuristics,
            rule: Some(rule),
        })
    }
}

fn recently_active(activity: u64) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    now.saturating_sub(activity) <= ACTIVITY_RECENCY_SECS
}

fn may_host_agent(command: &str) -> bool {
    AGENT_HOST_COMMANDS
        .iter()
        .any(|host| command.eq_ignore_ascii_case(host))
}

/// Some agent wrappers show in tmux as a bare version such as `2.1.63`, which says nothing
/// about the agent, so their title and screen are looked at instead.
fn looks_like_version_command(command: &str) -> bool {
    let parts: Vec<&str> = command.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The command lines of every process below `pid`, one per line.
fn child_process_args(pid: u32) -> Option<String> {
    let mut args = String::new();
    if !child_args_procfs(pid, &mut args, 0) {
        child_args_pgrep(pid, &mut args, 0);
    }
    (!args.is_empty()).then_some(args)
}

/// Walks the tree through `/proc` (Linux and WSL). Returns whether `/proc` is available.
fn child_args_procfs(pid: u32, args: &mut String, depth: usize) -> bool {
    if depth >= MAX_CHILD_DEPTH {
        return true;
    }
    let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) else {
        return false;
    };
    for child in children.split_whitespace() {
        if let Ok(raw) = std::fs::read(format!("/proc/{child}/cmdline")) {
            args.push_str(&String::from_utf8_lossy(&raw).replace('\0', " "));
            args.push('\n');
        }
        if let Ok(child) = child.parse() {
            child_args_procfs(child, args, depth + 1);
        }
    }
    true
}

/// Walks the tree with `pgrep` and `ps`, for systems without `/proc` such as macOS.
fn child_args_pgrep(pid: u32, args: &mut String, depth: usize) {
    if depth >= MAX_CHILD_DEPTH {
        return;
    }
    let Ok(output) = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let children: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if children.is_empty() {
        return;
    }
    let mut ps = std::process::Command::new("ps");
    ps.args(["-o", "args="]);
    for child in &children {
        ps.args(["-p", child]);
    }
    if let Ok(output) = ps.output()
        && output.status.success()
    {
        args.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    for child in &children {
        if let Ok(child) = child.parse() {
            child_args_pgrep(child, args, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{PaneId, PanePipe, SessionId, mock::MockTmux};

    /// A process id with no children, so process-tree lookups find nothing.
    const NO_CHILDREN: u32 = 999_999;

    fn detect(
        command: &str,
        title: &str,
        screen: Option<&str>,
        activity: u64,
    ) -> Option<PaneStatus> {
        let tmux = MockTmux::default();
        if let Some(screen) = screen {
            tmux.respond(Ok(screen.into()));
        }
        let pane = Pane {
            id: PaneId::parse("%0").unwrap(),
            pid: NO_CHILDREN,
            current_command: command.into(),
            title: title.into(),
            dead: false,
            pipe: PanePipe::None,
            program_status: None,
        };
        let session = Session {
            id: SessionId::parse("$0").unwrap(),
            name: "s".into(),
            activity,
            place: None,
            panes: vec![pane.clone()],
        };
        Heuristics::new(&tmux).status(&pane, &session)
    }

    fn state(command: &str, screen: &str) -> Option<(String, AgentState)> {
        detect(command, "", Some(screen), 0).map(|status| (status.agent.unwrap(), status.state))
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn agents_running_as_the_pane_command_are_read_from_their_screen() {
        assert_eq!(
            state("claude", "⠋ Reading file src/main.rs"),
            Some(("claude".into(), AgentState::Running))
        );
        assert_eq!(
            state(
                "claude",
                "Allow write to src/main.rs?\n  Yes, allow\n  No, deny"
            ),
            Some(("claude".into(), AgentState::Waiting))
        );
        assert_eq!(
            state("claude", "❯ \n? for shortcuts"),
            Some(("claude".into(), AgentState::Idle))
        );
        assert_eq!(
            state("codex", "⠋ Searching codebase\nesc to interrupt"),
            Some(("codex".into(), AgentState::Running))
        );
    }

    #[test]
    fn old_agent_transcripts_in_a_shell_are_not_agents() {
        for screen in [
            "⠋ Reading file src/main.rs\nesc to interrupt\n❯ \n? for shortcuts",
            "/ commands\nDo you trust the contents of this directory?\nEnter to select",
            "Action required\nallow execution\n? for shortcuts",
            "regular output",
        ] {
            assert_eq!(state("zsh", screen), None, "{screen}");
        }
    }

    #[test]
    fn wrappers_are_recognised_by_screen_or_title_but_shells_not_by_title() {
        assert_eq!(
            state(
                "2.1.63",
                "▐▛███▜▌   Claude Code v2.1.63\n▝▜█████▛▘  Opus 4.6 · Claude Max\nDo you want to proceed?\n❯ 1. Yes"
            ),
            Some(("claude".into(), AgentState::Waiting))
        );
        assert_eq!(
            state(
                "2.1.316",
                "Claude Code'll be able to read, edit, and execute files here.\n\
                 ❯ No, exit\n  Yes, I trust this folder\nEnter to confirm · Esc to cancel"
            ),
            Some(("claude".into(), AgentState::Waiting))
        );
        assert_eq!(
            detect(
                "2.1.63",
                "✳ Claude Code",
                Some("Bash command\nDo you want to proceed?\n❯ 1. Yes"),
                0
            )
            .map(|status| status.state),
            Some(AgentState::Waiting)
        );
        assert_eq!(
            detect("zsh", "✳ Claude Code", Some("stale transcript\n$"), 0),
            None
        );
        assert_eq!(detect("vim", "OpenAI Codex", None, 0), None);
    }

    #[test]
    fn recent_activity_turns_an_unknown_screen_into_running_except_for_codex() {
        let unknown = "some output the patterns do not know";
        assert_eq!(
            detect("claude", "", Some(unknown), now()).map(|status| status.state),
            Some(AgentState::Running)
        );
        assert_eq!(
            detect("claude", "", Some(unknown), 0).map(|status| status.state),
            Some(AgentState::Unknown)
        );
        assert_eq!(
            detect("codex", "", Some(unknown), now()).map(|status| status.state),
            Some(AgentState::Unknown)
        );
        assert_eq!(
            detect("claude", "", Some("❯ \n? for shortcuts"), now()).map(|status| status.state),
            Some(AgentState::Idle)
        );
    }

    #[test]
    fn host_and_wrapper_commands_are_told_apart() {
        for shell in ["zsh", "BASH", "fish", "node"] {
            assert!(may_host_agent(shell), "{shell}");
        }
        assert!(!may_host_agent("vim"));
        assert!(looks_like_version_command("2.1.63"));
        assert!(looks_like_version_command("10.0"));
        assert!(!looks_like_version_command("2"));
        assert!(!looks_like_version_command("2..1"));
        assert!(!looks_like_version_command("v2.1"));
    }
}
