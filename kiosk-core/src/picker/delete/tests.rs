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
    // herdr removes and closes in one call, so the record stays until that call returns.
    assert!(matches!(second, AppEvent::WorktreeRemovalFinished { .. }));
    assert_eq!(state.delete.pending.len(), 1);
    let mut changes = TickChanges::default();
    process_app_event(second, &mut state, &mut changes);
    assert_eq!(state.delete.pending, vec![]);
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
    // herdr removes and closes in one call, so the record stays until that call returns.
    assert!(matches!(second, AppEvent::WorktreeRemovalFinished { .. }));
    assert_eq!(state.delete.pending.len(), 1);
    let mut changes = TickChanges::default();
    process_app_event(second, &mut state, &mut changes);
    assert_eq!(state.delete.pending, vec![]);
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
    let removed = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        matches!(&removed, AppEvent::CheckoutRemoved { worktree_path, .. } if worktree_path == Path::new("/repo-feature")),
        "{removed:?}"
    );
    assert_eq!(git_mock.remove_calls.lock().unwrap().len(), 1);
    assert_eq!(state.delete.pending.len(), 1);
    // The session stays open until the picker has forgotten the record, as closing it can
    // end the picker.
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !backend
            .calls()
            .iter()
            .any(|call| matches!(call, BackendCall::Close { .. }))
    );
    process_app_event(removed, &mut state, &mut TickChanges::default());
    assert_eq!(state.delete.pending, vec![]);
    let finished = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(finished, &mut state, &mut TickChanges::default());

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
fn a_managed_removal_still_closes_the_session_when_the_picker_stops_first() {
    let git = git_provider();
    let backend = Arc::new(MockBackend::managed(std::env::temp_dir()));
    queue_checkouts(&backend, Some("s_1"), 1);
    backend.close_results.lock().unwrap().push_back(Ok(()));
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    confirm(&mut state, &git, Some(&backend), &sender);
    let removed = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(removed);

    let finished = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        finished,
        AppEvent::WorktreeRemovalFinished {
            outcome: WorktreeRemovalOutcome::Removed { warning: None },
            ..
        }
    ));
    assert_eq!(
        backend.calls().last(),
        Some(&BackendCall::Close {
            session: "s_1".into()
        })
    );
}

#[test]
fn a_managed_session_stays_open_when_the_cleared_record_cannot_be_saved() {
    let git = git_provider();
    let backend = Arc::new(MockBackend::managed(std::env::temp_dir()));
    queue_checkouts(&backend, Some("s_1"), 1);
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    confirm(&mut state, &git, Some(&backend), &sender);
    let removed = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    FAIL_SAVES.set(true);
    process_app_event(removed, &mut state, &mut TickChanges::default());
    FAIL_SAVES.set(false);

    let finished = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let AppEvent::WorktreeRemovalFinished {
        outcome: WorktreeRemovalOutcome::Removed {
            warning: Some(warning),
        },
        ..
    } = &finished
    else {
        panic!("expected a removal with a warning, got {finished:?}");
    };
    assert!(
        warning.contains("left open in") && warning.contains("the disk is full"),
        "{warning}"
    );
    assert!(
        !backend
            .calls()
            .iter()
            .any(|call| matches!(call, BackendCall::Close { .. }))
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
        &PendingWorktreeDelete::new("/repo".into(), "feature".into(), "/repo-feature".into()),
    )
    .unwrap();
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

fn branches_loaded_with_checkout_of(state: &mut AppState, branch: &str) {
    let generation = state.branch_view.generation;
    state.branch_view.loading = true;
    process_app_event(
        AppEvent::BranchesLoaded {
            repo_path: "/repo".into(),
            generation,
            branches: vec![BranchEntry {
                checkout: Some(BranchCheckout::closed("/repo-feature".into())),
                ..BranchEntry::local(branch)
            }],
            worktrees: vec![Worktree {
                path: "/repo-feature".into(),
                branch: Some(branch.into()),
            }],
            skipped_unsupported_refs: false,
        },
        state,
        &mut TickChanges::default(),
    );
}

fn forced_pending_delete_of_feature(state: &mut AppState) {
    mark_pending(
        state,
        &PendingWorktreeDelete {
            force: true,
            ..PendingWorktreeDelete::new("/repo".into(), "feature".into(), "/repo-feature".into())
        },
    )
    .unwrap();
}

#[test]
fn a_saved_force_deletion_resumes_while_its_branch_is_still_checked_out_there() {
    let mut state = state_with_branch(true);
    forced_pending_delete_of_feature(&mut state);
    branches_loaded_with_checkout_of(&mut state, "feature");
    assert_eq!(state.delete.pending.len(), 1);
}

#[test]
fn a_saved_force_deletion_is_dropped_when_another_branch_now_uses_its_path() {
    let git = git_provider();
    let backend = Arc::new(MockBackend::native());
    let (sender, _rx) = sender();
    let mut state = state_with_branch(true);
    forced_pending_delete_of_feature(&mut state);

    branches_loaded_with_checkout_of(&mut state, "replacement");
    resume(&mut state, &git, Some(&backend), &sender);

    assert_eq!(state.delete.pending, vec![]);
    assert!(state.delete.in_flight.is_empty());
    assert_eq!(backend.calls(), []);
}

#[test]
fn a_saved_force_deletion_another_picker_finished_is_not_resumed_or_saved_again() {
    let git = git_provider();
    let backend = Arc::new(MockBackend::native());
    let (sender, _rx) = sender();
    let mut state = state_with_branch(true);
    forced_pending_delete_of_feature(&mut state);
    // Another picker finishes the deletion, and the branch is then checked out again at the
    // same path.
    SAVED.with_borrow_mut(Vec::clear);

    branches_loaded_with_checkout_of(&mut state, "feature");
    resume(&mut state, &git, Some(&backend), &sender);
    forget_pending(&mut state, Path::new("/repo-other"));

    assert_eq!(state.delete.pending, vec![]);
    assert!(SAVED.with_borrow(Vec::is_empty));
    assert!(state.delete.in_flight.is_empty());
    assert_eq!(backend.calls(), []);
}

#[test]
fn resuming_picks_up_a_deletion_another_picker_saved() {
    let git = git_provider();
    let backend = Arc::new(MockBackend::native());
    let (sender, _rx) = sender();
    let mut state = state_with_branch(true);
    SAVED.with_borrow_mut(|saved| {
        saved.push(PendingWorktreeDelete::new(
            "/repo".into(),
            "feature".into(),
            "/repo-feature".into(),
        ));
    });

    resume(&mut state, &git, Some(&backend), &sender);

    assert_eq!(state.delete.pending.len(), 1);
    assert!(state.delete.in_flight.contains(Path::new("/repo-feature")));
}
