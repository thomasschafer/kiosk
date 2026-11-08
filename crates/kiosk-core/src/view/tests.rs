use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc},
};

use super::*;
use crate::{
    backend::{Annotations, Tone as LabelTone, mock::MockBackend},
    git::{GitProvider, Repo, ScanWarning, mock::MockGitProvider},
    keyboard::Action,
    picker::{
        FilterWorker, Picker, RunOutcome, branch::OpenCheckouts, new_branch::test_remote_prompt,
        process_action,
    },
    product::TEST_PRODUCT,
    spawn::EventSender,
    state::{
        BaseBranchSelection, BranchCheckout, BranchContext, DeleteWorktreeTarget,
        ScanWarningsOverlayState,
    },
};

mod setup_views;

const VIEWPORTS: Viewports = Viewports {
    list: 3,
    bases: 2,
    help: 4,
    scan_warnings: 2,
};

/// What the picker shows before it has found anything.
pub(super) fn empty_picker_view() -> PickerView {
    picker(
        &mut AppState::new(&TEST_PRODUCT, None),
        &KeysConfig::default(),
        VIEWPORTS,
    )
}

fn repo(name: &str, is_git: bool) -> Repo {
    Repo {
        name: name.into(),
        path: format!("/{name}").into(),
        is_git,
        worktrees: Vec::new(),
    }
}

fn state_with_repos(names: &[&str]) -> AppState {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.repo_view.loading = false;
    state.repo_view.entries = names
        .iter()
        .map(|name| RepoEntry::new(repo(name, true)))
        .collect();
    state.repo_view.list = SearchableList::new(names.len());
    state
}

fn context() -> BranchContext {
    BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    }
}

fn state_with_branches(branches: Vec<BranchEntry>) -> AppState {
    let mut state = state_with_repos(&["repo"]);
    state.mode = Mode::BranchSelect(context());
    state.branch_view.list = SearchableList::new(branches.len());
    state.branch_view.entries = branches;
    state
}

fn list(view: &PickerView) -> &ListView {
    match &view.main {
        Main::List(list) => list,
        Main::Loading { .. } => panic!("expected a list, got {view:?}"),
    }
}

/// A row as a terminal writes it.
fn texts(row: &Row) -> Vec<(String, Tone)> {
    row.segments()
        .into_iter()
        .map(|segment| (segment.text, segment.tone))
        .collect()
}

fn expected(segments: &[(&str, Tone)]) -> Vec<(String, Tone)> {
    segments
        .iter()
        .map(|(text, tone)| ((*text).to_owned(), *tone))
        .collect()
}

fn hints(hints: &[Hint]) -> Vec<(&str, &str)> {
    hints
        .iter()
        .map(|hint| (hint.key.as_str(), hint.label))
        .collect()
}

fn keys(toml: &str) -> KeysConfig {
    KeysConfig::from_toml(toml).unwrap()
}

#[test]
fn repo_rows_are_windowed_to_the_viewport_and_keep_the_selection_in_view() {
    let mut state = state_with_repos(&["a", "b", "c", "d", "e"]);
    state.repo_view.list.selected = Some(4);
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let list = list(&view);
    assert_eq!(list.kind, ListKind::Repos);
    assert_eq!(list.title, "kiosk-test — select repo");
    assert_eq!(list.placeholder, REPO_PLACEHOLDER);
    assert_eq!(list.summary, "5 of 5 repos");
    assert_eq!(list.activity, None);
    assert_eq!(list.offset, 2);
    assert_eq!(list.total, 5);
    assert_eq!(
        list.rows.iter().map(|row| row.position).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    for row in &list.rows {
        assert_eq!(
            item_id(&state, row.position).as_deref(),
            Some(row.id.as_str())
        );
    }
    assert_eq!(list.selected, Some(2));
    assert_eq!(list.empty, None);
    assert_eq!(state.repo_view.list.scroll_offset, 2);
}

#[test]
fn repo_rows_mark_folders_labels_open_state_and_which_have_branches() {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.repo_view.loading = false;
    let mut open = RepoEntry::new(repo("git", true));
    open.is_open = true;
    state.repo_view.entries = vec![open, RepoEntry::new(repo("folder", false))];
    state.repo_view.list = SearchableList::new(2);
    state.annotations = AnnotationIndex::new(Annotations {
        repos: vec![(
            "/git".into(),
            vec![Label {
                text: "claude\nwaiting".into(),
                tone: LabelTone::Attention,
            }],
        )],
        checkouts: Vec::new(),
    });
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let rows = &list(&view).rows;
    assert_eq!(texts(&rows[0]), expected(&[("git", Tone::Normal)]));
    assert_eq!(
        rows[0].labels,
        [Segment::new("claude\\nwaiting", Tone::Attention)]
    );
    assert!(rows[0].open);
    assert!(rows[0].actions.branches);
    assert_eq!(
        texts(&rows[1]),
        expected(&[("folder", Tone::Normal), (FOLDER_SUFFIX, Tone::Muted)])
    );
    assert_eq!(rows[1].labels, Vec::<Segment>::new());
    assert!(!rows[1].open);
    assert!(!rows[1].actions.branches);
    assert!(!rows[0].actions.delete && !rows[1].actions.delete);
}

#[test]
fn repo_list_explains_why_it_is_empty() {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).empty.as_deref(), Some(DISCOVERING_REPOS));
    assert_eq!(list(&view).activity, Some(SCANNING));

    state.repo_view.loading = false;
    for (missing, message) in [
        (0, "No repos found in the configured search directories"),
        (1, "No repos found (1 optional search directory not found)"),
        (
            3,
            "No repos found (3 optional search directories not found)",
        ),
    ] {
        state.repo_view.missing_optional_dirs = missing;
        let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
        assert_eq!(list(&view).empty.as_deref(), Some(message));
    }

    let mut state = state_with_repos(&["a"]);
    state.repo_view.list.filtered.clear();
    state.repo_view.list.selected = None;
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).empty.as_deref(), Some(NO_MATCHING_REPOS));
    assert_eq!(list(&view).summary, "0 of 1 repos");
}

