//! Starting and stopping collectors. Kiosk pipes every pane without a pipe of its own to
//! a collector, and sets global hooks so panes made later get one too. A pane the user
//! already pipes elsewhere is left alone, as tmux allows one pipe per pane.

use std::path::Path;

use crate::{
    args,
    tmux::{PanePipe, Session, Tmux, TmuxError, chain},
};

/// The index kiosk's hooks use in tmux's hook arrays, clear of indices users pick by hand.
const HOOK_INDEX: u32 = 7501;
/// The commands after which a new pane exists.
const HOOKS: &[&str] = &[
    "after-new-session",
    "after-new-window",
    "after-split-window",
];

/// The shell command tmux pipes a pane to. tmux expands the formats for each pane; it does
/// not give the command `$TMUX`, so the socket is passed explicitly.
fn collector_command(executable: &Path) -> Result<String, TmuxError> {
    let path = executable.to_str().filter(|path| {
        path.chars()
            .all(|character| character.is_ascii_alphanumeric() || "/._+-@ ,=~:".contains(character))
    });
    let Some(path) = path else {
        return Err(TmuxError::InvalidArgument(format!(
            "kiosk's path {} has characters that cannot be put in a tmux hook safely; \
             set `agent.osc7501 = false` or move the kiosk binary",
            kiosk_core::display::sanitize(&executable.to_string_lossy())
        )));
    };
    Ok(format!(
        "'{path}' collect --socket #{{q:socket_path}} --pane #{{pane_id}}"
    ))
}

fn pipe_pane(target: &str, command: &str) -> Vec<String> {
    args!["pipe-pane", "-O", "-o", "-t", target, command]
}

/// Starts collectors for `sessions`' untracked panes and keeps the hooks pointing at
/// `executable`, which may have moved since they were set.
pub fn enable(tmux: &dyn Tmux, executable: &Path, sessions: &[Session]) -> Result<(), TmuxError> {
    let command = collector_command(executable)?;
    let hook = format!("pipe-pane -O -o \"{command}\"");
    let hooks = HOOKS.iter().map(|name| {
        args![
            "set-hook",
            "-g",
            format!("{name}[{HOOK_INDEX}]"),
            hook.clone()
        ]
    });
    let panes = sessions
        .iter()
        .flat_map(|session| &session.panes)
        .filter(|pane| pane.pipe == PanePipe::None && !pane.dead)
        .map(|pane| pipe_pane(pane.id.as_str(), &command));
    ignore_no_server(tmux.run(chain(hooks.chain(panes))))
}

/// Removes the hooks and stops kiosk's own collectors, leaving other pipes alone.
pub fn disable(tmux: &dyn Tmux, sessions: &[Session]) -> Result<(), TmuxError> {
    let hooks = HOOKS
        .iter()
        .map(|name| args!["set-hook", "-gu", format!("{name}[{HOOK_INDEX}]")]);
    let pipes = sessions
        .iter()
        .flat_map(|session| &session.panes)
        .filter(|pane| pane.pipe == PanePipe::Kiosk)
        .map(|pane| args!["pipe-pane", "-t", pane.id.as_str()]);
    ignore_no_server(tmux.run(chain(hooks.chain(pipes))))
}

/// Without a server there are no panes to track and nowhere to keep hooks.
fn ignore_no_server(result: Result<String, TmuxError>) -> Result<(), TmuxError> {
    match result {
        Ok(_) | Err(TmuxError::NoServer) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::tmux::{Pane, PaneId, SessionId, mock::MockTmux};

    fn pane(id: &str, pipe: PanePipe, dead: bool) -> Pane {
        Pane {
            id: PaneId::parse(id).unwrap(),
            pid: 1,
            current_command: "zsh".into(),
            title: String::new(),
            dead,
            pipe,
            program_status: None,
        }
    }

    fn sessions() -> Vec<Session> {
        vec![Session {
            id: SessionId::parse("$1").unwrap(),
            name: "s".into(),
            activity: 0,
            place: None,
            panes: vec![
                pane("%1", PanePipe::None, false),
                pane("%2", PanePipe::Kiosk, false),
                pane("%3", PanePipe::Other, false),
                pane("%4", PanePipe::None, true),
            ],
        }]
    }

    #[test]
    fn enabling_sets_hooks_and_pipes_only_unpiped_live_panes() {
        let tmux = MockTmux::default();
        tmux.respond(Ok(String::new()));
        enable(&tmux, Path::new("/opt/bin/kiosk"), &sessions()).unwrap();
        let command = "'/opt/bin/kiosk' collect --socket #{q:socket_path} --pane #{pane_id}";
        let hook = format!("pipe-pane -O -o \"{command}\"");
        assert_eq!(
            tmux.calls(),
            [chain([
                args!["set-hook", "-g", "after-new-session[7501]", hook.clone()],
                args!["set-hook", "-g", "after-new-window[7501]", hook.clone()],
                args!["set-hook", "-g", "after-split-window[7501]", hook],
                args!["pipe-pane", "-O", "-o", "-t", "%1", command],
            ])]
        );
    }

    #[test]
    fn disabling_removes_hooks_and_stops_only_kiosk_pipes() {
        let tmux = MockTmux::default();
        tmux.respond(Err(TmuxError::NoServer));
        disable(&tmux, &sessions()).unwrap();
        assert_eq!(
            tmux.calls(),
            [chain([
                args!["set-hook", "-gu", "after-new-session[7501]"],
                args!["set-hook", "-gu", "after-new-window[7501]"],
                args!["set-hook", "-gu", "after-split-window[7501]"],
                args!["pipe-pane", "-t", "%2"],
            ])]
        );
    }

    #[test]
    fn unsafe_executable_paths_are_refused() {
        for path in ["/a'b/kiosk", "/a\"b", "/$HOME/kiosk", "/a;b", "/a#{b}"] {
            assert!(collector_command(&PathBuf::from(path)).is_err(), "{path}");
        }
    }
}
