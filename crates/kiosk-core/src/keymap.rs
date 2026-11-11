//! What a key press does in the picker. Frontends turn their own key events into a
//! [`KeyInput`]; [`resolve_input`] turns that into the [`Action`] the picker performs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    config::keys::{BindingMode, Command, KeyChord, KeysConfig},
    keyboard::Action,
    state::{AppState, Mode, Overlay, TextEdit},
};

/// The scan warnings view scrolls whatever mode it opened over, so it takes its scroll
/// keys from the dialog, navigation and general layers.
pub const SCAN_WARNINGS_BINDING_MODE: BindingMode = BindingMode::Choice;

const ESC: KeyChord = KeyChord::new(KeyCode::Esc, KeyModifiers::NONE);

/// One key press: the chord bindings are looked up by, and the character it types, if any.
///
/// The two are separate because they can disagree. In a terminal a chord with ctrl or alt
/// types nothing, but on macOS option composes characters (`option+2` is `@` on many
/// layouts), so a GUI reports `alt+2` typing `@`. A bound chord always wins; an unbound one
/// types its character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub chord: KeyChord,
    pub text: Option<char>,
}

impl KeyInput {
    /// A terminal key event, where only a chord without ctrl or alt types its character.
    pub fn from_event(event: KeyEvent) -> Self {
        let chord = KeyChord::from_event(event);
        let text = match chord.code {
            KeyCode::Char(character)
                if !chord
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(character)
            }
            _ => None,
        };
        Self { chord, text }
    }

    /// The printable character this press types.
    fn typed(self) -> Option<char> {
        self.text.filter(|character| !character.is_control())
    }
}

/// [`resolve_input`] for a terminal key event.
pub fn resolve_action(key: KeyEvent, state: &AppState, keys: &KeysConfig) -> Option<Action> {
    resolve_input(KeyInput::from_event(key), state, keys)
}

pub fn resolve_input(input: KeyInput, state: &AppState, keys: &KeysConfig) -> Option<Action> {
    let chord = input.chord;
    match &state.overlay {
        Some(Overlay::Help(_)) => {
            let command = keys.command_for(KeysConfig::mode_for(&state.mode), chord);
            return help_overlay_action(input, command, keys);
        }
        Some(Overlay::ScanWarnings(_)) => {
            let opening = keys.command_for(KeysConfig::mode_for(&state.mode), chord);
            let scrolling = keys.command_for(SCAN_WARNINGS_BINDING_MODE, chord);
            return scan_warnings_overlay_action(chord, opening, scrolling);
        }
        None => {}
    }

    if !state.toasts.is_empty() && keys.is_dismiss_toast(chord) {
        return Some(Action::DismissToast);
    }

    let binding_mode = KeysConfig::mode_for(&state.mode);
    if matches!(
        state.mode,
        Mode::Loading { .. } | Mode::ValidatingNewBranch { .. }
    ) {
        return match keys.command_for(binding_mode, chord) {
            Some(Command::Quit) => Some(Action::Quit),
            _ => None,
        };
    }

    let query = active_query(state);
    if let Some(command) = keys.command_for(binding_mode, chord)
        && !(command == Command::Quit
            && chord.modifiers == KeyModifiers::NONE
            && matches!(chord.code, KeyCode::Char(_))
            && !query.is_empty())
    {
        return command_to_action(command, state);
    }
    if let Some(character) = input.typed()
        && matches!(
            state.mode,
            Mode::RepoSelect | Mode::BranchSelect(_) | Mode::SelectBaseBranch { .. }
        )
    {
        return Some(Action::Insert(character));
    }
    None
}

/// Selecting the item at `position` of the list in front with the pointer, or `None`
/// when nothing in front has items to select.
pub fn select(position: usize, state: &AppState) -> Option<Action> {
    pointer_list_in_front(state).then_some(Action::Select(position))
}