#[test]
fn repo_list_centres_the_repository_it_preselects_then_scrolls_as_usual() {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
    let mut state = state_with_repos(&names);
    let viewports = Viewports {
        list: 5,
        ..VIEWPORTS
    };
    let offset =
        |state: &mut AppState| list(&picker(state, &KeysConfig::default(), viewports)).offset;
    // As the preselection leaves it: the repository the person is in, untouched.
    state.repo_view.list.selected = Some(7);
    assert_eq!(offset(&mut state), 5, "h in the middle of f to j");
    state.repo_view.list.selected = Some(10);
    assert_eq!(offset(&mut state), 7, "k too near the end to centre");

    state.repo_view.list.selected = Some(7);
    offset(&mut state);
    crate::picker::repo::move_selection(&mut state, 1);
    assert_eq!(offset(&mut state), 5, "moving within view doesn't scroll");
    crate::picker::repo::move_selection(&mut state, 2);
    assert_eq!(offset(&mut state), 6, "moving past the end scrolls one row");
}

#[test]
fn repo_list_keeps_rows_while_scanning_and_says_so() {
    let mut state = state_with_repos(&["a"]);
    state.repo_view.loading = true;
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).activity, Some(SCANNING));
    assert_eq!(list(&view).empty, None);
    assert_eq!(list(&view).rows.len(), 1);
}

#[test]
fn branch_rows_show_suffixes_in_their_tones_and_offer_delete_only_for_linked_local_checkouts() {
    let mut state = state_with_branches(vec![
        BranchEntry {
            checkout: Some(BranchCheckout {
                path: "/repo".into(),
                is_open: true,
            }),
            is_current: true,
            is_default: true,
            ..BranchEntry::local("main")
        },
        BranchEntry::local("plain"),
        BranchEntry {
            remote: Some("origin".into()),
            checkout: Some(BranchCheckout::closed("/elsewhere".into())),
            ..BranchEntry::local("feature")
        },
        BranchEntry {
            checkout: Some(BranchCheckout::closed("/worktrees/repo/linked".into())),
            ..BranchEntry::local("linked")
        },
    ]);
    state.annotations = AnnotationIndex::new(Annotations {
        repos: Vec::new(),
        checkouts: vec![(
            "/repo".into(),
            vec![Label {
                text: "codex working".into(),
                tone: LabelTone::Progress,
            }],
        )],
    });
    let view = picker(
        &mut state,
        &KeysConfig::default(),
        Viewports {
            list: 4,
            ..VIEWPORTS
        },
    );
    let list = list(&view);
    assert_eq!(list.kind, ListKind::Branches);
    assert_eq!(list.title, "repo — select branch");
    assert_eq!(list.placeholder, BRANCH_PLACEHOLDER);
    assert_eq!(list.summary, "4 of 4 branches");
    let rows = &list.rows;
    assert_eq!(
        texts(&rows[0]),
        expected(&[
            ("main", Tone::Normal),
            (WORKTREE_SUFFIX, Tone::Secondary),
            (CURRENT_SUFFIX, Tone::Secondary),
            (DEFAULT_SUFFIX, Tone::Muted),
        ])
    );
    assert_eq!(
        rows[0].labels,
        [Segment::new("codex working", Tone::Progress)]
    );
    assert!(rows[0].open);
    assert!(
        !rows[0].actions.delete,
        "the repository's own checkout can't be removed"
    );
    assert_eq!(texts(&rows[1]), expected(&[("plain", Tone::Normal)]));
    assert!(!rows[1].actions.delete);
    assert_eq!(
        texts(&rows[2]),
        expected(&[("origin/feature", Tone::Faint)])
    );
    assert!(!rows[2].actions.delete);
    assert_eq!(
        texts(&rows[3]),
        expected(&[("linked", Tone::Normal), (WORKTREE_SUFFIX, Tone::Secondary)])
    );
    assert!(rows[3].actions.delete);
    assert!(rows.iter().all(|row| !row.actions.branches));
}

#[test]
fn rows_name_their_items_and_say_what_else_they_are() {
    let mut state = state_with_branches(vec![
        BranchEntry {
            checkout: Some(BranchCheckout::closed("/worktrees/repo/feature".into())),
            is_current: true,
            ..BranchEntry::local("feature")
        },
        BranchEntry {
            remote: Some("origin".into()),
            ..BranchEntry::local("review")
        },
    ]);
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let list = list(&view);
    assert_eq!(list.subject.as_deref(), Some("repo"));
    assert_eq!(list.rows[0].name, Segment::new("feature", Tone::Normal));
    assert_eq!(list.rows[0].badges, [Badge::Worktree, Badge::Current]);
    let mut state = state_with_branches(vec![BranchEntry {
        checkout: Some(BranchCheckout::closed("/repo".into())),
        ..BranchEntry::local("main")
    }]);
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(
        self::list(&view).rows[0].badges,
        [Badge::MainCheckout],
        "the repository's own checkout is not a linked worktree"
    );
    assert_eq!(list.rows[1].name, Segment::new("review", Tone::Faint));
    assert_eq!(
        list.rows[1].badges,
        [Badge::Remote {
            name: "origin".into()
        }]
    );
    let serialized = serde_json::to_value(&list.rows[1].badges).unwrap();
    assert_eq!(
        serialized,
        serde_json::json!([{ "kind": "remote", "name": "origin" }])
    );
}

