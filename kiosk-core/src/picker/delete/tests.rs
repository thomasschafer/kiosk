use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use crate::{
    backend::{
        CheckoutState, RemoveError, Removed,
        mock::{BackendCall, MockBackend, MockError},
    },
    event::{AppEvent, WorktreeRemovalOutcome},
    git::{GitProvider, Repo, Worktree, mock::MockGitProvider},
    pending_delete::PendingWorktreeDelete,
    picker::{TickChanges, process_app_event},
    product::TEST_PRODUCT,
    spawn::EventSender,
    state::{
        AppState, BranchCheckout, BranchContext, BranchEntry, Mode, OpenCheckoutLoadState,
        RepoEntry, SearchableList,
    },
};

use super::*;

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

fn state_with_branch(has_worktree: bool) -> AppState {
    let mut state = state_with_repo();
    state.mode = Mode::BranchSelect(BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    });
    state.branch_view.entries = vec![BranchEntry {
        checkout: has_worktree.then(|| BranchCheckout::closed("/repo-feature".into())),
        ..BranchEntry::local("feature")
    }];
    state.branch_view.list = SearchableList::new(1);
    state.branch_view.open_checkout_load_state = OpenCheckoutLoadState::Loaded {
        repo_path: "/repo".into(),
        generation: state.branch_view.generation,
    };
    state
}

fn checkout(session: Option<&str>) -> CheckoutState<String> {
    CheckoutState {
        path: "/repo-feature".into(),
        session: session.map(Into::into),
    }
}

fn queue_checkouts(backend: &MockBackend, session: Option<&str>, times: usize) {
    backend
        .checkouts_results
        .lock()
        .unwrap()
        .extend((0..times).map(|_| Ok(vec![checkout(session)])));
}

fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
    let (tx, rx) = mpsc::channel();
    (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
}

fn git_provider() -> Arc<dyn GitProvider> {
    Arc::new(MockGitProvider::default())
}

#[test]
fn delete_guards_refuse_main_checkout_and_remote_only_entries_in_state() {
    let mut state = AppState::new(&TEST_PRODUCT, None);
    state.mode = Mode::BranchSelect(BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    });
    state.branch_view.entries = vec![BranchEntry {
        name: "main".into(),
        checkout: Some(BranchCheckout {
            path: "/repo".into(),
            is_open: true,
        }),
        is_current: true,
        is_default: true,
        remote: None,
    }];
    state.branch_view.list = SearchableList::new(1);
    assert_eq!(
        selected_target(&state),
        Err("Cannot delete the main checkout")
    );

    state.branch_view.entries[0] = BranchEntry {
        name: "remote-only".into(),
        checkout: None,
        is_current: false,
        is_default: false,
        remote: Some("origin".into()),
    };
    assert_eq!(
        selected_target(&state),
        Err("Remote-only branches have no checkout to delete")
    );
}

#[test]
fn delete_before_open_state_load_is_refused() {
    let git_mock = Arc::new(MockGitProvider::default());
    let backend = MockBackend::native();
    let mut state = state_with_branch(true);
    state.branch_view.open_checkout_load_state = OpenCheckoutLoadState::Unknown;

    begin(&mut state);

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(
        state
            .toasts
            .back()
            .is_some_and(|toast| toast.message.contains("still loading"))
    );
    assert!(git_mock.remove_calls.lock().unwrap().is_empty());
    assert_eq!(backend.calls(), []);
}

#[test]
fn delete_after_open_state_failure_is_refused() {
    let mut state = state_with_branch(true);
    state.branch_view.open_checkout_load_state = OpenCheckoutLoadState::Failed {
        repo_path: "/repo".into(),
        generation: state.branch_view.generation,
    };

    begin(&mut state);

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(state.toasts.back().is_some_and(|toast| {
        toast
            .message
            .contains("open checkout state could not be loaded")
    }));
}

