use std::path::{Path, PathBuf};

use super::*;
use crate::{
    backend::{
        CheckoutState, Removed,
        mock::{BackendCall, MockBackend, MockError},
    },
    git::{Worktree, mock::MockGitProvider},
};

const BACKEND: &str = "the test backend";

fn repo() -> PathBuf {
    std::env::temp_dir().join("repo")
}

fn root() -> PathBuf {
    std::env::temp_dir().join("kiosk-worktrees")
}

fn managed() -> MockBackend {
    MockBackend::managed(root())
}

fn opened(backend: &MockBackend) {
    backend
        .open_checkout_results
        .lock()
        .unwrap()
        .push_back(Ok(Opened::default()));
}

/// Removes `checkout`, failing the test if a session would be closed after a git removal.
fn remove(
    git: &MockGitProvider,
    backend: &MockBackend,
    checkout: &Path,
    force: bool,
) -> WorktreeRemovalOutcome {
    remove_checkout(git, backend, BACKEND, &repo(), checkout, force, || {
        panic!("nothing should close a session after a git removal here");
    })
}

/// Removes `checkout` and returns what git and the backend had been asked to do by the
/// time the hook before the session close ran.
fn remove_then_close(
    git: &MockGitProvider,
    backend: &MockBackend,
    checkout: &Path,
    force: bool,
) -> (
    WorktreeRemovalOutcome,
    Vec<(PathBuf, PathBuf, bool)>,
    Vec<BackendCall>,
) {
    let mut seen = None;
    let outcome = remove_checkout(git, backend, BACKEND, &repo(), checkout, force, || {
        seen = Some((git.remove_calls.lock().unwrap().clone(), backend.calls()));
        Ok(())
    });
    let (removed, calls) = seen.expect("the hook runs before the session closes");
    (outcome, removed, calls)
}

fn checkouts(backend: &MockBackend, states: Vec<CheckoutState<String>>) {
    backend
        .checkouts_results
        .lock()
        .unwrap()
        .push_back(Ok(states));
}

fn git_with_checkout(branch: &str, path: &Path) -> MockGitProvider {
    MockGitProvider {
        worktrees: vec![
            Worktree {
                path: repo(),
                branch: Some("main".into()),
            },
            Worktree {
                path: path.into(),
                branch: Some(branch.into()),
            },
        ],
        ..MockGitProvider::default()
    }
}

#[test]
fn managed_open_uses_the_branch_checkout_git_lists_now() {
    let existing = std::env::temp_dir().join("elsewhere/feature");
    let git = git_with_checkout("feature", &existing);
    let backend = managed();
    opened(&backend);

    open_branch(&git, &backend, &repo(), "feature", KnownCheckout::Missing).unwrap();

    assert!(git.add_worktree_calls.lock().unwrap().is_empty());
    assert_eq!(
        backend.calls(),
        [BackendCall::OpenCheckout {
            repo: repo(),
            checkout: existing,
        }]
    );
}

#[test]
fn managed_open_makes_a_checkout_in_the_placement_when_the_branch_has_none() {
    let git = MockGitProvider::default();
    let backend = managed();
    opened(&backend);

    open_branch(&git, &backend, &repo(), "feat/login", KnownCheckout::Exists).unwrap();

    let checkout = root().join("repo").join("feat-login");
    assert_eq!(
        *git.add_worktree_calls.lock().unwrap(),
        [(repo(), checkout.clone(), "feat/login".into(), None)]
    );
    assert_eq!(
        backend.calls(),
        [BackendCall::OpenCheckout {
            repo: repo(),
            checkout,
        }]
    );
}

#[test]
fn managed_create_branches_from_the_base_then_opens_the_checkout() {
    let git = MockGitProvider::default();
    let backend = managed();
    opened(&backend);

    create_branch(&git, &backend, &repo(), "new", "main").unwrap();

    let checkout = root().join("repo").join("new");
    assert_eq!(
        *git.add_worktree_calls.lock().unwrap(),
        [(
            repo(),
            checkout.clone(),
            "new".into(),
            Some("refs/heads/main".into())
        )]
    );
    assert_eq!(
        backend.calls(),
        [BackendCall::OpenCheckout {
            repo: repo(),
            checkout,
        }]
    );
}