#[test]
fn branch_list_reports_loading_then_fetching() {
    let mut state = state_with_branches(Vec::new());
    state.branch_view.set_load(
        state.branch_view.generation(),
        BranchLoad::Loading { reselect: None },
    );
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).activity, Some("loading"));
    assert_eq!(list(&view).empty.as_deref(), Some(LOADING_BRANCHES));

    let mut state = state_with_branches(vec![BranchEntry::local("main")]);
    state.branch_view.set_load(
        state.branch_view.generation(),
        BranchLoad::Ready {
            fetching_remotes: true,
        },
    );
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).activity, Some("fetching"));
    assert_eq!(list(&view).empty, None);
}

#[test]
fn unmatched_branch_query_says_the_configured_open_key_creates_it() {
    let mut state = state_with_branches(vec![BranchEntry::local("main")]);
    state.branch_view.list.input.text = "new\tname".into();
    state.branch_view.list.input.cursor = 3;
    state.branch_view.list.filtered.clear();
    state.branch_view.list.selected = None;
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(
        list(&view).empty.as_deref(),
        Some("No matching branches. enter creates new\\tname")
    );
    let view = picker(
        &mut state,
        &keys("[branch_select]\nenter = \"noop\"\n\"C-y\" = \"open\""),
        VIEWPORTS,
    );
    assert_eq!(
        list(&view).empty.as_deref(),
        Some("No matching branches. ctrl+y creates new\\tname")
    );
    let view = picker(
        &mut state,
        &keys("[branch_select]\nenter = \"noop\""),
        VIEWPORTS,
    );
    assert_eq!(list(&view).empty, None);
}

#[test]
fn query_splits_at_the_cursor_and_moves_a_cursor_inside_a_character_before_it() {
    assert_eq!(
        Query::from_parts("aé界b", 4),
        Query {
            before: "aé".into(),
            after: "界b".into(),
        }
    );
    assert_eq!(
        Query::from_parts("ab", 99),
        Query {
            before: "ab".into(),
            after: String::new(),
        }
    );
    assert_eq!(
        Query::from_parts("a\nb", 1),
        Query {
            before: "a".into(),
            after: "\\nb".into(),
        }
    );
    assert!(Query::from_parts("", 0).is_empty());
}

#[test]
fn validating_and_base_branch_dialogs() {
    let mut state = state_with_branches(vec![BranchEntry::local("main")]);
    state.mode = Mode::ValidatingNewBranch {
        context: context(),
        name: "feat/new".into(),
    };
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(
        view.dialog,
        Some(Dialog::ValidatingBranch {
            title: "New branch \"feat/new\"".into(),
            message: VALIDATING_BRANCH,
        })
    );
    assert_eq!(list(&view).kind, ListKind::Branches);
    assert_eq!(hints(&view.hints), [("ctrl+c", "quit")]);

    let mut flow = BaseBranchSelection {
        new_name: "feat/new".into(),
        bases: vec!["main".into(), "dev".into(), "release".into()],
        list: SearchableList::new(3),
    };
    flow.list.selected = Some(2);
    state.mode = Mode::SelectBaseBranch {
        context: context(),
        flow,
    };
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let Some(Dialog::BaseBranch { title, list }) = view.dialog else {
        panic!("expected the base branch dialog, got {:?}", view.dialog);
    };
    assert_eq!(title, "New branch \"feat/new\" — pick base");
    assert_eq!(list.kind, ListKind::Bases);
    assert_eq!(list.title, BASE_BRANCH_TITLE);
    assert_eq!(list.placeholder, BASE_BRANCH_PLACEHOLDER);
    assert_eq!(list.summary, "3 bases");
    assert_eq!(
        list.rows.iter().map(|row| row.position).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(list.selected, Some(1));
    assert_eq!(texts(&list.rows[1]), expected(&[("release", Tone::Normal)]));
    assert_eq!(list.empty, None);
    assert_eq!(
        hints(&view.hints),
        [
            ("enter", "create"),
            ("esc", "back"),
            ("ctrl+h", "help"),
            ("ctrl+c", "quit"),
        ]
    );

    let Mode::SelectBaseBranch { flow, .. } = &mut state.mode else {
        unreachable!()
    };
    flow.list.filtered.clear();
    flow.list.selected = None;
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(
        hints(&view.hints),
        [("esc", "back"), ("ctrl+h", "help"), ("ctrl+c", "quit")],
        "with no base, there is nothing to create the branch from"
    );
    let Some(Dialog::BaseBranch { list, .. }) = view.dialog else {
        unreachable!()
    };
    assert_eq!(list.empty.as_deref(), Some(NO_MATCHING_BASES));
}

fn remote_dialog(remotes: &[&str], pending: bool) -> Dialog {
    remote_branch_dialog(
        std::path::Path::new("/repo"),
        &test_remote_prompt("feat/x", remotes, pending),
        &KeysConfig::default(),
    )
}

#[test]
fn remote_branch_dialog_describes_what_the_remotes_have() {
    let Dialog::RemoteBranch {
        title,
        lines,
        choices,
        selected,
        hints: dialog_hints,
    } = remote_dialog(&["origin-a", "origin-b"], false)
    else {
        unreachable!()
    };
    assert_eq!(title, "Branch exists on remote");
    assert_eq!(lines, ["feat/x already exists on 2 remotes."]);
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        [
            "Track origin-a/feat/x",
            "Track origin-b/feat/x",
            "Create new branch feat/x"
        ]
    );
    assert_eq!(selected, 0);
    assert_eq!(
        hints(&dialog_hints),
        [("↑/↓", "move"), ("enter", "confirm"), ("esc", "cancel")]
    );

    let Dialog::RemoteBranch { title, lines, .. } = remote_dialog(&["origin"], true) else {
        unreachable!()
    };
    assert_eq!(title, "Branch exists on remote");
    assert_eq!(
        lines,
        [
            "feat/x already exists on a remote.",
            "Other remotes are still loading."
        ]
    );

    let Dialog::RemoteBranch {
        title,
        lines,
        choices,
        ..
    } = remote_dialog(&[], true)
    else {
        unreachable!()
    };
    assert_eq!(title, "Remotes still loading");
    assert_eq!(
        lines,
        ["Remotes are still loading, so feat/x may already exist on one."]
    );
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        ["Create new branch feat/x"]
    );

    let Dialog::RemoteBranch { title, lines, .. } = remote_dialog(&[], false) else {
        unreachable!()
    };
    assert_eq!(title, "New branch");
    assert_eq!(lines, ["No remote has a branch named feat/x."]);
}