#[test]
fn checkout_becoming_open_after_confirmation_routes_removal_through_the_backend() {
    let git_mock = Arc::new(MockGitProvider::default());
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let backend = Arc::new(MockBackend::native());
    queue_checkouts(&backend, Some("w_1"), 1);
    backend
        .remove_results
        .lock()
        .unwrap()
        .push_back(Ok(Removed::default()));
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    assert!(matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete(flow) if !flow.target.is_open
    ));

    confirm(&mut state, &git, Some(&backend), &sender);
    let _event = rx.recv_timeout(Duration::from_secs(1)).unwrap();

    assert_eq!(
        backend.calls(),
        [
            BackendCall::Checkouts {
                repo: "/repo".into(),
            },
            BackendCall::Remove {
                session: "w_1".into(),
                force: false,
            },
        ]
    );
    assert!(git_mock.remove_calls.lock().unwrap().is_empty());
}

#[test]
fn fresh_open_state_failure_refuses_removal_without_falling_back_to_git() {
    let git_mock = Arc::new(MockGitProvider::default());
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let backend = Arc::new(MockBackend::native());
    backend
        .checkouts_results
        .lock()
        .unwrap()
        .push_back(Err(MockError("backend unavailable".into())));
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    confirm(&mut state, &git, Some(&backend), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(state.toasts.back().is_some_and(|toast| {
        toast
            .message
            .contains("could not refresh open checkout state")
    }));
    assert!(git_mock.remove_calls.lock().unwrap().is_empty());
    assert_eq!(
        backend.calls(),
        [BackendCall::Checkouts {
            repo: "/repo".into(),
        }]
    );
}

#[test]
fn native_delete_requires_a_second_force_confirmation_then_refreshes() {
    let backend = Arc::new(MockBackend::native());
    queue_checkouts(&backend, Some("w_1"), 2);
    backend.remove_results.lock().unwrap().extend([
        Err(RemoveError::DirtyRequiresForce(MockError("dirty".into()))),
        Ok(Removed::default()),
    ]);
    let git = git_provider();
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);
    state.branch_view.entries[0].checkout = Some(BranchCheckout {
        path: "/repo-feature".into(),
        is_open: true,
    });

    begin(&mut state);
    confirm(&mut state, &git, Some(&backend), &sender);
    let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(first, &mut state, &mut TickChanges::default());
    assert!(matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete(flow)
            if flow.target.force && !flow.target.in_progress && flow.target.is_open
    ));

    confirm(&mut state, &git, Some(&backend), &sender);
    let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(second, &mut state, &mut changes);
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(changes.refresh_branch.is_some());
    assert_eq!(
        backend.calls(),
        [
            BackendCall::Checkouts {
                repo: "/repo".into(),
            },
            BackendCall::Remove {
                session: "w_1".into(),
                force: false,
            },
            BackendCall::Checkouts {
                repo: "/repo".into(),
            },
            BackendCall::Remove {
                session: "w_1".into(),
                force: true,
            },
        ]
    );
}

#[test]
fn closed_git_checkout_requires_force_confirmation_and_prunes_after_success() {
    let git_mock = Arc::new(MockGitProvider::default());
    git_mock.dirty_remove_once.store(true, Ordering::Release);
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let backend = Arc::new(MockBackend::native());
    queue_checkouts(&backend, None, 2);
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    confirm(&mut state, &git, Some(&backend), &sender);
    let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(first, &mut state, &mut TickChanges::default());
    assert!(matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete(flow)
            if flow.target.force && !flow.target.in_progress
    ));

    confirm(&mut state, &git, Some(&backend), &sender);
    let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(second, &mut state, &mut changes);
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(changes.refresh_branch.is_some());
    assert_eq!(
        *git_mock.remove_calls.lock().unwrap(),
        [
            (
                PathBuf::from("/repo"),
                PathBuf::from("/repo-feature"),
                false
            ),
            (PathBuf::from("/repo"), PathBuf::from("/repo-feature"), true),
        ]
    );
    assert_eq!(
        *git_mock.prune_calls.lock().unwrap(),
        [PathBuf::from("/repo")]
    );
    assert_eq!(
        backend.calls(),
        [
            BackendCall::Checkouts {
                repo: "/repo".into(),
            },
            BackendCall::Checkouts {
                repo: "/repo".into(),
            },
        ]
    );
}

