//! The messages a frontend and the picker exchange: one JSON object per line, each with a
//! `t` naming its kind.
//!
//! The frontend opens with [`ClientMessage::Hello`]. The picker answers with
//! [`ServerMessage::Hello`], or [`ServerMessage::Error`] if it cannot start, then sends a
//! [`ServerMessage::View`] whenever what it shows changes, and [`ServerMessage::Exit`] last.
//! A quiet picker sends [`ServerMessage::Ping`] every [`HEARTBEAT`] so a frontend that waits
//! on reads with a timeout can tell a quiet picker from a stuck one.

use std::time::Duration;

use kiosk_core::{
    tabs::TabId,
    view::{setup::FolderModeName, setup::NoSearchDirsView, setup::SetupView, tabs::TabsView},
};
use serde::{Deserialize, Serialize};

/// Bumped whenever a message changes shape, so a frontend and a picker from different
/// versions refuse each other rather than misread each other.
pub const PROTOCOL_VERSION: u32 = 4;

/// Refuses a client that speaks another version of the protocol, saying which to update.
pub fn check_client(protocol: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        protocol == PROTOCOL_VERSION,
        "the frontend speaks protocol {protocol} and this picker speaks {PROTOCOL_VERSION}; \
         install matching versions of both"
    );
    Ok(())
}

/// How long the picker stays quiet before it sends [`ServerMessage::Ping`].
pub const HEARTBEAT: Duration = Duration::from_secs(10);

/// What the picker shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "a session holds one screen at a time, compared and sent as it is"
)]
pub enum Screen {
    Setup(SetupView),
    NoSearchDirs(NoSearchDirsView),
    Tabs(TabsView),
}

#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMessage<'a> {
    Hello {
        protocol: u32,
        product: &'a str,
        /// Every chord the picker acts on besides typed characters, spelled as GPUI does
        /// (`ctrl-h`, `escape`), for the frontend to claim before its host's shortcuts.
        chords: Vec<String>,
        /// Every tab the picker knows, whether or not it shows it, so the frontend can refuse
        /// a request for a tab with no name the picker understands.
        tabs: &'a [TabId],
    },
    View {
        view: &'a Screen,
    },
    /// A notification for the frontend's host to show, which outlives the picker.
    Notify {
        title: &'a str,
        body: &'a str,
        level: NoticeLevel,
    },
    /// The picker is done; the connection closes next.
    Exit {
        outcome: Outcome,
    },
    /// The picker could not start or stopped unexpectedly; the connection closes next.
    Error {
        title: &'a str,
        body: &'a str,
    },
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Something opened.
    Opened,
    /// The user left without opening anything.
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        token: String,
        protocol: u32,
    },
    /// A key press, as GPUI spells it (`ctrl-shift-a`), and the text it types, if any.
    Key {
        keystroke: String,
        #[serde(default)]
        key_char: Option<String>,
    },
    /// Text pasted into whichever field has the keyboard.
    Paste {
        text: String,
    },
    /// How many rows each list has room for.
    Viewport {
        list: usize,
        bases: usize,
        help: usize,
        scan_warnings: usize,
    },
    /// A click on the row at `position` of the list in front, or on one of its buttons.
    /// `id` is the item the row showed, so a click on a row that has since moved does
    /// nothing.
    Activate {
        position: usize,
        id: String,
        #[serde(default)]
        command: PointerCommand,
    },
    /// A button for one of the picker's commands, such as a dialog's confirm.
    Command {
        command: ButtonCommand,
    },
    /// Closes the dialog or overlay in front without acting on it: a cancel button, or a
    /// click beside it.
    Dismiss,
    /// A wheel or trackpad scroll over the list in front, in rows.
    Scroll {
        delta: i32,
    },
    /// A click on a setup wizard folder mode.
    FolderMode {
        mode: FolderModeName,
    },
    /// A click on a setup wizard path completion.
    Completion {
        index: usize,
    },
    /// A click on a tab in the tab bar.
    ShowTab {
        tab: TabId,
    },
    /// A key bound in the frontend's host asks for `tab`: it shows, or kiosk closes if it
    /// already shows, as pressing the key that opened kiosk again does.
    Launch {
        tab: TabId,
    },
    /// The frontend is closing, as if the user had quit.
    Close,
}

/// What a click does to the row it lands on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerCommand {
    /// The row itself: what the open key does.
    #[default]
    Open,
    /// A repository's branches button.
    Branches,
    /// A checkout's delete button.
    Delete,
}

/// What a button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonCommand {
    /// Open or confirm, as the open key does.
    Open,
    /// Back to the repository list from a repository's branches, whatever is typed.
    Repositories,
    /// Start a new branch, as the new-branch key does.
    NewBranch,
    /// Show the key bindings.
    Help,
}

