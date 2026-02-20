use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kiosk_core::action::Action;
use kiosk_core::state::{AppState, Mode};

/// Resolve a key event into an Action based on current mode
pub fn resolve_action(key: KeyEvent, state: &AppState) -> Option<Action> {
    // Global quit
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Action::Quit);
    }

    // Ctrl+N for new branch in branch select mode
    if key.code == KeyCode::Char('n') && key.modifiers.contains(KeyModifiers::CONTROL) && state.mode == Mode::BranchSelect {
        return Some(Action::StartNewBranchFlow);
    }

    // Clear error on any keypress
    match state.mode {
        Mode::RepoSelect => resolve_repo_key(key.code),
        Mode::BranchSelect => resolve_branch_key(key.code, state),
        Mode::NewBranchBase => resolve_new_branch_key(key.code),
        Mode::Loading(_) => None, // Handled directly in app.rs (only Ctrl+C)
    }
}

fn resolve_repo_key(key: KeyCode) -> Option<Action> {
    match key {
        KeyCode::Esc => Some(Action::Quit),
        KeyCode::Enter => Some(Action::OpenRepo),
        KeyCode::Tab => Some(Action::EnterRepo),
        KeyCode::Up => Some(Action::MoveSelection(-1)),
        KeyCode::Down => Some(Action::MoveSelection(1)),
        KeyCode::Backspace => Some(Action::SearchPop),
        KeyCode::Char(c) => Some(Action::SearchPush(c)),
        _ => None,
    }
}

fn resolve_branch_key(key: KeyCode, state: &AppState) -> Option<Action> {
    match key {
        KeyCode::Esc => Some(Action::GoBack),
        KeyCode::Enter => {
            if !state.branch_search.is_empty() && state.filtered_branches.is_empty() {
                Some(Action::StartNewBranchFlow)
            } else {
                Some(Action::OpenBranch)
            }
        }
        KeyCode::Up => Some(Action::MoveSelection(-1)),
        KeyCode::Down => Some(Action::MoveSelection(1)),
        KeyCode::Backspace => Some(Action::SearchPop),
        KeyCode::Char(c) => Some(Action::SearchPush(c)),
        _ => None,
    }
}

fn resolve_new_branch_key(key: KeyCode) -> Option<Action> {
    match key {
        KeyCode::Esc => Some(Action::GoBack),
        KeyCode::Enter => Some(Action::OpenBranch),
        KeyCode::Up => Some(Action::MoveSelection(-1)),
        KeyCode::Down => Some(Action::MoveSelection(1)),
        KeyCode::Backspace => Some(Action::SearchPop),
        KeyCode::Char(c) => Some(Action::SearchPush(c)),
        _ => None,
    }
}