#[test]
fn managed_delete_of_an_open_checkout_removes_it_with_git_then_closes_its_session() {
    let git_mock = Arc::new(MockGitProvider::default());
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let backend = Arc::new(MockBackend::managed(std::env::temp_dir()));
    queue_checkouts(&backend, Some("s_1"), 1);
    backend.close_results.lock().unwrap().push_back(Ok(()));
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    confirm(&mut state, &git, Some(&backend), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert_eq!(
        *git_mock.remove_calls.lock().unwrap(),
        [(
            PathBuf::from("/repo"),
            PathBuf::from("/repo-feature"),
            false
        )]
    );
    assert_eq!(
        backend.calls(),
        [
            BackendCall::Checkouts {
                repo: "/repo".into(),
            },
            BackendCall::Close {
                session: "s_1".into(),
            },
        ]
    );
}

#[test]
fn post_deletion_refresh_rejects_old_final_fetch_without_clearing_spinner() {
    let git = git_provider();
    let (sender, _rx) = sender();
    let mut state = state_with_branch(true);
    let old_generation = state.branch_view.generation;
    begin(&mut state);
    if let Mode::ConfirmWorktreeDelete(flow) = &mut state.mode {
        flow.target.in_progress = true;
    }
    let mut changes = TickChanges::default();

    process_app_event(
        AppEvent::WorktreeRemovalFinished {
            repo_path: "/repo".into(),
            branch_name: "feature".into(),
            worktree_path: "/repo-feature".into(),
            outcome: WorktreeRemovalOutcome::Removed { warning: None },
        },
        &mut state,
        &mut changes,
    );
    let repo = changes.refresh_branch.take().expect("branch refresh");
    crate::picker::branch::refresh::<MockBackend>(&mut state, &git, None, &sender, repo);
    assert_eq!(state.branch_view.generation, old_generation + 1);
    state.branch_view.fetching_remote_repo = Some("/repo".into());

    process_app_event(
        AppEvent::GitFetchCompleted {
            remote: Some("origin".into()),
            branches: Vec::new(),
            repo_path: "/repo".into(),
            generation: old_generation,
            error: None,
            is_final: true,
            skipped_unsupported_refs: false,
        },
        &mut state,
        &mut TickChanges::default(),
    );

    assert_eq!(
        state.branch_view.fetching_remote_repo.as_deref(),
        Some(Path::new("/repo"))
    );
}

#[test]
fn late_recovered_delete_completion_does_not_change_a_newer_mode() {
    let mut state = state_with_repo();
    state.repo_view.entries[0].repo.worktrees.push(Worktree {
        path: "/repo-feature".into(),
        branch: Some("feature".into()),
    });
    state.delete.in_flight.insert("/repo-feature".into());
    mark_pending(
        &mut state,
        PendingWorktreeDelete::new("/repo".into(), "feature".into(), "/repo-feature".into()),
    );
    let mut changes = TickChanges::default();

    process_app_event(
        AppEvent::WorktreeRemovalFinished {
            repo_path: "/repo".into(),
            branch_name: "feature".into(),
            worktree_path: "/repo-feature".into(),
            outcome: WorktreeRemovalOutcome::Removed { warning: None },
        },
        &mut state,
        &mut changes,
    );

    assert_eq!(state.mode, Mode::RepoSelect);
    assert_eq!(state.delete.pending, vec![]);
    assert_eq!(state.repo_view.entries[0].repo.worktrees, vec![]);
    assert!(changes.refresh_branch.is_none());
}
