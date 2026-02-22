use kiosk_core::action::Action;
use kiosk_core::config::{Command, KeysConfig};
use kiosk_core::keyboard::{KeyCode, KeyEvent, KeyModifiers};
use kiosk_core::state::{AppState, Mode};

/// Resolve a key event into an Action based on current mode and key configuration
pub fn resolve_action(
    key: crossterm::event::KeyEvent,
    state: &AppState,
    keys: &KeysConfig,
) -> Option<Action> {
    // Convert crossterm KeyEvent to our KeyEvent and canonicalize
    let mut our_key: KeyEvent = key.into();
    our_key.canonicalize();

    // Help can always be dismissed with Esc
    if matches!(state.mode, Mode::Help { .. })
        && our_key == KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    {
        return Some(Action::ShowHelp);
    }

    let mode_keymap = keys.keymap_for_mode(&state.mode);
    if let Some(command) = mode_keymap.get(&our_key)
        && let Some(action) = command_to_action(command, state)
    {
        return Some(action);
    }

    // Handle printable characters for search in search-enabled modes
    if can_search_in_mode(&state.mode)
        && let KeyCode::Char(c) = our_key.code
        && (our_key.modifiers == KeyModifiers::NONE && c.is_ascii_graphic() || c == ' ')
    {
        return Some(Action::SearchPush(c));
    }

    None
}

/// Convert a Command to an Action, taking into account the current state
fn command_to_action(command: &Command, state: &AppState) -> Option<Action> {
    match command {
        Command::Noop => None,
        Command::Quit => Some(Action::Quit),
        Command::ShowHelp => Some(Action::ShowHelp),
        Command::OpenRepo => Some(Action::OpenRepo),
        Command::EnterRepo => Some(Action::EnterRepo),
        Command::OpenBranch => {
            // In branch-select mode, Enter with non-empty search and no matches starts new branch flow.
            if let Mode::BranchSelect = state.mode
                && !state.branch_list.search.is_empty()
                && state.branch_list.filtered.is_empty()
            {
                return Some(Action::StartNewBranchFlow);
            }
            Some(Action::OpenBranch)
        }
        Command::GoBack => Some(Action::GoBack),
        Command::NewBranch => Some(Action::StartNewBranchFlow),
        Command::DeleteWorktree => {
            if let Mode::BranchSelect = state.mode {
                Some(Action::DeleteWorktree)
            } else {
                None
            }
        }
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        Command::HalfPageUp => Some(Action::HalfPageUp),
        Command::HalfPageDown => Some(Action::HalfPageDown),
        Command::PageUp => Some(Action::PageUp),
        Command::PageDown => Some(Action::PageDown),
        Command::MoveTop => Some(Action::MoveTop),
        Command::MoveBottom => Some(Action::MoveBottom),
        Command::DeleteBackwardChar => Some(Action::SearchPop),
        Command::DeleteBackwardWord => Some(Action::SearchDeleteWord),
        Command::MoveCursorLeft => Some(Action::CursorLeft),
        Command::MoveCursorRight => Some(Action::CursorRight),
        Command::MoveCursorStart => Some(Action::CursorStart),
        Command::MoveCursorEnd => Some(Action::CursorEnd),
        Command::Confirm => match state.mode {
            Mode::ConfirmDelete { .. } => Some(Action::ConfirmDeleteWorktree),
            Mode::NewBranchBase => Some(Action::OpenBranch),
            _ => None,
        },
        Command::Cancel => match state.mode {
            Mode::ConfirmDelete { .. } => Some(Action::CancelDeleteWorktree),
            Mode::NewBranchBase => Some(Action::GoBack),
            _ => None,
        },
    }
}

/// Check if the current mode supports search input
fn can_search_in_mode(mode: &Mode) -> bool {
    matches!(
        mode,
        Mode::RepoSelect | Mode::BranchSelect | Mode::NewBranchBase
    )
}
