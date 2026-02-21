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

    // Check general bindings first
    if let Some(command) = keys.general.get(&our_key)
        && let Some(action) = command_to_action(command, state)
    {
        return Some(action);
    }

    // Check mode-specific bindings
    let mode_keymap = match state.mode {
        Mode::RepoSelect => &keys.repo_select,
        Mode::BranchSelect => &keys.branch_select,
        Mode::NewBranchBase => &keys.new_branch_base,
        Mode::ConfirmDelete { .. } => &keys.confirmation,
        Mode::Help { .. } => {
            // Help can be dismissed with C-h (ShowHelp toggle) or Esc
            if our_key == KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE) {
                return Some(Action::ShowHelp);
            }
            &keys.general
        }
        Mode::Loading(_) => return None, // Only general bindings work in loading mode
    };

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
            // Special logic for branch select mode - if search is non-empty and no matches, start new branch flow
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
            // Only available in branch select mode with 'd' key (handled specially in old version)
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
        Command::SearchPop => Some(Action::SearchPop),
        Command::SearchDeleteForward => Some(Action::SearchDeleteForward),
        Command::SearchDeleteWord => Some(Action::SearchDeleteWord),
        Command::SearchDeleteWordForward => Some(Action::SearchDeleteWordForward),
        Command::SearchDeleteToStart => Some(Action::SearchDeleteToStart),
        Command::SearchDeleteToEnd => Some(Action::SearchDeleteToEnd),
        Command::CursorLeft => Some(Action::CursorLeft),
        Command::CursorRight => Some(Action::CursorRight),
        Command::CursorWordLeft => Some(Action::CursorWordLeft),
        Command::CursorWordRight => Some(Action::CursorWordRight),
        Command::CursorStart => Some(Action::CursorStart),
        Command::CursorEnd => Some(Action::CursorEnd),
        Command::Confirm => match state.mode {
            Mode::ConfirmDelete { .. } => Some(Action::ConfirmDeleteWorktree),
            _ => None,
        },
        Command::Cancel => match state.mode {
            Mode::ConfirmDelete { .. } => Some(Action::CancelDeleteWorktree),
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