#[test]
fn remote_branch_dialog_is_shown_over_the_branches() {
    let mut state = state_with_branches(vec![BranchEntry::local("main")]);
    state.mode = Mode::ConfirmRemoteBranch {
        context: context(),
        prompt: test_remote_prompt("feat/x", &["origin"], false),
    };
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert!(matches!(view.dialog, Some(Dialog::RemoteBranch { .. })));
    assert_eq!(list(&view).kind, ListKind::Branches);
}

fn delete_view(is_open: bool, force: bool, in_progress: bool) -> Dialog {
    let mut target = DeleteWorktreeTarget::new("feature".into(), "/repo-feature".into(), is_open);
    target.force = force;
    target.in_progress = in_progress;
    delete_dialog(
        &DeleteFlowState::new(context(), target),
        "Herdr",
        &KeysConfig::default(),
    )
}

#[test]
fn delete_dialog_warns_about_changes_and_closing_and_hides_hints_while_removing() {
    assert_eq!(
        delete_view(false, false, false),
        Dialog::ConfirmDelete {
            title: CONFIRM_DELETE_TITLE,
            warning: None,
            lines: vec!["Remove checkout /repo-feature?".into()],
            path: "/repo-feature".into(),
            force: false,
            closes_in: None,
            busy: None,
            hints: vec![Hint::new("enter", "remove"), Hint::new("esc", "cancel")],
        }
    );
    let Dialog::ConfirmDelete {
        warning,
        lines,
        force,
        closes_in,
        busy,
        ..
    } = delete_view(true, true, true)
    else {
        unreachable!()
    };
    assert_eq!(warning, Some(UNCOMMITTED_CHANGES));
    assert!(force);
    assert_eq!(closes_in.as_deref(), Some("Herdr"));
    assert_eq!(
        lines,
        [
            "Force-remove /repo-feature?",
            "It will also be closed in Herdr."
        ]
    );
    assert_eq!(busy, Some(REMOVING_CHECKOUT));
}

#[test]
fn toast_shows_the_front_notification_with_its_queue_and_hints() {
    let mut state = state_with_repos(&["a"]);
    assert_eq!(toast(&state, &KeysConfig::default()), None);
    state.push_toast(ToastKind::Error, "failed badly");
    state.push_toast(ToastKind::Warning, "second");
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(
        view.toast,
        Some(ToastView {
            level: ToastLevel::Error,
            label: "Error",
            message: "failed badly".into(),
            counter: Some("1/2".into()),
            hints: Vec::new(),
        })
    );
    assert_eq!(ToastLevel::Error.tone(), Tone::Error);
    assert_eq!(ToastLevel::Warning.tone(), Tone::Warning);
    assert_eq!(toast_label(ToastKind::Warning), "Warning");
}

#[test]
fn toast_counter_only_appears_for_pending_toasts() {
    assert_eq!(toast_counter(0), None);
    assert_eq!(toast_counter(1), None);
    assert_eq!(toast_counter(3).as_deref(), Some("1/3"));
}

#[test]
fn dismiss_hint_uses_the_effective_remapped_binding() {
    let keys = keys("[general]\n\"C-g\" = \"dismiss_toast\"");
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.push_toast(ToastKind::Error, "failed");
    assert_eq!(
        hints(&toast_hints(&keys, state.toasts.front().unwrap())),
        [("ctrl+g", "dismiss")]
    );
}

#[test]
fn scan_summary_names_the_details_key_unless_it_is_unbound() {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.push_scan_warning(ScanWarning {
        path: "/locked".into(),
        message: "denied".into(),
    });
    let toast = state.toasts.front().unwrap();
    assert_eq!(
        hints(&toast_hints(&KeysConfig::default(), toast)),
        [("ctrl+s", "details")]
    );
    assert_eq!(
        hints(&toast_hints(&keys("[general]\n\"C-s\" = \"noop\""), toast)),
        []
    );
}

fn help_state(toast: bool) -> AppState {
    let mut state = state_with_repos(&["a"]);
    if toast {
        state.push_toast(ToastKind::Warning, "careful");
    }
    state.overlay = Some(Overlay::Help(HelpOverlayState::for_mode(
        &KeysConfig::default(),
        BindingMode::Repo,
    )));
    state
}

fn help(view: &PickerView) -> &HelpView {
    match &view.overlay {
        Some(OverlayView::Help(help)) => help,
        other => panic!("expected help, got {other:?}"),
    }
}

