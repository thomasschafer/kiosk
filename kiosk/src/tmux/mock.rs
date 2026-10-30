//! A scripted tmux that records each invocation, for tests.

use std::{collections::VecDeque, sync::Mutex};

use super::{FIELD_SEPARATOR, Tmux, TmuxError};

/// Answers each invocation with the next queued response. An invocation with nothing
/// queued fails, so tests notice calls they did not expect.
#[derive(Default)]
pub struct MockTmux {
    pub calls: Mutex<Vec<Vec<String>>>,
    responses: Mutex<VecDeque<Result<String, TmuxError>>>,
}

impl MockTmux {
    pub fn respond(&self, response: Result<String, TmuxError>) {
        self.responses.lock().unwrap().push_back(response);
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl Tmux for MockTmux {
    fn run(&self, args: Vec<String>) -> Result<String, TmuxError> {
        self.calls.lock().unwrap().push(args.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Err(TmuxError::Invocation(format!(
                    "mock tmux has no response queued for {args:?}"
                )))
            })
    }
}

/// One pane as `list-panes -a` prints it for [`super::sessions`].
pub struct PaneLine<'a> {
    pub session: &'a str,
    pub name: &'a str,
    pub started_in: &'a str,
    /// The kiosk path option; empty for sessions kiosk did not make.
    pub tagged: &'a str,
    pub pane: &'a str,
    pub command: &'a str,
    /// The collector's summary; set means a kiosk collector pipes the pane.
    pub status: Option<&'a str>,
}

impl PaneLine<'_> {
    pub fn render(&self) -> String {
        let (pipe, collector, status) = match self.status {
            Some(status) => ("1", "42", status),
            None => ("0", "", ""),
        };
        [
            self.session,
            self.name,
            "0",
            self.started_in,
            self.tagged,
            self.pane,
            // A process with no children, so process-tree lookups find nothing.
            "999999",
            "0",
            pipe,
            collector,
            status,
            self.command,
            "",
        ]
        .join(&FIELD_SEPARATOR.to_string())
    }
}

/// The `list-panes -a` output for `panes`.
pub fn listing(panes: &[PaneLine<'_>]) -> String {
    panes.iter().map(|pane| pane.render() + "\n").collect()
}
