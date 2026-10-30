use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::Duration,
};

use crate::{
    backend::{
        Opened,
        mock::{BackendCall, MockBackend, MockError},
    },
    event::{AppEvent, FilterKey, FilterTarget},
    git::Repo,
    picker::{TickChanges, process_app_event},
    product::TEST_PRODUCT,
    spawn::EventSender,
    state::{AppState, Mode, SearchableList},
};

use super::*;

fn repo(path: &str) -> Repo {
    Repo {
        name: Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        path: path.into(),
        is_git: true,
        worktrees: Vec::new(),
    }
}

fn state_with_repo() -> AppState {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.repo_view.entries.push(RepoEntry::new(Repo {
        name: "repo".into(),
        path: "/repo".into(),
        is_git: true,
        worktrees: Vec::new(),
    }));
    state.repo_view.list = SearchableList::new(1);
    state
}

fn state_with_folder() -> AppState {
    let mut state = state_with_repo();
    state.repo_view.entries[0].repo.is_git = false;
    state
}

fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
    let (tx, rx) = mpsc::channel();
    (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
}

#[test]
fn collision_disambiguates_two_equal_names_with_shortest_parent_suffix() {
    let repos = [repo("foo/bar/baz"), repo("qux/bar/baz")];
    assert_eq!(
        collision_disambiguators(&repos),
        [Some("foo/bar".into()), Some("qux/bar".into())]
    );
}

#[test]
fn collision_disambiguates_three_places_independently() {
    let repos = [
        repo("/root/a/shared/demo"),
        repo("/root/b/shared/demo"),
        repo("/root/unique/demo"),
    ];
    assert_eq!(
        collision_disambiguators(&repos),
        [
            Some("…/a/shared".into()),
            Some("…/b/shared".into()),
            Some("…/unique".into()),
        ]
    );
}

#[test]
fn collision_handles_parents_that_also_collide() {
    let repos = [repo("/one/team/api"), repo("/two/team/api")];
    assert_eq!(
        collision_disambiguators(&repos),
        [Some("one/team".into()), Some("two/team".into())]
    );
}

#[test]
fn collision_handles_repos_nested_below_search_roots() {
    let repos = [
        repo("/search/client/ios/app"),
        repo("/search/client/web/app"),
        repo("/search/direct/app"),
        repo("/search/other"),
    ];
    assert_eq!(
        collision_disambiguators(&repos),
        [
            Some("…/ios".into()),
            Some("…/web".into()),
            Some("…/direct".into()),
            None,
        ]
    );
}

#[test]
fn collision_leaves_unique_names_unchanged() {
    let repos = [repo("/one/alpha"), repo("/two/beta")];
    assert_eq!(collision_disambiguators(&repos), [None, None]);
}

#[test]
fn collision_windows_mode_is_case_insensitive_and_uses_forward_slashes() {
    let repos = [repo("C:/One/Team/API"), repo("c:/two/team/api")];
    assert_eq!(
        collision_disambiguators_with_case(&repos, true),
        [Some("…/One/Team".into()), Some("…/two/team".into())]
    );
}

#[test]
fn current_repo_selection_prefers_the_deepest_containing_repo() {
    let mut state = AppState::new(&TEST_PRODUCT, Some("/work/outer/inner/src".into()));
    state.repo_view.entries = vec![
        RepoEntry::new(repo("/work/outer")),
        RepoEntry::new(repo("/work/outer/inner")),
    ];
    state.repo_view.list = SearchableList::new(2);
    state.repo_view.apply_current_selection();
    assert_eq!(state.repo_view.list.selected, Some(1));
}

fn state_in(cwd: &str, repos: &[&str]) -> AppState {
    let mut state = AppState::new(&TEST_PRODUCT, Some(cwd.into()));
    state.repo_view.entries = repos
        .iter()
        .map(|path| RepoEntry::new(repo(path)))
        .collect();
    state.repo_view.list = SearchableList::new(repos.len());
    state.repo_view.list.selected = Some(0);
    state
}

fn resolve_current_repo(state: &mut AppState, main_checkout: &str) {
    process_app_event(
        AppEvent::CurrentRepoResolved {
            main_checkout: main_checkout.into(),
        },
        state,
        &mut TickChanges::default(),
    );
}

#[test]
fn current_repo_selection_finds_a_linked_worktree_outside_the_search_dirs() {
    let mut state = state_in("/trees/repo/feature/src", &["/code/other", "/code/repo"]);
    state.repo_view.apply_current_selection();
    assert_eq!(state.repo_view.list.selected, Some(0));

    resolve_current_repo(&mut state, "/code/repo");
    assert_eq!(
        state.selected_repo().unwrap().repo.path,
        Path::new("/code/repo")
    );

    // Later scan results re-apply the preselection rather than losing it.
    state
        .repo_view
        .entries
        .insert(0, RepoEntry::new(repo("/code/alpha")));
    state.repo_view.list = SearchableList::new(3);
    state.repo_view.apply_current_selection();
    assert_eq!(
        state.selected_repo().unwrap().repo.path,
        Path::new("/code/repo")
    );
}

#[test]
fn current_repo_selection_prefers_the_main_checkout_over_a_containing_entry() {
    let mut state = state_in("/work/outer/linked", &["/work/outer", "/work/repo"]);
    state.repo_view.apply_current_selection();
    assert_eq!(
        state.selected_repo().unwrap().repo.path,
        Path::new("/work/outer")
    );

    resolve_current_repo(&mut state, "/work/repo");
    assert_eq!(
        state.selected_repo().unwrap().repo.path,
        Path::new("/work/repo")
    );
}

