use std::sync::atomic::AtomicBool;

use crate::{
    backend::mock::MockBackend,
    git::{Repo, mock::MockGitProvider},
    product::TEST_PRODUCT,
    state::{
        BranchCheckout, BranchContext, BranchEntry, BranchId, OpenCheckoutLoadState, RepoEntry,
    },
};

use super::*;

fn items(names: &[&str]) -> Vec<FilterItem> {
    names
        .iter()
        .map(|name| FilterItem {
            key: FilterKey::Branch((*name).into()),
            text: (*name).into(),
        })
        .collect()
}

#[test]
fn git_only_results_do_not_request_pane_list() {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    let mut changes = TickChanges::default();
    process_app_event(
        AppEvent::ReposFound {
            repo: Repo {
                name: "repo".into(),
                path: "/repo".into(),
                is_git: true,
                worktrees: Vec::new(),
            },
        },
        &mut state,
        &mut changes,
    );
    let backend = Arc::new(MockBackend::native());
    let (tx, _rx) = mpsc::channel();
    let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

    load_open_folder_indicators(&mut changes, Some(&backend), &sender);

    assert_eq!(backend.calls(), []);
}

fn names(matches: &[(FilterKey, i64)]) -> Vec<String> {
    matches
        .iter()
        .map(|(key, _)| match key {
            FilterKey::Repo(path) => path.file_name().unwrap().to_string_lossy().into_owned(),
            FilterKey::Branch(id) => id.display_name(),
            FilterKey::Base(name) => name.clone(),
            FilterKey::Help(index) => index.to_string(),
        })
        .collect()
}

#[test]
fn empty_search_preserves_canonical_order() {
    let items = items(&["zebra", "apple", "mango"]);
    assert_eq!(
        names(&fuzzy_filter("", &items, &SkimMatcherV2::default())),
        ["zebra", "apple", "mango"]
    );
}

#[test]
fn fuzzy_order_is_score_then_length_then_alphabetical() {
    let cli_items = items(&["cli-extension-dep-graph", "cli-tools", "cli", "cli-abc"]);
    assert_eq!(
        names(&fuzzy_filter("cli", &cli_items, &SkimMatcherV2::default())),
        ["cli", "cli-abc", "cli-tools", "cli-extension-dep-graph"]
    );
    let foo_items = items(&["bfoo", "afoo", "cfoo"]);
    assert_eq!(
        names(&fuzzy_filter("foo", &foo_items, &SkimMatcherV2::default())),
        ["afoo", "bfoo", "cfoo"]
    );
}

#[test]
fn fuzzy_search_matches_collision_disambiguator() {
    let items = vec![FilterItem {
        key: FilterKey::Repo("/repo".into()),
        text: "demo (…/customer-one)".into(),
    }];
    assert_eq!(
        names(&fuzzy_filter(
            "customer-one",
            &items,
            &SkimMatcherV2::default()
        )),
        ["repo"]
    );
}

#[test]
fn fuzzy_search_matches_remote_qualified_branch_display() {
    let id = BranchId::Remote {
        remote: "upstream".into(),
        name: "feature".into(),
    };
    let items = vec![FilterItem {
        key: FilterKey::Branch(id.clone()),
        text: id.display_name(),
    }];

    let matches = fuzzy_filter("upstream", &items, &SkimMatcherV2::default());
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].0, FilterKey::Branch(id));
}

#[test]
fn no_matches_returns_an_empty_result() {
    assert_eq!(
        fuzzy_filter(
            "zzzzz",
            &items(&["alpha", "beta"]),
            &SkimMatcherV2::default()
        ),
        vec![]
    );
}

#[test]
fn branch_filter_uses_the_shared_score_length_and_name_ranking() {
    let branches = items(&["feature/very-long", "feature", "feature-short"]);
    assert_eq!(
        names(&fuzzy_filter(
            "feature",
            &branches,
            &SkimMatcherV2::default()
        )),
        ["feature", "feature-short", "feature/very-long"]
    );
}

