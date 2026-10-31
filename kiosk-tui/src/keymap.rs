use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use kiosk_core::{
    config::keys::{BindingMode, Command, KeyChord, KeysConfig},
    keyboard::Action,
    state::{AppState, Mode, Overlay},
};

/// The scan warnings view is a scrollable dialog whatever mode it opened over, so it
/// takes its keys from the dialog, navigation and general layers.
pub(crate) const SCAN_WARNINGS_BINDING_MODE: BindingMode = BindingMode::Choice;

const ESC: KeyChord = KeyChord::new(KeyCode::Esc, KeyModifiers::NONE);

pub fn resolve_action(key: KeyEvent, state: &AppState, keys: &KeysConfig) -> Option<Action> {
    let chord = KeyChord::from_event(key);
    match &state.overlay {
        Some(Overlay::Help(_)) => {
            let command = keys.command_for(KeysConfig::mode_for(&state.mode), chord);
            return help_overlay_action(chord, command);
        }
        Some(Overlay::ScanWarnings(_)) => {
            let command = keys.command_for(SCAN_WARNINGS_BINDING_MODE, chord);
            return scan_warnings_overlay_action(chord, command);
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
    if let KeyCode::Char(character) = chord.code
        && !chord
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && !character.is_control()
        && matches!(
            state.mode,
            Mode::RepoSelect | Mode::BranchSelect(_) | Mode::SelectBaseBranch { .. }
        )
    {
        return Some(Action::Insert(character));
    }
    None
}

/// Help owns its own query, so typing filters it and `esc` always closes it.
fn help_overlay_action(chord: KeyChord, command: Option<Command>) -> Option<Action> {
    if chord == ESC {
        return Some(Action::CloseOverlay);
    }
    if let Some(action) = command.and_then(help_command_to_action) {
        return Some(action);
    }
    if let KeyCode::Char(character) = chord.code
        && !chord
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && !character.is_control()
    {
        return Some(Action::Insert(character));
    }
    None
}

/// The scan warnings view only scrolls, and closes with `esc` or the key that opened it.
fn scan_warnings_overlay_action(chord: KeyChord, command: Option<Command>) -> Option<Action> {
    if chord == ESC {
        return Some(Action::CloseOverlay);
    }
    match command? {
        Command::ScanWarnings => Some(Action::CloseOverlay),
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        _ => None,
    }
}

fn help_command_to_action(command: Command) -> Option<Action> {
    match command {
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        Command::Clear => Some(Action::ClearQuery),
        Command::Backspace => Some(Action::Backspace),
        Command::DeleteWord => Some(Action::DeleteWord),
        Command::CursorLeft => Some(Action::CursorLeft),
        Command::CursorRight => Some(Action::CursorRight),
        Command::Noop
        | Command::Quit
        | Command::Help
        | Command::DismissToast
        | Command::ScanWarnings
        | Command::Open
        | Command::BranchesView
        | Command::Back
        | Command::NewBranch
        | Command::Delete => None,
    }
}

fn command_to_action(command: Command, state: &AppState) -> Option<Action> {
    match command {
        Command::Noop => None,
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
            Mode::BranchSelect(_) if state.branch_view.loading => None,
            Mode::BranchSelect(_)
                if !state.branch_view.loading
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
        Command::Back => {
            if active_query(state).is_empty() {
                match state.mode {
                    Mode::BranchSelect(_) => Some(Action::BackToRepos),
                    Mode::SelectBaseBranch { .. }
                    | Mode::ConfirmRemoteBranch { .. }
                    | Mode::ConfirmWorktreeDelete(_) => Some(Action::CancelOverlay),
                    _ => None,
                }
            } else {
                Some(Action::ClearQuery)
            }
        }
        Command::NewBranch => (matches!(state.mode, Mode::BranchSelect(_))
            && !state.branch_view.loading)
            .then_some(Action::StartNewBranch),
        Command::Delete => {
            matches!(state.mode, Mode::BranchSelect(_)).then_some(Action::DeleteWorktree)
        }
        Command::Clear => {
            if !active_query(state).is_empty() {
                Some(Action::ClearQuery)
            } else if matches!(state.mode, Mode::RepoSelect) {
                Some(Action::Quit)
            } else {
                None
            }
        }
        Command::Backspace => Some(Action::Backspace),
        Command::DeleteWord => Some(Action::DeleteWord),
        Command::CursorLeft => Some(Action::CursorLeft),
        Command::CursorRight => Some(Action::CursorRight),
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
    use kiosk_core::product::TEST_PRODUCT;
    use kiosk_core::state::{
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
        let keys = toml::from_str::<KeysConfig>("[repo_select]\nx = \"branches_view\"").unwrap();
        assert_eq!(
            resolve_action(key(KeyCode::Char('x'), KeyModifiers::NONE), &state, &keys),
            Some(Action::OpenBranches)
        );
    }

    #[test]
    fn q_default_is_contextual_and_can_be_unbound() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        let q = key(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(
            resolve_action(q, &state, &KeysConfig::default()),
            Some(Action::Quit)
        );
        state.repo_view.list.input.text = "a".into();
        assert_eq!(
            resolve_action(q, &state, &KeysConfig::default()),
            Some(Action::Insert('q'))
        );

        state.repo_view.list.input.clear();
        let keys = toml::from_str::<KeysConfig>("[repo_select]\nq = \"noop\"").unwrap();
        assert_eq!(resolve_action(q, &state, &keys), None);
    }

    #[test]
    fn toast_dismiss_has_precedence_over_branch_delete() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        let chord = key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(
            resolve_action(chord, &state, &KeysConfig::default()),
            Some(Action::DeleteWorktree)
        );
        state.push_toast(kiosk_core::state::ToastKind::Warning, "notice");
        assert_eq!(
            resolve_action(chord, &state, &KeysConfig::default()),
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
        state.branch_view.loading = true;
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

        state.branch_view.loading = false;
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
            prompt: kiosk_core::picker::new_branch::test_remote_prompt(
                "feature",
                &["origin"],
                false,
            ),
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
            (KeyCode::Backspace, KeyModifiers::NONE, Action::Backspace),
            (
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
                Action::DeleteWord,
            ),
            (KeyCode::Left, KeyModifiers::NONE, Action::CursorLeft),
            (KeyCode::Right, KeyModifiers::NONE, Action::CursorRight),
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
                key_display: "ctrl+h".into(),
                command_name: "help",
                description: "Show active key bindings",
            }],
            list: SearchableList::new(1),
        }));
        let keys = KeysConfig::default();
        assert_eq!(
            resolve_action(key(KeyCode::Char('q'), KeyModifiers::NONE), &state, &keys),
            Some(Action::Insert('q'))
        );
        assert_eq!(
            resolve_action(key(KeyCode::Backspace, KeyModifiers::NONE), &state, &keys),
            Some(Action::Backspace)
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
        let ctrl_e = key(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(resolve_action(ctrl_e, &state, &keys), None);

        state.push_scan_warning(kiosk_core::git::ScanWarning {
            path: "/locked".into(),
            message: "permission denied".into(),
        });
        assert_eq!(
            resolve_action(ctrl_e, &state, &keys),
            Some(Action::ShowScanWarnings)
        );
        state.dismiss_toast();
        assert_eq!(
            resolve_action(ctrl_e, &state, &keys),
            Some(Action::ShowScanWarnings)
        );

        let keys = toml::from_str::<KeysConfig>(
            "[general]\n\"C-e\" = \"noop\"\n\"C-g\" = \"scan_warnings\"",
        )
        .unwrap();
        assert_eq!(resolve_action(ctrl_e, &state, &keys), None);
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
                KeyCode::Char('e'),
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
}