#[test]
fn help_lists_bindings_under_section_headings_with_gaps_between() {
    let mut state = help_state(false);
    let Some(Overlay::Help(overlay)) = &state.overlay else {
        unreachable!()
    };
    let entries = help_entries(overlay);
    assert_eq!(entries[0], HelpEntry::Section { name: "repository" });
    assert!(matches!(entries[1], HelpEntry::Binding { position: 0, .. }));
    let gap = entries
        .iter()
        .position(|entry| *entry == HelpEntry::Blank)
        .unwrap();
    assert!(matches!(entries[gap + 1], HelpEntry::Section { .. }));
    let bindings = entries
        .iter()
        .filter(|entry| matches!(entry, HelpEntry::Binding { .. }))
        .count();
    assert_eq!(bindings, overlay.list.filtered.len());

    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let help = help(&view);
    assert_eq!(help.title, HELP_TITLE);
    assert_eq!(help.placeholder, HELP_PLACEHOLDER);
    assert_eq!(help.notice, None);
    assert_eq!(help.summary, format!("{bindings} commands (esc: close)"));
    assert_eq!(help.entries.len(), VIEWPORTS.help);
    assert_eq!(help.entries[..2], entries[..2]);
    assert_eq!(help.selected, Some(1));
    assert_eq!((help.offset, help.total), (0, entries.len()));
    assert_eq!(help.empty, None);
}

#[test]
fn help_lists_each_command_once_with_all_its_keys_so_each_move_changes_command() {
    let mut state = help_state(false);
    let Some(Overlay::Help(overlay)) = &mut state.overlay else {
        unreachable!()
    };
    let move_down = overlay
        .rows
        .iter()
        .find(|row| row.command_name == "move_down")
        .unwrap();
    assert_eq!(move_down.keys, ["↓", "ctrl+n"]);
    let mut seen = std::collections::HashSet::new();
    for row in &overlay.rows {
        assert!(!row.keys.is_empty(), "{row:?}");
        assert!(
            seen.insert((row.section_name, row.command_name)),
            "{row:?} is listed twice"
        );
    }

    let mut previous = None;
    for _ in 0..overlay.rows.len() - 1 {
        let entries = help_entries(overlay);
        let selected = scroll_help(overlay, &entries, VIEWPORTS.help).unwrap();
        let HelpEntry::Binding { command, .. } = &entries[selected] else {
            panic!("selected {:?}", entries[selected]);
        };
        assert_ne!(previous, Some(*command));
        previous = Some(*command);
        overlay.list.move_selection(1);
    }
}

#[test]
fn help_scrolls_its_selection_into_view_counting_headings() {
    let mut state = help_state(true);
    let Some(Overlay::Help(overlay)) = &mut state.overlay else {
        unreachable!()
    };
    let last = overlay.list.filtered.len() - 1;
    overlay.list.selected = Some(last);
    let entries = help_entries(overlay);
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let help = help(&view);
    assert_eq!(help.notice, Some(HELP_TOAST_NOTICE));
    assert_eq!(help.entries.len(), VIEWPORTS.help);
    assert_eq!(help.entries[..], entries[entries.len() - VIEWPORTS.help..]);
    assert_eq!(
        (help.offset, help.total),
        (entries.len() - VIEWPORTS.help, entries.len())
    );
    assert_eq!(help.selected, Some(VIEWPORTS.help - 1));
    assert!(matches!(
        help.entries[VIEWPORTS.help - 1],
        HelpEntry::Binding { position, .. } if position == last
    ));
}

#[test]
fn help_says_when_nothing_matches() {
    let mut state = help_state(false);
    let Some(Overlay::Help(overlay)) = &mut state.overlay else {
        unreachable!()
    };
    overlay.list.filtered.clear();
    overlay.list.selected = None;
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let help = help(&view);
    assert_eq!(help.empty, Some(NO_MATCHING_BINDINGS));
    assert_eq!(help.entries, Vec::<HelpEntry>::new());
    assert_eq!(help.selected, None);
    assert_eq!(help.summary, "0 commands (esc: close)");
}

#[test]
fn scan_warnings_are_windowed_and_their_offset_clamped() {
    let mut state = state_with_repos(&["a"]);
    for index in 0..5 {
        state.push_scan_warning(ScanWarning {
            path: PathBuf::from(format!("/locked{index}")),
            message: "denied\n".into(),
        });
    }
    state.overlay = Some(Overlay::ScanWarnings(ScanWarningsOverlayState {
        scroll_offset: 99,
    }));
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let Some(OverlayView::ScanWarnings(warnings)) = &view.overlay else {
        panic!("expected scan warnings, got {:?}", view.overlay);
    };
    assert_eq!(warnings.title, scan_summary(5));
    assert_eq!(warnings.offset, 3);
    assert_eq!(warnings.total, 5);
    assert_eq!(
        warnings.warnings,
        [
            ScanWarningView {
                path: "/locked3".into(),
                message: "denied\\n".into(),
            },
            ScanWarningView {
                path: "/locked4".into(),
                message: "denied\\n".into(),
            },
        ]
    );
    assert_eq!(
        hints(&warnings.hints),
        [("↑/↓", "scroll"), ("esc", "close")]
    );
    assert!(matches!(
        state.overlay,
        Some(Overlay::ScanWarnings(ScanWarningsOverlayState {
            scroll_offset: 3
        }))
    ));
}

#[test]
fn scan_warning_hints_name_a_lone_scroll_direction() {
    let keys = keys("[list_navigation]\nup = \"noop\"\n\"C-p\" = \"noop\"");
    assert_eq!(
        hints(&scan_warnings_hints(&keys)),
        [("↓", "scroll down"), ("esc", "close")]
    );
}

