//! What the agents in tmux panes are doing. Each [`StatusSource`] answers for a pane or
//! passes; [`Detector`] asks them in order, so a pane that reports its own status over
//! OSC 7501 is never second-guessed by screen heuristics.

pub mod heuristics;
pub mod osc7501;

use std::fmt;

use kiosk_core::backend::{Label, Tone};
use serde::Serialize;

use crate::{
    config::AgentConfig,
    tmux::{Pane, PaneId, Session, Tmux},
};

/// What an agent is doing, in the order it wants the user's attention, most first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Needs the user: a permission prompt, a question or a login.
    Waiting,
    Error,
    /// Finished something the user has not looked at.
    Done,
    Idle,
    Running,
    /// An agent whose screen matched nothing known.
    Unknown,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Error => "error",
            Self::Done => "done",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the agent is waiting on the user rather than working.
    pub fn wants_user(self) -> bool {
        !matches!(self, Self::Running)
    }

    fn tone(self) -> Tone {
        match self {
            Self::Waiting => Tone::Attention,
            Self::Error => Tone::Failure,
            Self::Done => Tone::Success,
            Self::Idle => Tone::Neutral,
            Self::Running => Tone::Progress,
            Self::Unknown => Tone::Muted,
        }
    }
}

impl fmt::Display for AgentState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a status came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Osc7501,
    Heuristics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneStatus {
    /// The agent, when known: an OSC 7501 `app` or a recognised agent's name.
    pub agent: Option<String>,
    pub state: AgentState,
    /// What a waiting agent waits for, when it says.
    pub detail: Option<&'static str>,
    pub source: Source,
    /// Which screen pattern decided the state, for diagnosing heuristics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<&'static str>,
}

impl PaneStatus {
    pub fn label(&self) -> Label {
        Label {
            text: self.agent.as_ref().map_or_else(
                || self.state.to_string(),
                |agent| format!("{agent} {}", self.state),
            ),
            tone: self.state.tone(),
        }
    }
}

/// The panes of a session with an agent, and their status.
pub type PaneStatuses = Vec<(PaneId, PaneStatus)>;

pub trait StatusSource {
    /// The status of the agent in `pane` of `session`, or `None` to leave it to the next
    /// source.
    fn status(&self, pane: &Pane, session: &Session) -> Option<PaneStatus>;
}

/// The sources the config turns on, in the order they are asked.
pub struct Detector<'a> {
    sources: Vec<Box<dyn StatusSource + 'a>>,
}

impl<'a> Detector<'a> {
    pub fn new(config: &AgentConfig, tmux: &'a dyn Tmux) -> Self {
        let mut sources: Vec<Box<dyn StatusSource + 'a>> = Vec::new();
        if config.enabled {
            if config.osc7501 {
                sources.push(Box::new(osc7501::Osc7501));
            }
            sources.push(Box::new(heuristics::Heuristics::new(tmux)));
        }
        Self { sources }
    }

    pub fn pane(&self, pane: &Pane, session: &Session) -> Option<PaneStatus> {
        self.sources
            .iter()
            .find_map(|source| source.status(pane, session))
    }

    /// Every pane of `session` with an agent, most urgent first.
    pub fn session(&self, session: &Session) -> PaneStatuses {
        let mut statuses: Vec<_> = session
            .panes
            .iter()
            .filter_map(|pane| Some((pane.id.clone(), self.pane(pane, session)?)))
            .collect();
        sort(&mut statuses);
        statuses
    }
}

/// Most urgent first; panes keep their order among equals.
pub fn sort(statuses: &mut [(PaneId, PaneStatus)]) {
    statuses.sort_by_key(|(_, status)| status.state);
}

/// One label per distinct status, most urgent first.
pub fn labels(statuses: &[(PaneId, PaneStatus)]) -> Vec<Label> {
    let mut labels: Vec<Label> = Vec::new();
    for (_, status) in statuses {
        let label = status.label();
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{PanePipe, SessionId, mock::MockTmux};

    fn pane(id: &str, status: &str) -> Pane {
        Pane {
            id: PaneId::parse(id).unwrap(),
            pid: 1,
            current_command: "vim".into(),
            title: String::new(),
            dead: false,
            pipe: PanePipe::Kiosk,
            program_status: Some(status.into()),
        }
    }

    fn session() -> Session {
        Session {
            id: SessionId::parse("$1").unwrap(),
            name: "s".into(),
            activity: 0,
            place: None,
            panes: vec![
                pane("%1", "state=working:app=claude-code"),
                pane("%2", "state=blocked:app=codex"),
                pane("%3", "state=working:app=claude-code"),
            ],
        }
    }

    #[test]
    fn statuses_are_sorted_by_urgency_and_labels_deduplicated() {
        let tmux = MockTmux::default();
        let detector = Detector::new(&AgentConfig::default(), &tmux);
        let statuses = detector.session(&session());
        assert_eq!(
            statuses
                .iter()
                .map(|(pane, _)| pane.as_str())
                .collect::<Vec<_>>(),
            ["%2", "%1", "%3"]
        );
        assert_eq!(
            labels(&statuses),
            [
                Label {
                    text: "codex waiting".into(),
                    tone: Tone::Attention
                },
                Label {
                    text: "claude-code running".into(),
                    tone: Tone::Progress
                },
            ]
        );
    }

    #[test]
    fn turning_osc7501_off_leaves_only_heuristics_and_disabling_leaves_nothing() {
        let tmux = MockTmux::default();
        let session = session();
        let without_osc = AgentConfig {
            osc7501: false,
            ..AgentConfig::default()
        };
        // vim is not an agent, so the heuristics find nothing without asking tmux.
        assert_eq!(Detector::new(&without_osc, &tmux).session(&session), []);
        let disabled = AgentConfig {
            enabled: false,
            ..AgentConfig::default()
        };
        assert_eq!(Detector::new(&disabled, &tmux).session(&session), []);
        assert_eq!(tmux.calls(), Vec::<Vec<String>>::new());
    }
}