/// Closing whatever is in front of the list without acting on it, as a dialog's cancel
/// button or a click beside it does: an overlay, or a dialog's flow. `None` when nothing
/// can be closed, as while a new branch's name is checked.
pub fn dismiss(state: &AppState) -> Option<Action> {
    if state.overlay.is_some() {
        return Some(Action::CloseOverlay);
    }
    match state.mode {
        Mode::SelectBaseBranch { .. }
        | Mode::ConfirmRemoteBranch { .. }
        | Mode::ConfirmWorktreeDelete(_) => Some(Action::CancelOverlay),
        Mode::RepoSelect
        | Mode::BranchSelect(_)
        | Mode::ValidatingNewBranch { .. }
        | Mode::Loading { .. } => None,
    }
}

/// Leaving a repository's branches for the repository list, whatever is typed, as a back
/// button does.
pub fn back_to_repos(state: &AppState) -> Option<Action> {
    (state.overlay.is_none() && matches!(state.mode, Mode::BranchSelect(_)))
        .then_some(Action::BackToRepos)
}

/// Whether the list in front takes pointer selection: an overlay's, the mode's list or the
/// remote branch prompt's choices.
fn pointer_list_in_front(state: &AppState) -> bool {
    match &state.overlay {
        Some(Overlay::Help(_)) => true,
        Some(Overlay::ScanWarnings(_)) => false,
        None => matches!(
            state.mode,
            Mode::RepoSelect
                | Mode::BranchSelect(_)
                | Mode::SelectBaseBranch { .. }
                | Mode::ConfirmRemoteBranch { .. }
        ),
    }
}

/// Help owns its own query, so typing filters it and `esc` always closes it.
/// Help's search edits with the text-editing keys even over a dialog that has none, unless
/// the dialog binds the chord itself.
pub fn help_overlay_action(
    input: KeyInput,
    command: Option<Command>,
    keys: &KeysConfig,
) -> Option<Action> {
    if input.chord == ESC {
        return Some(Action::CloseOverlay);
    }
    if let Some(action) = command.and_then(help_command_to_action) {
        return Some(action);
    }
    if command.is_none()
        && let Some(edit) = keys.text_edit(input.chord)
    {
        return Some(Action::Edit(edit));
    }
    input.typed().map(Action::Insert)
}

/// The scan warnings view only scrolls, and closes with `esc` or the key that opened it.
/// `opening` is what the chord does in the mode underneath, so that key always closes the
/// view, and `scrolling` what it does in the view.
fn scan_warnings_overlay_action(
    chord: KeyChord,
    opening: Option<Command>,
    scrolling: Option<Command>,
) -> Option<Action> {
    if chord == ESC || opening == Some(Command::ScanWarnings) {
        return Some(Action::CloseOverlay);
    }
    match scrolling? {
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        _ => None,
    }
}

fn help_command_to_action(command: Command) -> Option<Action> {
    match command {
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        _ => command.text_edit().map(Action::Edit),
    }
}