#[test]
fn footer_hints_follow_the_mode() {
    let keys = KeysConfig::default();
    let footer = |state: &AppState| footer_hints(&keys, state);
    assert_eq!(
        hints(&footer(&state_with_repos(&["a"]))),
        [
            ("enter", "open"),
            ("tab", "branches"),
            ("ctrl+h", "help"),
            ("esc", "quit"),
        ]
    );
    assert_eq!(
        hints(&footer(&loaded(state_with_branches(vec![linked(
            "feature"
        )])))),
        [
            ("enter", "open"),
            ("ctrl+o", "new"),
            ("ctrl+x", "delete"),
            ("esc", "back"),
            ("ctrl+h", "help"),
            ("ctrl+c", "quit"),
        ]
    );
    let mut state = state_with_repos(&["a"]);
    state.mode = Mode::ConfirmRemoteBranch {
        context: context(),
        prompt: test_remote_prompt("x", &[], false),
    };
    assert_eq!(
        hints(&footer(&state)),
        [
            ("enter", "confirm"),
            ("esc", "back"),
            ("ctrl+h", "help"),
            ("ctrl+c", "quit"),
        ]
    );
    state.mode = Mode::ConfirmWorktreeDelete(DeleteFlowState::new(
        context(),
        DeleteWorktreeTarget::new("feature".into(), "/repo-feature".into(), false),
    ));
    assert_eq!(hints(&footer(&state))[0], ("enter", "remove"));
    state.mode = Mode::Loading {
        message: "Opening".into(),
        branch: None,
    };
    assert_eq!(hints(&footer(&state)), [("ctrl+c", "quit")]);
}

fn linked(name: &str) -> BranchEntry {
    BranchEntry {
        checkout: Some(BranchCheckout::closed(
            format!("/worktrees/repo/{name}").into(),
        )),
        ..BranchEntry::local(name)
    }
}

/// The branch list once it knows which checkouts are open, which removing one waits for.
fn loaded(mut state: AppState) -> AppState {
    state
        .branch_view
        .set_open_checkouts(OpenCheckouts::Loaded(Vec::new()));
    state
}

fn nothing_matches(list: &mut SearchableList, query: &str) {
    list.input.text = query.into();
    list.input.cursor = query.len();
    list.filtered.clear();
    list.selected = None;
}

#[test]
fn footer_hints_offer_only_what_the_selection_can_do() {
    let defaults = KeysConfig::default();
    let footer = |state: &AppState| footer_hints(&defaults, state);

    let mut state = state_with_repos(&["a"]);
    state.repo_view.entries = vec![RepoEntry::new(repo("notes", false))];
    assert_eq!(
        hints(&footer(&state)),
        [("enter", "open"), ("ctrl+h", "help"), ("esc", "quit")],
        "a plain folder has no branches"
    );
    nothing_matches(&mut state.repo_view.list, "zzz");
    assert_eq!(
        hints(&footer(&state)),
        [("ctrl+h", "help"), ("esc", "quit")],
        "nothing to open"
    );

    let state = state_with_branches(vec![linked("feature")]);
    assert!(
        !hints(&footer(&state)).contains(&("ctrl+x", "delete")),
        "removing waits until it is known whether the checkout is open"
    );

    let mut state = loaded(state_with_branches(vec![BranchEntry::local("plain")]));
    assert_eq!(
        hints(&footer(&state)),
        [
            ("enter", "open"),
            ("ctrl+o", "new"),
            ("esc", "back"),
            ("ctrl+h", "help"),
            ("ctrl+c", "quit"),
        ],
        "a branch without a checkout of its own has nothing to remove"
    );
    nothing_matches(&mut state.branch_view.list, "new-thing");
    assert_eq!(
        hints(&footer(&state)),
        [
            ("enter", "create"),
            ("ctrl+o", "new"),
            ("esc", "back"),
            ("ctrl+h", "help"),
            ("ctrl+c", "quit"),
        ]
    );
    assert_eq!(
        hints(&footer_hints(
            &keys("[branch_select]\nenter = \"noop\""),
            &state
        )),
        [
            ("ctrl+o", "new"),
            ("esc", "back"),
            ("ctrl+h", "help"),
            ("ctrl+c", "quit"),
        ],
        "with nothing bound to open, nothing creates the branch"
    );
}

#[test]
fn rows_mark_where_the_search_matched_their_names() {
    let mut state = state_with_repos(&["docs", "tab\there"]);
    state.repo_view.list.input.text = "oc".into();
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).rows[0].matches, [1, 2]);
    assert_eq!(list(&view).rows[1].matches, Vec::<usize>::new());

    // Shown as `tab\there`: the escape moves the characters after it.
    state.repo_view.list.input.text = "bh".into();
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).rows[1].name.text, "tab\\there");
    assert_eq!(list(&view).rows[1].matches, [2, 5]);

    state.repo_view.list.input.text = String::new();
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert!(list(&view).rows.iter().all(|row| row.matches.is_empty()));

    // A remote's branch is searched as `origin/feature` but named `feature`.
    let mut state = state_with_branches(vec![BranchEntry {
        remote: Some("origin".into()),
        ..BranchEntry::local("feature")
    }]);
    state.branch_view.list.input.text = "orfe".into();
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(list(&view).rows[0].name.text, "feature");
    assert_eq!(list(&view).rows[0].matches, [0, 1]);
}

#[test]
fn loading_shows_its_message_and_how_to_close() {
    let mut state = state_with_repos(&["a"]);
    state.mode = Mode::Loading {
        message: "Opening a\u{1b}…".into(),
        branch: None,
    };
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    assert_eq!(
        view.main,
        Main::Loading {
            message: "Opening a\\u{1b}…".into(),
            hint: Some("ctrl+c to close (operation continues)".into()),
        }
    );
    assert_eq!(view.dialog, None);
    assert_eq!(loading_hint(&keys("[general]\n\"C-c\" = \"noop\"")), None);
}