#[test]
fn an_unlisted_main_checkout_falls_back_to_the_containing_entry() {
    let mut state = state_in("/work/outer/linked", &["/work/other", "/work/outer"]);
    resolve_current_repo(&mut state, "/elsewhere/repo");
    assert_eq!(
        state.selected_repo().unwrap().repo.path,
        Path::new("/work/outer")
    );
}

#[test]
fn current_repo_resolution_leaves_a_selection_the_user_moved() {
    let mut state = state_in("/trees/repo/feature", &["/code/other", "/code/repo"]);
    move_selection(&mut state, 0);
    resolve_current_repo(&mut state, "/code/repo");
    assert_eq!(state.repo_view.list.selected, Some(0));
}

#[test]
fn current_repo_resolution_only_considers_filtered_entries() {
    let mut state = state_in("/trees/repo/feature", &["/code/other", "/code/repo"]);
    state.repo_view.list.filtered = vec![(0, 1)];
    resolve_current_repo(&mut state, "/code/repo");
    assert_eq!(state.repo_view.list.selected, Some(0));
}

#[test]
fn a_failed_current_repo_resolution_is_a_warning() {
    let mut state = state_in("/somewhere", &["/code/repo"]);
    process_app_event(
        AppEvent::CurrentRepoFailed("could not find the repository".into()),
        &mut state,
        &mut TickChanges::default(),
    );
    let toast = state.toasts.front().unwrap();
    assert_eq!(toast.kind, ToastKind::Warning);
    assert_eq!(toast.message, "could not find the repository");
}

#[test]
fn current_logical_selection_survives_current_generation_filter_results() {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.repo_view.entries = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| {
            RepoEntry::new(Repo {
                name: name.into(),
                path: PathBuf::from(format!("/{name}")),
                is_git: true,
                worktrees: Vec::new(),
            })
        })
        .collect();
    state.repo_view.list = SearchableList::new(3);
    state.repo_view.list.selected = Some(1);
    state.repo_view.filter_generation = 7;
    process_app_event(
        AppEvent::FilterCompleted {
            target: FilterTarget::Repos,
            generation: 7,
            matches: vec![
                (FilterKey::Repo("/gamma".into()), 3),
                (FilterKey::Repo("/beta".into()), 2),
                (FilterKey::Repo("/alpha".into()), 1),
            ],
            selected: None,
        },
        &mut state,
        &mut TickChanges::default(),
    );
    assert_eq!(state.selected_repo().unwrap().repo.name, "beta");
}

#[test]
fn opening_transitions_to_loading_and_dispatches_through_the_backend() {
    let backend = Arc::new(MockBackend::native());
    backend
        .open_checkout_results
        .lock()
        .unwrap()
        .push_back(Err(MockError("boom".into())));
    let (sender, rx) = sender();
    let mut state = state_with_repo();

    open_selected(&mut state, Some(&backend), &sender);

    assert_eq!(
        state.mode,
        Mode::Loading {
            message: "Opening repo…".into(),
            branch: None,
        }
    );
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        &event,
        AppEvent::RepoOpenFailed(message) if message.contains("boom")
    ));
    process_app_event(event, &mut state, &mut TickChanges::default());
    assert_eq!(state.mode, Mode::RepoSelect);
    assert!(state.toasts.front().unwrap().message.contains("boom"));
    assert_eq!(
        backend.calls(),
        [BackendCall::OpenCheckout {
            repo: "/repo".into(),
            checkout: "/repo".into(),
        }]
    );
}

#[test]
fn opening_a_plain_folder_asks_the_backend_for_the_folder() {
    let backend = Arc::new(MockBackend::native());
    backend
        .open_folder_results
        .lock()
        .unwrap()
        .push_back(Ok(Opened {
            warning: Some("note".into()),
        }));
    let (sender, rx) = sender();
    let mut state = state_with_folder();

    open_selected(&mut state, Some(&backend), &sender);

    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        AppEvent::RepoOpened { warning: Some(warning) } if warning == "note"
    ));
    assert_eq!(
        backend.calls(),
        [BackendCall::OpenFolder {
            folder: "/repo".into(),
        }]
    );
}

#[test]
fn open_indicators_use_repo_roots_for_git_and_session_folders_for_folders() {
    let mut state = state_with_repo();
    state.repo_view.entries.push(RepoEntry::new(Repo {
        name: "folder".into(),
        path: "/folder".into(),
        is_git: false,
        worktrees: Vec::new(),
    }));
    state.repo_view.list = SearchableList::new(2);

    process_app_event(
        AppEvent::OpenReposLoaded {
            roots: vec!["/repo".into()],
        },
        &mut state,
        &mut TickChanges::default(),
    );
    assert!(state.repo_view.entries[0].is_open);
    assert!(!state.repo_view.entries[1].is_open);
    process_app_event(
        AppEvent::OpenFoldersLoaded {
            folders: vec!["/folder".into()],
        },
        &mut state,
        &mut TickChanges::default(),
    );

    assert!(state.repo_view.entries[0].is_open);
    assert!(state.repo_view.entries[1].is_open);
}

#[test]
fn opening_without_a_backend_keeps_picker_usable_and_shows_error() {
    let (sender, _rx) = sender();
    let mut state = state_with_repo();
    open_selected::<MockBackend>(&mut state, None, &sender);
    assert_eq!(state.mode, Mode::RepoSelect);
    assert_eq!(
        state.toasts.front().unwrap().message,
        TEST_PRODUCT.no_backend_message
    );
}