pub fn command_to_action(command: Command, state: &AppState) -> Option<Action> {
    match command {
        // Tabs are changed before a key reaches the picker; see `KeysConfig::tab_command`.
        // The diff tab's commands are the diff tab's.
        Command::Noop
        | Command::NextTab
        | Command::PreviousTab
        | Command::ShowTab(_)
        | Command::ScrollDown
        | Command::ScrollUp
        | Command::ScrollLeft
        | Command::ScrollRight
        | Command::PageDown
        | Command::PageUp
        | Command::ScrollTop
        | Command::ScrollBottom
        | Command::NextFile
        | Command::PreviousFile
        | Command::NextRange
        | Command::PreviousRange
        | Command::ToggleFile
        | Command::ToggleLayout
        | Command::OpenInEditor => None,
        Command::Quit => Some(Action::Quit),
        Command::Help => Some(Action::ShowHelp),
        Command::DismissToast => (!state.toasts.is_empty()).then_some(Action::DismissToast),
        Command::ScanWarnings => {
            (!state.scan_warnings().is_empty()).then_some(Action::ShowScanWarnings)
        }
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        Command::Open => match &state.mode {
            Mode::RepoSelect => Some(Action::OpenRepo),
            Mode::BranchSelect(_) if state.branch_view.is_loading() => None,
            Mode::BranchSelect(_)
                if !state.branch_view.is_loading()
                    && !active_query(state).is_empty()
                    && state.branch_view.list.filtered.is_empty() =>
            {
                Some(Action::StartNewBranch)
            }
            Mode::BranchSelect(_) => Some(Action::OpenBranch),
            Mode::SelectBaseBranch { .. } => Some(Action::CreateNewBranch),
            Mode::ConfirmRemoteBranch { .. } => Some(Action::ResolveRemoteBranch),
            Mode::ConfirmWorktreeDelete(flow) if !flow.in_progress() => {
                Some(Action::ConfirmDeleteWorktree)
            }
            Mode::Loading { .. }
            | Mode::ValidatingNewBranch { .. }
            | Mode::ConfirmWorktreeDelete(_) => None,
        },
        Command::BranchesView => {
            matches!(state.mode, Mode::RepoSelect).then_some(Action::OpenBranches)
        }
        Command::Back => match state.mode {
            Mode::BranchSelect(_) => Some(Action::BackToRepos),
            Mode::SelectBaseBranch { .. }
            | Mode::ConfirmRemoteBranch { .. }
            | Mode::ConfirmWorktreeDelete(_) => Some(Action::CancelOverlay),
            _ => None,
        },
        Command::BackspaceOrBack => match state.mode {
            Mode::BranchSelect(_) if active_query(state).is_empty() => Some(Action::BackToRepos),
            _ => Some(Action::Edit(TextEdit::Backspace)),
        },
        Command::NewBranch => (matches!(state.mode, Mode::BranchSelect(_))
            && !state.branch_view.is_loading())
        .then_some(Action::StartNewBranch),
        Command::Delete => {
            matches!(state.mode, Mode::BranchSelect(_)).then_some(Action::DeleteWorktree)
        }
        Command::Clear => {
            (!active_query(state).is_empty()).then_some(Action::Edit(TextEdit::Clear))
        }
        Command::Backspace
        | Command::DeleteForward
        | Command::DeleteWord
        | Command::DeleteWordForward
        | Command::DeleteToEnd
        | Command::CursorLeft
        | Command::CursorRight
        | Command::CursorWordLeft
        | Command::CursorWordRight
        | Command::CursorStart
        | Command::CursorEnd => command.text_edit().map(Action::Edit),
    }
}