#[test]
fn managed_create_opens_nothing_when_git_fails() {
    let git = MockGitProvider::default();
    *git.add_worktree_failure.lock().unwrap() = Some("path already exists".into());
    let backend = managed();

    let error = create_branch(&git, &backend, &repo(), "new", "main").unwrap_err();

    assert!(error.to_string().contains("path already exists"), "{error}");
    assert_eq!(backend.calls(), []);
}

#[test]
fn native_backends_get_branch_operations_without_kiosk_touching_git() {
    let git = MockGitProvider::default();
    let backend = MockBackend::native();
    backend
        .open_branch_results
        .lock()
        .unwrap()
        .push_back(Ok(Opened::default()));
    backend
        .create_branch_results
        .lock()
        .unwrap()
        .push_back(Err(MockError("busy".into())));

    open_branch(&git, &backend, &repo(), "feature", KnownCheckout::Exists).unwrap();
    let error = create_branch(&git, &backend, &repo(), "new", "main").unwrap_err();

    assert_eq!(error.to_string(), "busy");
    assert!(git.list_worktree_calls.lock().unwrap().is_empty());
    assert!(git.add_worktree_calls.lock().unwrap().is_empty());
    assert_eq!(
        backend.calls(),
        [
            BackendCall::OpenBranch {
                repo: repo(),
                branch: "feature".into(),
                known: KnownCheckout::Exists,
            },
            BackendCall::CreateBranch {
                repo: repo(),
                branch: "new".into(),
                base: "refs/heads/main".into(),
            },
        ]
    );
}

#[test]
fn a_checkout_nothing_shows_is_removed_and_pruned_with_git_for_every_backend() {
    let checkout = root().join("repo/feature");
    for backend in [MockBackend::native(), managed()] {
        let git = MockGitProvider::default();
        checkouts(
            &backend,
            vec![CheckoutState {
                path: checkout.clone(),
                session: None,
            }],
        );

        let outcome = remove(&git, &backend, &checkout, false);

        assert_eq!(outcome, WorktreeRemovalOutcome::Removed { warning: None });
        assert_eq!(
            *git.remove_calls.lock().unwrap(),
            [(repo(), checkout.clone(), false)]
        );
        assert_eq!(*git.prune_calls.lock().unwrap(), [repo()]);
        assert_eq!(backend.calls(), [BackendCall::Checkouts { repo: repo() }]);
    }
}

#[test]
fn managed_removal_of_an_open_checkout_removes_it_then_closes_its_session() {
    let checkout = root().join("repo/feature");
    let git = MockGitProvider::default();
    let backend = managed();
    checkouts(
        &backend,
        vec![CheckoutState {
            path: checkout.clone(),
            session: Some("s1".into()),
        }],
    );
    backend.close_results.lock().unwrap().push_back(Ok(()));

    let (outcome, removed_before_close, calls_before_close) =
        remove_then_close(&git, &backend, &checkout, true);

    assert_eq!(outcome, WorktreeRemovalOutcome::Removed { warning: None });
    assert_eq!(removed_before_close, [(repo(), checkout.clone(), true)]);
    assert_eq!(
        calls_before_close,
        [BackendCall::Checkouts { repo: repo() }]
    );
    assert_eq!(
        *git.remove_calls.lock().unwrap(),
        [(repo(), checkout, true)]
    );
    assert!(git.prune_calls.lock().unwrap().is_empty());
    assert_eq!(
        backend.calls()[1..],
        [BackendCall::Close {
            session: "s1".into()
        }]
    );
}

#[test]
fn managed_removal_leaves_the_session_open_when_git_refuses_a_dirty_checkout() {
    let checkout = root().join("repo/feature");
    let git = MockGitProvider::default();
    git.dirty_remove_once
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let backend = managed();
    checkouts(
        &backend,
        vec![CheckoutState {
            path: checkout.clone(),
            session: Some("s1".into()),
        }],
    );

    let outcome = remove(&git, &backend, &checkout, false);

    assert_eq!(outcome, WorktreeRemovalOutcome::DirtyRequiresForce);
    assert_eq!(backend.calls(), [BackendCall::Checkouts { repo: repo() }]);
}