#[test]
fn every_bound_chord_is_listed_once_and_noop_bindings_are_left_out() {
    let names = KeysConfig::default()
        .all_chords()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(names.contains(&"esc".to_string()));
    assert_eq!(
        names.iter().filter(|name| *name == "ctrl+x").count(),
        1,
        "ctrl+x is bound in two layers"
    );
    let keys = keys("[branch_select]\n\"C-o\" = \"noop\"");
    assert!(
        !keys
            .all_chords()
            .iter()
            .any(|chord| chord.to_string() == "ctrl+o")
    );
}

#[test]
fn list_tones_are_the_views_colours() {
    assert_eq!(ListKind::Repos.tone(), Tone::Accent);
    assert_eq!(ListKind::Branches.tone(), Tone::Secondary);
    assert_eq!(ListKind::Bases.tone(), Tone::Secondary);
}

#[test]
fn select_and_scroll_keep_the_selection_on_screen() {
    let mut list = SearchableList::new(10);
    list.select(4);
    assert_eq!(list.selected, Some(4));
    list.select(10);
    assert_eq!(list.selected, Some(4));

    list.scroll_by(5, 3);
    assert_eq!((list.scroll_offset, list.selected), (5, Some(5)));
    list.scroll_by(99, 3);
    assert_eq!((list.scroll_offset, list.selected), (7, Some(7)));
    list.select(9);
    list.scroll_by(-2, 3);
    assert_eq!((list.scroll_offset, list.selected), (5, Some(7)));
    list.scroll_by(-99, 3);
    assert_eq!((list.scroll_offset, list.selected), (0, Some(2)));

    let mut empty = SearchableList::new(0);
    empty.scroll_offset = 3;
    empty.scroll_by(1, 3);
    assert_eq!((empty.scroll_offset, empty.selected), (0, None));

    let mut short = SearchableList::new(2);
    short.scroll_by(5, 10);
    assert_eq!((short.scroll_offset, short.selected), (0, Some(0)));
}

fn run(state: &mut AppState, action: Action) -> Option<RunOutcome> {
    let (tx, _rx) = mpsc::channel();
    let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
    let worker = FilterWorker::spawn(sender.clone());
    let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
    process_action::<MockBackend>(
        action,
        state,
        &git,
        None,
        &sender,
        &worker,
        &KeysConfig::default(),
    )
}

#[test]
fn select_and_scroll_actions_apply_to_the_list_in_front() {
    let mut state = state_with_repos(&["a", "b", "c", "d"]);
    assert_eq!(run(&mut state, Action::Select(2)), None);
    assert_eq!(state.repo_view.list.selected, Some(2));
    assert!(state.repo_view.selection_touched);
    run(
        &mut state,
        Action::Scroll {
            delta: 2,
            viewport_rows: 2,
        },
    );
    assert_eq!(state.repo_view.list.scroll_offset, 2);
    assert_eq!(state.repo_view.list.selected, Some(2));

    let mut state = state_with_branches(vec![BranchEntry::local("a"), BranchEntry::local("b")]);
    run(&mut state, Action::Select(1));
    assert_eq!(state.branch_view.list.selected, Some(1));

    state.mode = Mode::ConfirmRemoteBranch {
        context: context(),
        prompt: test_remote_prompt("x", &["origin"], false),
    };
    run(&mut state, Action::Select(1));
    run(&mut state, Action::Select(5));
    let Mode::ConfirmRemoteBranch { prompt, .. } = &state.mode else {
        unreachable!()
    };
    assert_eq!(prompt.selected(), 1);

    let mut state = help_state(false);
    run(&mut state, Action::Select(3));
    run(
        &mut state,
        Action::Scroll {
            delta: 2,
            viewport_rows: 1,
        },
    );
    let Some(Overlay::Help(help)) = &state.overlay else {
        unreachable!()
    };
    assert_eq!(help.list.selected, Some(5));
    assert_eq!(state.repo_view.list.selected, Some(0));

    let mut state = state_with_repos(&["a"]);
    state.overlay = Some(Overlay::ScanWarnings(ScanWarningsOverlayState {
        scroll_offset: 1,
    }));
    run(
        &mut state,
        Action::Scroll {
            delta: -3,
            viewport_rows: 1,
        },
    );
    run(&mut state, Action::Select(1));
    assert!(matches!(
        state.overlay,
        Some(Overlay::ScanWarnings(ScanWarningsOverlayState {
            scroll_offset: 0
        }))
    ));
}

fn start(state: &AppState) -> Picker<MockBackend> {
    Picker::start(
        Arc::new(MockGitProvider::default()),
        Some(Arc::new(MockBackend::native())),
        KeysConfig::default(),
        Vec::new(),
        state,
    )
}

#[test]
fn activating_a_row_selects_it_then_runs_the_command_on_it() {
    let mut state = state_with_repos(&["a", "b"]);
    let picker = start(&state);
    let id = item_id(&state, 1).unwrap();
    assert_eq!(picker.activate(&mut state, 1, &id, Command::Open), None);
    assert_eq!(state.repo_view.list.selected, Some(1));
    assert_eq!(
        state.mode,
        Mode::Loading {
            message: "Opening b…".into(),
            branch: None,
        }
    );
}

#[test]
fn the_back_button_leaves_branches_whatever_is_typed() {
    let mut state = state_with_branches(vec![BranchEntry::local("main")]);
    state.branch_view.list.input.text = "ma".into();
    assert_eq!(
        crate::keymap::back_to_repos(&state),
        Some(Action::BackToRepos)
    );
    state.overlay = help_state(false).overlay;
    assert_eq!(crate::keymap::back_to_repos(&state), None);
    assert_eq!(
        crate::keymap::back_to_repos(&state_with_repos(&["a"])),
        None
    );
}