fn active_query(state: &AppState) -> &str {
    match &state.mode {
        Mode::RepoSelect => &state.repo_view.list.input.text,
        Mode::BranchSelect(_) => &state.branch_view.list.input.text,
        Mode::SelectBaseBranch { flow, .. } => &flow.list.input.text,
        Mode::Loading { .. }
        | Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmRemoteBranch { .. }
        | Mode::ConfirmWorktreeDelete(_) => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::TEST_PRODUCT;
    use crate::state::{
        BaseBranchSelection, BranchContext, DeleteFlowState, DeleteWorktreeTarget, HelpBindingRow,
        HelpOverlayState, ScanWarningsOverlayState, SearchableList,
    };

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn question_mark_remains_searchable_and_configured_help_opens_overlay() {
        let state = AppState::new(&TEST_PRODUCT, None);
        let keys = KeysConfig::default();
        assert_eq!(
            resolve_action(key(KeyCode::Char('?'), KeyModifiers::NONE), &state, &keys),
            Some(Action::Insert('?'))
        );
        assert_eq!(
            resolve_action(
                key(KeyCode::Char('h'), KeyModifiers::CONTROL),
                &state,
                &keys
            ),
            Some(Action::ShowHelp)
        );
    }

    #[test]
    fn unicode_text_is_insertable_in_picker_and_branch_name_queries() {
        let keys = KeysConfig::default();
        let mut state = AppState::new(&TEST_PRODUCT, None);
        assert_eq!(
            resolve_action(key(KeyCode::Char('é'), KeyModifiers::NONE), &state, &keys),
            Some(Action::Insert('é'))
        );
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        assert_eq!(
            resolve_action(key(KeyCode::Char('界'), KeyModifiers::NONE), &state, &keys),
            Some(Action::Insert('界'))
        );
    }

    #[test]
    fn configured_command_letters_still_precede_text_insertion() {
        let state = AppState::new(&TEST_PRODUCT, None);
        let keys = KeysConfig::from_toml("[repo_select]\nx = \"branches_view\"").unwrap();
        assert_eq!(
            resolve_action(key(KeyCode::Char('x'), KeyModifiers::NONE), &state, &keys),
            Some(Action::OpenBranches)
        );
    }

    #[test]
    fn help_over_a_dialog_still_edits_its_search() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::ConfirmWorktreeDelete(DeleteFlowState::new(
            BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            DeleteWorktreeTarget::new("feat".into(), "/wt".into(), false),
        ));
        state.overlay = Some(Overlay::Help(HelpOverlayState {
            rows: Vec::new(),
            list: SearchableList::new(0),
        }));
        let keys = KeysConfig::default();
        assert_eq!(
            resolve_action(key(KeyCode::Backspace, KeyModifiers::NONE), &state, &keys),
            Some(Action::Edit(TextEdit::Backspace))
        );
        assert_eq!(
            resolve_action(
                key(KeyCode::Char('u'), KeyModifiers::CONTROL),
                &state,
                &keys
            ),
            Some(Action::Edit(TextEdit::Clear))
        );
        let keys = KeysConfig::from_toml("[modal]\nbackspace = \"noop\"").unwrap();
        assert_eq!(
            resolve_action(key(KeyCode::Backspace, KeyModifiers::NONE), &state, &keys),
            None,
            "the dialog's own binding wins"
        );
    }

    #[test]
    fn default_text_editing_keys_edit_the_search() {
        let keys = KeysConfig::default();
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.repo_view.list.input.text = "feat/login".into();
        let alt = KeyModifiers::ALT;
        let ctrl = KeyModifiers::CONTROL;
        let none = KeyModifiers::NONE;
        for (code, modifiers, edit) in [
            (KeyCode::Char('b'), alt, TextEdit::CursorWordLeft),
            (KeyCode::Left, alt, TextEdit::CursorWordLeft),
            (KeyCode::Left, ctrl, TextEdit::CursorWordLeft),
            (KeyCode::Char('f'), alt, TextEdit::CursorWordRight),
            (KeyCode::Right, alt, TextEdit::CursorWordRight),
            (KeyCode::Right, ctrl, TextEdit::CursorWordRight),
            (KeyCode::Char('b'), ctrl, TextEdit::CursorLeft),
            (KeyCode::Char('f'), ctrl, TextEdit::CursorRight),
            (KeyCode::Home, none, TextEdit::CursorStart),
            (KeyCode::Char('a'), ctrl, TextEdit::CursorStart),
            (KeyCode::End, none, TextEdit::CursorEnd),
            (KeyCode::Char('e'), ctrl, TextEdit::CursorEnd),
            (KeyCode::Delete, none, TextEdit::DeleteForward),
            (KeyCode::Char('d'), ctrl, TextEdit::DeleteForward),
            (KeyCode::Char('d'), alt, TextEdit::DeleteWordForward),
            (KeyCode::Delete, alt, TextEdit::DeleteWordForward),
            (KeyCode::Char('k'), ctrl, TextEdit::DeleteToEnd),
            (KeyCode::Backspace, alt, TextEdit::DeleteWordBackward),
            (KeyCode::Char('w'), ctrl, TextEdit::DeleteWordBackward),
        ] {
            assert_eq!(
                resolve_action(key(code, modifiers), &state, &keys),
                Some(Action::Edit(edit)),
                "{code:?} {modifiers:?}"
            );
        }
        assert_eq!(
            resolve_action(key(KeyCode::F(1), none), &state, &keys),
            Some(Action::ShowHelp)
        );
    }

    #[test]
    fn ctrl_u_clears_the_search_and_esc_leaves_whatever_is_typed() {
        let keys = KeysConfig::default();
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        let ctrl_u = key(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let mut state = AppState::new(&TEST_PRODUCT, None);
        assert_eq!(
            resolve_action(ctrl_u, &state, &keys),
            None,
            "nothing to clear"
        );
        state.repo_view.list.input.text = "al".into();
        assert_eq!(
            resolve_action(ctrl_u, &state, &keys),
            Some(Action::Edit(TextEdit::Clear))
        );
        assert_eq!(resolve_action(esc, &state, &keys), Some(Action::Quit));

        let context = BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        };
        state.mode = Mode::BranchSelect(context.clone());
        state.branch_view.list.input.text = "fe".into();
        assert_eq!(
            resolve_action(ctrl_u, &state, &keys),
            Some(Action::Edit(TextEdit::Clear))
        );
        assert_eq!(
            resolve_action(esc, &state, &keys),
            Some(Action::BackToRepos)
        );

        let mut list = SearchableList::new(1);
        list.input.text = "ma".into();
        state.mode = Mode::SelectBaseBranch {
            context,
            flow: BaseBranchSelection {
                new_name: "feat".into(),
                bases: vec!["main".into()],
                list,
            },
        };
        assert_eq!(
            resolve_action(ctrl_u, &state, &keys),
            Some(Action::Edit(TextEdit::Clear))
        );
        assert_eq!(
            resolve_action(esc, &state, &keys),
            Some(Action::CancelOverlay)
        );
    }

    #[test]
    fn q_is_typed_and_a_letter_bound_to_quit_only_quits_an_empty_search() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        let q = key(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(
            resolve_action(q, &state, &KeysConfig::default()),
            Some(Action::Insert('q')),
            "a search can start with q"
        );

        let keys = KeysConfig::from_toml("[repo_select]\nq = \"quit\"").unwrap();
        assert_eq!(resolve_action(q, &state, &keys), Some(Action::Quit));
        state.repo_view.list.input.text = "a".into();
        assert_eq!(resolve_action(q, &state, &keys), Some(Action::Insert('q')));
    }

    #[test]
    fn ctrl_x_only_deletes_and_a_bound_dismiss_key_wins_while_a_toast_shows() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.push_toast(crate::state::ToastKind::Warning, "notice");
        let chord = key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(
            resolve_action(chord, &state, &KeysConfig::default()),
            Some(Action::DeleteWorktree)
        );
        let keys = KeysConfig::from_toml("[general]\n\"C-g\" = \"dismiss_toast\"").unwrap();
        let ctrl_g = key(KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert_eq!(
            resolve_action(ctrl_g, &state, &keys),
            Some(Action::DismissToast)
        );
    }

    #[test]
    fn new_branch_routes_are_blocked_until_local_branches_finish_loading() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.branch_view.list.input.text = "feat/new".into();
        state.branch_view.list.filtered.clear();
        state.branch_view.set_load(
            state.branch_view.generation(),
            crate::state::BranchLoad::Loading { reselect: None },
        );
        let keys = KeysConfig::default();

        assert_eq!(
            resolve_action(
                key(KeyCode::Char('o'), KeyModifiers::CONTROL),
                &state,
                &keys
            ),
            None
        );
        assert_eq!(
            resolve_action(key(KeyCode::Enter, KeyModifiers::NONE), &state, &keys),
            None
        );

        state.branch_view.set_load(
            state.branch_view.generation(),
            crate::state::BranchLoad::Ready {
                fetching_remotes: false,
            },
        );
        assert_eq!(
            resolve_action(
                key(KeyCode::Char('o'), KeyModifiers::CONTROL),
                &state,
                &keys
            ),
            Some(Action::StartNewBranch)
        );
        assert_eq!(
            resolve_action(key(KeyCode::Enter, KeyModifiers::NONE), &state, &keys),
            Some(Action::StartNewBranch)
        );
    }

    #[test]
    fn existing_modal_defaults_are_preserved() {
        let context = BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        };
        let keys = KeysConfig::default();
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::SelectBaseBranch {
            context: context.clone(),
            flow: BaseBranchSelection {
                new_name: "feat".into(),
                bases: vec!["main".into()],
                list: SearchableList::new(1),
            },
        };
        assert_eq!(
            resolve_action(key(KeyCode::Enter, KeyModifiers::NONE), &state, &keys),
            Some(Action::CreateNewBranch)
        );
        assert_eq!(
            resolve_action(key(KeyCode::Esc, KeyModifiers::NONE), &state, &keys),
            Some(Action::CancelOverlay)
        );
        state.mode = Mode::ConfirmWorktreeDelete(DeleteFlowState::new(
            context,
            DeleteWorktreeTarget::new("feat".into(), "/wt".into(), false),
        ));
        assert_eq!(
            resolve_action(key(KeyCode::Enter, KeyModifiers::NONE), &state, &keys),
            Some(Action::ConfirmDeleteWorktree)
        );
    }

    #[test]
    fn remote_branch_prompt_navigates_confirms_and_cancels_without_text_editing() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::ConfirmRemoteBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            prompt: crate::picker::new_branch::test_remote_prompt("feature", &["origin"], false),
        };
        let keys = KeysConfig::default();
        for (code, modifiers, expected) in [
            (
                KeyCode::Up,
                KeyModifiers::NONE,
                Some(Action::MoveSelection(-1)),
            ),
            (
                KeyCode::Char('n'),
                KeyModifiers::CONTROL,
                Some(Action::MoveSelection(1)),
            ),
            (
                KeyCode::Enter,
                KeyModifiers::NONE,
                Some(Action::ResolveRemoteBranch),
            ),
            (
                KeyCode::Esc,
                KeyModifiers::NONE,
                Some(Action::CancelOverlay),
            ),
            (KeyCode::Char('x'), KeyModifiers::NONE, None),
            (KeyCode::Backspace, KeyModifiers::NONE, None),
        ] {
            assert_eq!(
                resolve_action(key(code, modifiers), &state, &keys),
                expected,
                "{code:?} {modifiers:?}"
            );
        }
    }

    #[test]
    fn base_picker_resolves_all_search_and_navigation_actions() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::SelectBaseBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            flow: BaseBranchSelection {
                new_name: "feat".into(),
                bases: vec!["main".into(), "next".into()],
                list: SearchableList::new(2),
            },
        };
        let keys = KeysConfig::default();
        for (code, modifiers, expected) in [
            (KeyCode::Up, KeyModifiers::NONE, Action::MoveSelection(-1)),
            (KeyCode::Down, KeyModifiers::NONE, Action::MoveSelection(1)),
            (
                KeyCode::Char('p'),
                KeyModifiers::CONTROL,
                Action::MoveSelection(-1),
            ),
            (
                KeyCode::Char('n'),
                KeyModifiers::CONTROL,
                Action::MoveSelection(1),
            ),
            (
                KeyCode::Backspace,
                KeyModifiers::NONE,
                Action::Edit(TextEdit::Backspace),
            ),
            (
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
                Action::Edit(TextEdit::DeleteWordBackward),
            ),
            (
                KeyCode::Left,
                KeyModifiers::NONE,
                Action::Edit(TextEdit::CursorLeft),
            ),
            (
                KeyCode::Right,
                KeyModifiers::NONE,
                Action::Edit(TextEdit::CursorRight),
            ),
        ] {
            assert_eq!(
                resolve_action(key(code, modifiers), &state, &keys),
                Some(expected)
            );
        }
    }

    #[test]
    fn help_overlay_searches_without_triggering_underlying_mode_commands() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.overlay = Some(Overlay::Help(HelpOverlayState {
            rows: vec![HelpBindingRow {
                section_name: "general",
                keys: vec!["ctrl+h".into()],
                command_name: "help",
                description: "Show active key bindings",
            }],
            list: SearchableList::new(1),
        }));
        let keys = KeysConfig::from_toml("[repo_select]\nq = \"quit\"").unwrap();
        assert_eq!(
            resolve_action(key(KeyCode::Char('q'), KeyModifiers::NONE), &state, &keys),
            Some(Action::Insert('q'))
        );
        assert_eq!(
            resolve_action(key(KeyCode::Backspace, KeyModifiers::NONE), &state, &keys),
            Some(Action::Edit(TextEdit::Backspace))
        );
        assert_eq!(
            resolve_action(key(KeyCode::Down, KeyModifiers::NONE), &state, &keys),
            Some(Action::MoveSelection(1))
        );
        assert_eq!(
            resolve_action(key(KeyCode::Esc, KeyModifiers::NONE), &state, &keys),
            Some(Action::CloseOverlay)
        );
    }

    #[test]
    fn scan_warnings_key_only_opens_the_view_when_there_are_warnings() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        let keys = KeysConfig::default();
        let ctrl_s = key(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(resolve_action(ctrl_s, &state, &keys), None);

        state.push_scan_warning(crate::git::ScanWarning {
            path: "/locked".into(),
            message: "permission denied".into(),
        });
        assert_eq!(
            resolve_action(ctrl_s, &state, &keys),
            Some(Action::ShowScanWarnings)
        );
        state.dismiss_toast();
        assert_eq!(
            resolve_action(ctrl_s, &state, &keys),
            Some(Action::ShowScanWarnings)
        );

        let keys =
            KeysConfig::from_toml("[general]\n\"C-s\" = \"noop\"\n\"C-g\" = \"scan_warnings\"")
                .unwrap();
        assert_eq!(resolve_action(ctrl_s, &state, &keys), None);
        assert_eq!(
            resolve_action(
                key(KeyCode::Char('g'), KeyModifiers::CONTROL),
                &state,
                &keys
            ),
            Some(Action::ShowScanWarnings)
        );
    }

    #[test]
    fn scan_warnings_view_scrolls_and_closes_without_reaching_the_picker() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.overlay = Some(Overlay::ScanWarnings(ScanWarningsOverlayState::default()));
        let keys = KeysConfig::default();
        for (code, modifiers, expected) in [
            (KeyCode::Esc, KeyModifiers::NONE, Some(Action::CloseOverlay)),
            (
                KeyCode::Char('s'),
                KeyModifiers::CONTROL,
                Some(Action::CloseOverlay),
            ),
            (
                KeyCode::Up,
                KeyModifiers::NONE,
                Some(Action::MoveSelection(-1)),
            ),
            (
                KeyCode::Char('n'),
                KeyModifiers::CONTROL,
                Some(Action::MoveSelection(1)),
            ),
            (KeyCode::Enter, KeyModifiers::NONE, None),
            (KeyCode::Tab, KeyModifiers::NONE, None),
            (KeyCode::Char('q'), KeyModifiers::NONE, None),
            (KeyCode::Char('x'), KeyModifiers::CONTROL, None),
            (KeyCode::Backspace, KeyModifiers::NONE, None),
        ] {
            assert_eq!(
                resolve_action(key(code, modifiers), &state, &keys),
                expected,
                "{code:?} {modifiers:?}"
            );
        }
    }

    #[test]
    fn the_key_that_opened_scan_warnings_closes_them_despite_a_dialog_binding() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.push_scan_warning(crate::git::ScanWarning {
            path: "/locked".into(),
            message: "denied".into(),
        });
        let keys = KeysConfig::from_toml("[modal]\n\"C-s\" = \"back\"").unwrap();
        let ctrl_s = key(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(
            resolve_action(ctrl_s, &state, &keys),
            Some(Action::ShowScanWarnings)
        );
        state.overlay = Some(Overlay::ScanWarnings(ScanWarningsOverlayState::default()));
        assert_eq!(
            resolve_action(ctrl_s, &state, &keys),
            Some(Action::CloseOverlay)
        );
    }

    #[test]
    fn backspace_or_back_leaves_an_empty_branch_search_for_the_repositories() {
        let keys =
            KeysConfig::from_toml("[branch_select]\nbackspace = \"backspace_or_back\"").unwrap();
        let backspace = key(KeyCode::Backspace, KeyModifiers::NONE);
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        assert_eq!(
            resolve_action(backspace, &state, &keys),
            Some(Action::BackToRepos)
        );
        assert_eq!(
            resolve_action(backspace, &state, &KeysConfig::default()),
            Some(Action::Edit(TextEdit::Backspace)),
            "without the binding, backspace only deletes"
        );
        let mut help = SearchableList::new(0);
        help.input.text = "open".into();
        state.overlay = Some(Overlay::Help(HelpOverlayState {
            rows: Vec::new(),
            list: help,
        }));
        assert_eq!(
            resolve_action(backspace, &state, &keys),
            Some(Action::Edit(TextEdit::Backspace)),
            "help's search over the branches only deletes"
        );
        state.overlay = None;
        state.branch_view.list.input.text = "feat".into();
        assert_eq!(
            resolve_action(backspace, &state, &keys),
            Some(Action::Edit(TextEdit::Backspace))
        );
    }
}