#[test]
fn managed_removal_warns_when_the_session_does_not_close() {
    let checkout = root().join("repo/feature");
    let git = MockGitProvider::default();
    let backend = managed();
    checkouts(
        &backend,
        vec![CheckoutState {
            path: checkout.clone(),
            session: Some("s1".into()),
        }],
    );
    backend
        .close_results
        .lock()
        .unwrap()
        .push_back(Err(MockError("gone".into())));

    let (outcome, _, _) = remove_then_close(&git, &backend, &checkout, false);

    assert_eq!(
        outcome,
        WorktreeRemovalOutcome::Removed {
            warning: Some(
                "checkout was removed, but it could not be closed in the test backend: gone".into()
            )
        }
    );
}

#[test]
fn a_managed_session_stays_open_when_the_record_cannot_be_forgotten() {
    let checkout = root().join("repo/feature");
    let git = MockGitProvider::default();
    let backend = managed();
    checkouts(
        &backend,
        vec![CheckoutState {
            path: checkout.clone(),
            session: Some("s1".into()),
        }],
    );

    let outcome = remove_checkout(&git, &backend, BACKEND, &repo(), &checkout, false, || {
        Err("the record could not be saved".into())
    });

    assert_eq!(
        outcome,
        WorktreeRemovalOutcome::Removed {
            warning: Some(
                "checkout was removed, but it was left open in the test backend because the record could not be saved".into()
            )
        }
    );
    assert_eq!(
        *git.remove_calls.lock().unwrap(),
        [(repo(), checkout, false)]
    );
    assert_eq!(backend.calls(), [BackendCall::Checkouts { repo: repo() }]);
}

#[test]
fn native_removal_goes_through_the_backend_and_asks_for_force_once() {
    let checkout = root().join("repo/feature");
    let open = || {
        vec![CheckoutState {
            path: checkout.clone(),
            session: Some("w1".into()),
        }]
    };
    let git = MockGitProvider::default();
    let backend = MockBackend::native();
    checkouts(&backend, open());
    checkouts(&backend, open());
    checkouts(&backend, open());
    backend.remove_results.lock().unwrap().extend([
        Err(RemoveError::DirtyRequiresForce(MockError("dirty".into()))),
        Err(RemoveError::DirtyRequiresForce(MockError(
            "still dirty".into(),
        ))),
        Ok(Removed {
            warning: Some("note".into()),
        }),
    ]);

    assert_eq!(
        remove(&git, &backend, &checkout, false),
        WorktreeRemovalOutcome::DirtyRequiresForce
    );
    assert_eq!(
        remove(&git, &backend, &checkout, true),
        WorktreeRemovalOutcome::Failed("still dirty".into())
    );
    assert_eq!(
        remove(&git, &backend, &checkout, true),
        WorktreeRemovalOutcome::Removed {
            warning: Some("note".into())
        }
    );
    assert!(git.remove_calls.lock().unwrap().is_empty());
}

#[test]
fn removal_stops_when_the_open_state_cannot_be_confirmed() {
    let checkout = root().join("repo/feature");
    let git = MockGitProvider::default();
    let backend = managed();
    backend
        .checkouts_results
        .lock()
        .unwrap()
        .push_back(Err(MockError("offline".into())));
    checkouts(&backend, Vec::new());

    let unreachable = remove(&git, &backend, &checkout, false);
    let unlisted = remove(&git, &backend, &checkout, false);

    assert!(
        matches!(&unreachable, WorktreeRemovalOutcome::Failed(message) if message.contains("offline")),
        "{unreachable:?}"
    );
    assert!(
        matches!(&unlisted, WorktreeRemovalOutcome::Failed(message) if message.contains("was not returned by the test backend")),
        "{unlisted:?}"
    );
    assert!(git.remove_calls.lock().unwrap().is_empty());
}
