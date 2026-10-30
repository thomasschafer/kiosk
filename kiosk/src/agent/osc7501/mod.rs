//! Agent status from the Program Status Protocol (OSC 7501), which programs report
//! themselves. See <https://mitchellh.com/writing/program-status-osc7501>.

pub mod collector;
mod records;
pub mod report;
mod scanner;
pub mod tracking;

use self::report::{BlockedKind, ProgramState, Report};
use super::{AgentState, PaneStatus, Source, StatusSource};
use crate::tmux::{Pane, PanePipe, Session};

/// Reads the record a kiosk collector keeps for a pane.
pub struct Osc7501;

impl StatusSource for Osc7501 {
    fn status(&self, pane: &Pane, _session: &Session) -> Option<PaneStatus> {
        if pane.pipe != PanePipe::Kiosk {
            return None;
        }
        let Some(Report::Set { record, .. }) = report::parse(pane.program_status.as_deref()?)
        else {
            return None;
        };
        // The program that reported working or blocked ended with the pane's process.
        if pane.dead && record.state.ends_with_program() {
            return None;
        }
        Some(PaneStatus {
            agent: record.app,
            state: match record.state {
                ProgramState::Idle => AgentState::Idle,
                ProgramState::Working => AgentState::Running,
                ProgramState::Done => AgentState::Done,
                ProgramState::Blocked => AgentState::Waiting,
                ProgramState::Error => AgentState::Error,
            },
            detail: record.kind.map(BlockedKind::as_str),
            source: Source::Osc7501,
            rule: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{PaneId, SessionId};

    fn pane(pipe: PanePipe, status: Option<&str>, dead: bool) -> (Pane, Session) {
        let pane = Pane {
            id: PaneId::parse("%1").unwrap(),
            pid: 1,
            current_command: "claude".into(),
            title: String::new(),
            dead,
            pipe,
            program_status: status.map(str::to_owned),
        };
        let session = Session {
            id: SessionId::parse("$1").unwrap(),
            name: "s".into(),
            activity: 0,
            place: None,
            panes: vec![pane.clone()],
        };
        (pane, session)
    }

    fn status(pipe: PanePipe, status: Option<&str>, dead: bool) -> Option<PaneStatus> {
        let (pane, session) = pane(pipe, status, dead);
        Osc7501.status(&pane, &session)
    }

    #[test]
    fn kiosk_collected_records_become_statuses() {
        assert_eq!(
            status(
                PanePipe::Kiosk,
                Some("state=blocked:kind=permission:app=claude-code"),
                false
            ),
            Some(PaneStatus {
                agent: Some("claude-code".into()),
                state: AgentState::Waiting,
                detail: Some("permission"),
                source: Source::Osc7501,
                rule: None,
            })
        );
    }

    #[test]
    fn records_need_a_kiosk_pipe_and_a_live_program() {
        assert_eq!(status(PanePipe::Kiosk, None, false), None);
        assert_eq!(status(PanePipe::Other, Some("state=idle"), false), None);
        assert_eq!(status(PanePipe::Kiosk, Some("state=working"), true), None);
        assert_eq!(
            status(PanePipe::Kiosk, Some("state=done"), true).map(|status| status.state),
            Some(AgentState::Done)
        );
    }
}