#[test]
fn help_fuzzy_filter_matches_key_command_and_description() {
    let overlay = HelpOverlayState::for_mode(
        &KeysConfig::default(),
        crate::config::keys::BindingMode::Branch,
    );
    let items = overlay
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| FilterItem {
            key: FilterKey::Help(index),
            text: row.search_text(),
        })
        .collect::<Vec<_>>();
    let matcher = SkimMatcherV2::default();
    for query in ["ctrl+o", "new_branch", "Create a new branch"] {
        let filtered = fuzzy_filter(query, &items, &matcher);
        assert!(filtered.iter().any(|(key, _)| {
            matches!(key, FilterKey::Help(index) if overlay.rows[*index].command_name == "new_branch")
        }));
    }
}

#[test]
fn branch_logical_selection_survives_current_generation_filter_results() {
    let mut branch_state = state_with_branch(false);
    branch_state.branch_view.entries = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| BranchEntry {
            name: name.into(),
            checkout: None,
            is_current: false,
            is_default: false,
            remote: None,
        })
        .collect();
    branch_state.branch_view.list = SearchableList::new(3);
    branch_state.branch_view.list.selected = Some(1);
    branch_state.branch_view.filter_generation = 9;
    process_app_event(
        AppEvent::FilterCompleted {
            target: FilterTarget::Branches,
            generation: 9,
            matches: vec![
                (FilterKey::Branch("gamma".into()), 3),
                (FilterKey::Branch("beta".into()), 2),
                (FilterKey::Branch("alpha".into()), 1),
            ],
            selected: None,
        },
        &mut branch_state,
        &mut TickChanges::default(),
    );
    assert_eq!(branch_state.selected_branch().unwrap().name, "beta");
}

#[test]
fn unicode_actions_edit_repo_and_new_branch_queries() {
    let git = Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>;
    let (sender, _rx) = sender();
    let worker = FilterWorker::spawn(sender.clone());
    let keys = KeysConfig::default();
    let mut state = state_with_repo();
    process_action::<MockBackend>(
        Action::Insert('é'),
        &mut state,
        &git,
        None,
        &sender,
        &worker,
        &keys,
    );
    assert_eq!(state.repo_view.list.input.text, "é");

    state.mode = Mode::BranchSelect(BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    });
    process_action::<MockBackend>(
        Action::Insert('界'),
        &mut state,
        &git,
        None,
        &sender,
        &worker,
        &keys,
    );
    assert_eq!(state.branch_view.list.input.text, "界");
}

fn state_with_repo() -> AppState {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.repo_view.entries.push(RepoEntry::new(Repo {
        name: "repo".into(),
        path: "/repo".into(),
        is_git: true,
        worktrees: Vec::new(),
    }));
    state.repo_view.list = crate::state::SearchableList::new(1);
    state
}

fn state_with_branch(has_worktree: bool) -> AppState {
    let mut state = state_with_repo();
    state.mode = Mode::BranchSelect(BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    });
    state.branch_view.entries = vec![BranchEntry {
        name: "feature".into(),
        checkout: has_worktree.then(|| BranchCheckout::closed("/repo-feature".into())),
        is_current: false,
        is_default: false,
        remote: None,
    }];
    state.branch_view.list = SearchableList::new(1);
    state.branch_view.open_checkout_load_state = OpenCheckoutLoadState::Loaded {
        repo_path: "/repo".into(),
        generation: state.branch_view.generation,
    };
    state
}

fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
    let (tx, rx) = mpsc::channel();
    (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
}

#[test]
fn annotation_events_replace_labels_and_failures_warn() {
    use crate::backend::{Annotations, Label, Tone, annotation::AnnotationIndex};

    let mut state = AppState::new(&TEST_PRODUCT, None);
    let index = AnnotationIndex::new(Annotations {
        repos: Vec::new(),
        checkouts: vec![(
            "/repo".into(),
            vec![Label {
                text: "claude working".into(),
                tone: Tone::Progress,
            }],
        )],
    });
    let mut changes = TickChanges::default();
    process_app_event(
        AppEvent::AnnotationsLoaded(index.clone()),
        &mut state,
        &mut changes,
    );
    assert_eq!(state.annotations, index);
    process_app_event(
        AppEvent::AnnotationsFailed("could not refresh status labels: down".into()),
        &mut state,
        &mut changes,
    );
    assert_eq!(
        state.toasts.front().map(|toast| toast.message.as_str()),
        Some("could not refresh status labels: down")
    );
}