#[test]
fn dismissing_closes_overlays_and_dialogs_but_not_lists() {
    let state = help_state(false);
    assert_eq!(crate::keymap::dismiss(&state), Some(Action::CloseOverlay));
    let mut state = state_with_repos(&["a"]);
    assert_eq!(crate::keymap::dismiss(&state), None);
    state.mode = Mode::ConfirmRemoteBranch {
        context: context(),
        prompt: test_remote_prompt("feat/x", &["origin"], false),
    };
    assert_eq!(crate::keymap::dismiss(&state), Some(Action::CancelOverlay));
}

#[test]
fn ids_tell_apart_items_whose_names_look_alike() {
    let mut state = state_with_branches(vec![
        BranchEntry::local("origin/feature"),
        BranchEntry {
            remote: Some("origin".into()),
            ..BranchEntry::local("feature")
        },
    ]);
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let rows = &list(&view).rows;
    assert_eq!(rows[0].segments()[0].text, rows[1].segments()[0].text);
    assert_ne!(rows[0].id, rows[1].id);

    let base = |repo: &str| {
        let mut flow = BaseBranchSelection {
            new_name: "feat/x".into(),
            bases: vec!["main".into()],
            list: SearchableList::new(1),
        };
        base_list(std::path::Path::new(repo), &mut flow, 1).rows[0]
            .id
            .clone()
    };
    assert_ne!(base("/one"), base("/two"));

    let choices = |repo: &str| {
        let Dialog::RemoteBranch { choices, .. } = remote_branch_dialog(
            std::path::Path::new(repo),
            &test_remote_prompt("feat/x", &["origin"], false),
            &KeysConfig::default(),
        ) else {
            unreachable!()
        };
        choices
            .into_iter()
            .map(|choice| choice.id)
            .collect::<Vec<_>>()
    };
    let (one, two) = (choices("/one"), choices("/two"));
    assert_eq!(one.len(), 2);
    assert!(one.iter().all(|id| !two.contains(id)));
    assert_ne!(one[0], one[1]);
}

/// Two paths that are not valid Unicode and display the same.
fn paths_that_display_alike() -> [std::path::PathBuf; 2] {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        [b"/repo-\xfe".as_slice(), b"/repo-\xff".as_slice()]
            .map(|bytes| std::ffi::OsStr::from_bytes(bytes).into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        [0xD800_u16, 0xDC00].map(|unit| {
            let mut wide: Vec<u16> = "C:\\repo-".encode_utf16().collect();
            wide.push(unit);
            std::ffi::OsString::from_wide(&wide).into()
        })
    }
}

#[test]
fn ids_keep_paths_that_are_not_unicode_apart() {
    let [one, two] = paths_that_display_alike();
    assert_eq!(one.to_string_lossy(), two.to_string_lossy());
    let mut state = AppState::new(&TEST_PRODUCT, None);
    for path in [&one, &two] {
        state.repo_view.entries.push(RepoEntry::new(Repo {
            name: "repo".into(),
            path: path.clone(),
            is_git: true,
            worktrees: Vec::new(),
        }));
    }
    state.repo_view.list = SearchableList::new(2);
    let view = picker(&mut state, &KeysConfig::default(), VIEWPORTS);
    let rows = &list(&view).rows;
    assert_ne!(rows[0].id, rows[1].id);
    assert_eq!(item_id(&state, 1).as_deref(), Some(rows[1].id.as_str()));
}

#[test]
fn activating_a_row_whose_item_has_moved_does_nothing() {
    let mut state = state_with_repos(&["b", "c"]);
    let picker = start(&state);
    let shown = item_id(&state, 0).unwrap();
    // Discovery finds `a`, and sorting puts it first, so position 0 and entry 0 are `a`.
    state
        .repo_view
        .entries
        .insert(0, state_with_repos(&["a"]).repo_view.entries.remove(0));
    state.repo_view.list = SearchableList::new(3);
    assert_ne!(item_id(&state, 0).as_deref(), Some(shown.as_str()));
    assert_eq!(picker.activate(&mut state, 0, &shown, Command::Open), None);
    assert_eq!(state.mode, Mode::RepoSelect, "nothing opened");

    let mut branches = state_with_branches(vec![BranchEntry::local("main")]);
    let branch = item_id(&branches, 0).unwrap();
    assert_ne!(Some(branch.as_str()), item_id(&state, 0).as_deref());
    branches.mode = Mode::RepoSelect;
    assert_eq!(
        picker.activate(&mut branches, 0, &branch, Command::Open),
        None,
        "a click on the branch list does nothing once it has gone"
    );
    assert_eq!(branches.mode, Mode::RepoSelect);
    assert_eq!(picker.activate(&mut state, 5, &shown, Command::Open), None);
    assert_eq!(
        state.mode,
        Mode::RepoSelect,
        "a position past the list opens nothing"
    );
}

#[test]
fn activating_in_an_overlay_only_selects_and_nothing_is_activated_under_scan_warnings() {
    let mut state = help_state(false);
    let picker = start(&state);
    let help_id = item_id(&state, 2).unwrap();
    assert_eq!(
        picker.activate(&mut state, 2, &help_id, Command::Open),
        None
    );
    let Some(Overlay::Help(help)) = &state.overlay else {
        unreachable!()
    };
    assert_eq!(help.list.selected, Some(2));
    assert_eq!(state.mode, Mode::RepoSelect);

    let mut state = state_with_repos(&["a", "b"]);
    let id = item_id(&state, 1).unwrap();
    state.overlay = Some(Overlay::ScanWarnings(ScanWarningsOverlayState::default()));
    assert_eq!(picker.activate(&mut state, 1, &id, Command::Open), None);
    assert_eq!(state.repo_view.list.selected, Some(0));
    assert_eq!(state.mode, Mode::RepoSelect);
}