impl ButtonCommand {
    /// What the button does where the picker is: nothing while an overlay, such as help,
    /// takes the input, as no button in front of it names this command.
    pub fn action(
        self,
        state: &kiosk_core::state::AppState,
    ) -> Option<kiosk_core::keyboard::Action> {
        use kiosk_core::{config::keys::Command, keymap};
        if state.overlay.is_some() {
            return None;
        }
        match self {
            Self::Open => keymap::command_to_action(Command::Open, state),
            Self::Repositories => keymap::back_to_repos(state),
            Self::NewBranch => keymap::command_to_action(Command::NewBranch, state),
            Self::Help => keymap::command_to_action(Command::Help, state),
        }
    }
}

impl PointerCommand {
    pub const fn command(self) -> kiosk_core::config::keys::Command {
        use kiosk_core::config::keys::Command;
        match self {
            Self::Open => Command::Open,
            Self::Branches => Command::BranchesView,
            Self::Delete => Command::Delete,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_messages_parse_from_their_wire_form() {
        let parse = |line: &str| serde_json::from_str::<ClientMessage>(line).unwrap();
        assert_eq!(
            parse(r#"{"t":"hello","token":"abc","protocol":1}"#),
            ClientMessage::Hello {
                token: "abc".into(),
                protocol: 1
            }
        );
        assert_eq!(
            parse(r#"{"t":"key","keystroke":"ctrl-h"}"#),
            ClientMessage::Key {
                keystroke: "ctrl-h".into(),
                key_char: None
            }
        );
        assert_eq!(
            parse(r#"{"t":"key","keystroke":"shift-a","key_char":"A"}"#),
            ClientMessage::Key {
                keystroke: "shift-a".into(),
                key_char: Some("A".into())
            }
        );
        assert_eq!(
            parse(r#"{"t":"activate","position":3,"id":"repo:/a"}"#),
            ClientMessage::Activate {
                position: 3,
                id: "repo:/a".into(),
                command: PointerCommand::Open
            }
        );
        assert_eq!(
            parse(r#"{"t":"activate","position":0,"id":"branch:/a:x","command":"delete"}"#),
            ClientMessage::Activate {
                position: 0,
                id: "branch:/a:x".into(),
                command: PointerCommand::Delete
            }
        );
        assert_eq!(
            parse(r#"{"t":"folder_mode","mode":"all_folders"}"#),
            ClientMessage::FolderMode {
                mode: FolderModeName::AllFolders
            }
        );
        assert_eq!(
            parse(r#"{"t":"command","command":"repositories"}"#),
            ClientMessage::Command {
                command: ButtonCommand::Repositories
            }
        );
        assert_eq!(
            parse(r#"{"t":"show_tab","tab":"diff"}"#),
            ClientMessage::ShowTab { tab: TabId::Diff }
        );
        assert_eq!(
            parse(r#"{"t":"launch","tab":"picker"}"#),
            ClientMessage::Launch { tab: TabId::Picker }
        );
        assert_eq!(parse(r#"{"t":"dismiss"}"#), ClientMessage::Dismiss);
        assert_eq!(parse(r#"{"t":"close"}"#), ClientMessage::Close);
    }

    #[test]
    fn a_client_of_another_protocol_is_refused_with_what_to_do() {
        assert!(check_client(PROTOCOL_VERSION).is_ok());
        let error = check_client(PROTOCOL_VERSION + 1).unwrap_err().to_string();
        assert!(error.contains("install matching versions"), "{error}");
    }

    #[test]
    fn buttons_do_nothing_behind_an_overlay() {
        use kiosk_core::{
            config::keys::KeysConfig,
            product::TEST_PRODUCT,
            state::{AppState, HelpOverlayState, Overlay},
        };
        let mut state = AppState::new(&TEST_PRODUCT, None);
        assert!(ButtonCommand::Help.action(&state).is_some());
        state.overlay = Some(Overlay::Help(HelpOverlayState::for_mode(
            &KeysConfig::default(),
            KeysConfig::mode_for(&state.mode),
        )));
        for button in [
            ButtonCommand::Open,
            ButtonCommand::Repositories,
            ButtonCommand::NewBranch,
            ButtonCommand::Help,
        ] {
            assert_eq!(button.action(&state), None, "{button:?}");
        }
    }

    #[test]
    fn unknown_client_messages_are_refused() {
        assert!(serde_json::from_str::<ClientMessage>(r#"{"t":"launch_missiles"}"#).is_err());
        assert!(serde_json::from_str::<ClientMessage>(r#"{"t":"scroll"}"#).is_err());
        assert!(serde_json::from_str::<ClientMessage>(r#"{"t":"launch","tab":"logs"}"#).is_err());
    }

    #[test]
    fn server_messages_carry_their_kind() {
        let line = serde_json::to_string(&ServerMessage::Exit {
            outcome: Outcome::Opened,
        })
        .unwrap();
        assert_eq!(line, r#"{"t":"exit","outcome":"opened"}"#);
        assert_eq!(
            serde_json::to_string(&ServerMessage::Ping).unwrap(),
            r#"{"t":"ping"}"#
        );
    }
}
