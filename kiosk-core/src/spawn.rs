use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use rayon::ThreadPoolBuilder;

use crate::{
    backend::{Backend, KnownCheckout, Opened, annotation::AnnotationIndex, worktree},
    config::ResolvedSearchDir,
    event::{AppEvent, BranchOperationFailure},
    git::{GitProvider, Repo, ScanWarning, is_local_branch_already_exists},
    state::BranchEntry,
};

/// Bounds concurrent remote fetches.
pub const FETCH_POOL_SIZE: usize = 4;

type FetchKey = (PathBuf, String);

#[derive(Clone, Default)]
pub struct FetchDeduplicator {
    in_flight: Arc<Mutex<HashSet<FetchKey>>>,
}

impl FetchDeduplicator {
    fn claim(&self, repo_path: &Path, remote: &str) -> Option<FetchClaim> {
        let key = (repo_path.to_path_buf(), remote.to_string());
        self.in_flight
            .lock()
            .unwrap()
            .insert(key.clone())
            .then(|| FetchClaim {
                key,
                in_flight: Arc::clone(&self.in_flight),
            })
    }
}

struct FetchClaim {
    key: FetchKey,
    in_flight: Arc<Mutex<HashSet<FetchKey>>>,
}

impl Drop for FetchClaim {
    fn drop(&mut self) {
        self.in_flight.lock().unwrap().remove(&self.key);
    }
}

#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<AppEvent>,
    cancel: Arc<AtomicBool>,
}

impl EventSender {
    pub fn new(tx: mpsc::Sender<AppEvent>, cancel: Arc<AtomicBool>) -> Self {
        Self { tx, cancel }
    }

    pub fn send(&self, event: AppEvent) -> bool {
        !self.is_cancelled() && self.tx.send(event).is_ok()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Run built work items on a bounded Rayon pool, with a bounded OS-thread
/// fallback if Rayon cannot initialize.
pub fn spawn_work_parallel<T, F, W>(
    pool_size: usize,
    items: impl IntoIterator<Item = T>,
    build_work: F,
) where
    F: Fn(T) -> W,
    W: FnOnce() + Send + 'static,
{
    assert!(pool_size > 0, "worker pool size must be non-zero");
    let pool = ThreadPoolBuilder::new().num_threads(pool_size).build().ok();
    if let Some(pool) = &pool {
        for item in items {
            pool.spawn(build_work(item));
        }
    } else {
        let work = items.into_iter().map(build_work).collect::<VecDeque<_>>();
        let worker_count = pool_size.min(work.len());
        let work = Arc::new(Mutex::new(work));
        for _ in 0..worker_count {
            let work = Arc::clone(&work);
            thread::spawn(move || {
                loop {
                    let Some(job) = work.lock().unwrap().pop_front() else {
                        break;
                    };
                    job();
                }
            });
        }
    }
}

pub fn spawn_repo_discovery(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    search_dirs: Vec<ResolvedSearchDir>,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.is_cancelled() {
            return;
        }

        thread::scope(|scope| {
            for search_dir in &search_dirs {
                let git = &git;
                let sender = &sender;
                scope.spawn(move || {
                    if sender.is_cancelled() {
                        return;
                    }
                    let result = git.scan_repos_streaming(
                        &search_dir.path,
                        search_dir.depth,
                        search_dir.include_non_git,
                        &|| sender.is_cancelled(),
                        &|repo| {
                            if sender.is_cancelled() {
                                return;
                            }
                            sender.send(AppEvent::ReposFound { repo });
                        },
                    );
                    match result {
                        Ok(warnings) => {
                            for warning in warnings {
                                sender.send(AppEvent::ScanWarning(warning));
                            }
                        }
                        Err(error) => {
                            sender.send(AppEvent::ScanWarning(ScanWarning {
                                path: search_dir.path.clone(),
                                message: format!("repository scan failed: {error:#}"),
                            }));
                        }
                    }
                });
            }
        });

        sender.send(AppEvent::ScanComplete);
    });
}

pub fn spawn_open_repos<B: Backend>(backend: &Arc<B>, sender: &EventSender) {
    let backend = Arc::clone(backend);
    let sender = sender.clone();
    thread::spawn(move || match backend.open_repos() {
        Ok(roots) => {
            sender.send(AppEvent::OpenReposLoaded { roots });
        }
        Err(error) => {
            sender.send(AppEvent::OpenReposFailed(format!(
                "could not load open workspace indicators: {error}"
            )));
        }
    });
}

/// Asks the backend's annotator for labels until the picker closes, reporting only
/// changes. A failure is reported once until a later refresh succeeds.
pub fn spawn_annotation_poller<B: Backend>(backend: &Arc<B>, sender: &EventSender) {
    if backend.annotator().is_none() {
        return;
    }
    let backend = Arc::clone(backend);
    let sender = sender.clone();
    thread::spawn(move || {
        let mut last: Option<AnnotationIndex> = None;
        let mut failing = false;
        while !sender.is_cancelled() {
            let Some(annotator) = backend.annotator() else {
                return;
            };
            match annotator.annotations() {
                Ok(annotations) => {
                    failing = false;
                    let index = AnnotationIndex::new(annotations);
                    if last.as_ref() != Some(&index) {
                        last = Some(index.clone());
                        sender.send(AppEvent::AnnotationsLoaded(index));
                    }
                }
                Err(error) => {
                    if !failing {
                        failing = true;
                        sender.send(AppEvent::AnnotationsFailed(format!(
                            "could not refresh status labels: {error}"
                        )));
                    }
                }
            }
            thread::sleep(annotator.interval());
        }
    });
}

pub fn spawn_open_folders<B: Backend>(backend: &Arc<B>, sender: &EventSender) {
    let backend = Arc::clone(backend);
    let sender = sender.clone();
    thread::spawn(move || match backend.open_folders() {
        Ok(folders) => {
            sender.send(AppEvent::OpenFoldersLoaded { folders });
        }
        Err(error) => {
            sender.send(AppEvent::OpenFoldersFailed(format!(
                "could not load open folder indicators: {error}"
            )));
        }
    });
}

/// Runs `open` on a new thread and reports a repository-level open, which closes the
/// picker on success.
fn spawn_repo_level_open<B: Backend>(
    backend: &Arc<B>,
    sender: &EventSender,
    open: impl FnOnce(&B) -> Result<Opened, B::Error> + Send + 'static,
) {
    let backend = Arc::clone(backend);
    let sender = sender.clone();
    thread::spawn(move || match open(&backend) {
        Ok(opened) => {
            sender.send(AppEvent::RepoOpened {
                warning: opened.warning,
            });
        }
        Err(error) => {
            sender.send(AppEvent::RepoOpenFailed(error.to_string()));
        }
    });
}

pub fn spawn_open_repo<B: Backend>(backend: &Arc<B>, sender: &EventSender, repo_path: PathBuf) {
    spawn_repo_level_open(backend, sender, move |backend| {
        backend.open_checkout(&repo_path, &repo_path)
    });
}

pub fn spawn_open_folder<B: Backend>(backend: &Arc<B>, sender: &EventSender, folder_path: PathBuf) {
    spawn_repo_level_open(backend, sender, move |backend| {
        backend.open_folder(&crate::path::canonical_or_original(&folder_path))
    });
}

pub fn spawn_branch_loading(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    mut repo: Repo,
    cwd: Option<PathBuf>,
    generation: u64,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        let repo_path = repo.path.clone();
        let result = (|| {
            let local_names = git.list_branches(&repo.path)?;
            let default_branch = git.default_branch(&repo.path, &local_names.items)?;
            let worktrees = git.list_worktrees(&repo.path)?;
            repo.worktrees = worktrees.items;
            let branches = BranchEntry::build_local(
                &repo,
                &local_names.items,
                default_branch.as_deref(),
                cwd.as_deref(),
            );
            Ok::<_, anyhow::Error>((
                branches,
                std::mem::take(&mut repo.worktrees),
                local_names.skipped_unsupported_refs || worktrees.skipped_unsupported_refs,
            ))
        })();

        match result {
            Ok((branches, worktrees, skipped_unsupported_refs)) => {
                sender.send(AppEvent::BranchesLoaded {
                    repo_path,
                    generation,
                    branches,
                    worktrees,
                    skipped_unsupported_refs,
                });
            }
            Err(error) => {
                sender.send(AppEvent::BranchLoadFailed {
                    repo_path,
                    generation,
                    message: format!("could not load branches: {error:#}"),
                });
            }
        }
    });
}

pub fn spawn_remote_branch_loading(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    local_names: Vec<String>,
    generation: u64,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        let remotes = match git.list_remotes(&repo_path) {
            Ok(remotes) => remotes,
            Err(error) => {
                sender.send(AppEvent::RemoteBranchLoadFailed {
                    repo_path,
                    generation,
                    message: format!("could not list remote branches: {error:#}"),
                });
                return;
            }
        };
        for remote in remotes {
            if sender.is_cancelled() {
                return;
            }
            match git.list_remote_branches_for_remote(&repo_path, &remote) {
                Ok(remote_names) => {
                    sender.send(AppEvent::RemoteBranchesLoaded {
                        repo_path: repo_path.clone(),
                        generation,
                        branches: BranchEntry::build_remote(
                            &remote,
                            &remote_names.items,
                            &local_names,
                        ),
                        skipped_unsupported_refs: remote_names.skipped_unsupported_refs,
                        remote,
                    });
                }
                Err(error) => {
                    sender.send(AppEvent::RemoteBranchLoadFailed {
                        repo_path: repo_path.clone(),
                        generation,
                        message: format!("could not list branches for remote {remote}: {error:#}"),
                    });
                }
            }
        }
    });
}

pub fn spawn_git_fetch(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    deduplicator: &FetchDeduplicator,
    repo_path: PathBuf,
    local_names: Vec<String>,
    generation: u64,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    let deduplicator = deduplicator.clone();
    thread::spawn(move || {
        let remotes = match git.list_remotes(&repo_path) {
            Ok(remotes) => remotes,
            Err(error) => {
                sender.send(AppEvent::GitFetchCompleted {
                    remote: None,
                    branches: Vec::new(),
                    repo_path,
                    generation,
                    error: Some(format!("could not list remotes for fetch: {error:#}")),
                    is_final: true,
                    skipped_unsupported_refs: false,
                });
                return;
            }
        };
        let claimed = remotes
            .into_iter()
            .filter_map(|remote| {
                deduplicator
                    .claim(&repo_path, &remote)
                    .map(|claim| (remote, claim))
            })
            .collect::<Vec<_>>();
        if claimed.is_empty() {
            sender.send(AppEvent::GitFetchCompleted {
                remote: None,
                branches: Vec::new(),
                repo_path,
                generation,
                error: None,
                is_final: true,
                skipped_unsupported_refs: false,
            });
            return;
        }

        let remaining = Arc::new(AtomicUsize::new(claimed.len()));
        let local_names = Arc::new(local_names);
        spawn_work_parallel(FETCH_POOL_SIZE, claimed, |(remote, claim)| {
            let git = Arc::clone(&git);
            let sender = sender.clone();
            let repo_path = repo_path.clone();
            let local_names = Arc::clone(&local_names);
            let remaining = Arc::clone(&remaining);
            move || {
                let _claim = claim;
                let (branches, error, skipped_unsupported_refs) = if sender.is_cancelled() {
                    (Vec::new(), None, false)
                } else {
                    match git.fetch_remote(&repo_path, &remote) {
                        Ok(()) => match git.list_remote_branches_for_remote(&repo_path, &remote) {
                            Ok(remote_names) => (
                                BranchEntry::build_remote(
                                    &remote,
                                    &remote_names.items,
                                    &local_names,
                                ),
                                None,
                                remote_names.skipped_unsupported_refs,
                            ),
                            Err(error) => (
                                Vec::new(),
                                Some(format!(
                                    "fetch succeeded but branches could not be refreshed: {error:#}"
                                )),
                                false,
                            ),
                        },
                        Err(error) => (Vec::new(), Some(error.to_string()), false),
                    }
                };
                let is_final = remaining.fetch_sub(1, Ordering::AcqRel) == 1;
                sender.send(AppEvent::GitFetchCompleted {
                    remote: Some(remote),
                    branches,
                    repo_path,
                    generation,
                    error,
                    is_final,
                    skipped_unsupported_refs,
                });
            }
        });
    });
}

pub fn spawn_open_checkouts<B: Backend>(
    backend: &Arc<B>,
    sender: &EventSender,
    repo_path: PathBuf,
    generation: u64,
) {
    let backend = Arc::clone(backend);
    let sender = sender.clone();
    thread::spawn(move || match backend.checkouts(&repo_path) {
        Ok(checkouts) => {
            sender.send(AppEvent::OpenCheckoutsLoaded {
                repo_path,
                generation,
                open: checkouts
                    .into_iter()
                    .filter(|checkout| checkout.session.is_some())
                    .map(|checkout| checkout.path)
                    .collect(),
            });
        }
        Err(error) => {
            sender.send(AppEvent::OpenCheckoutsFailed {
                repo_path,
                generation,
                message: format!("could not load open branch indicators: {error}"),
            });
        }
    });
}

/// Runs a branch operation on a new thread. Success closes the picker; `on_failure` turns an
/// error message into the failure the branch view shows.
fn spawn_branch_operation<B: Backend>(
    git: &Arc<dyn GitProvider>,
    backend: &Arc<B>,
    sender: &EventSender,
    repo_path: PathBuf,
    operation: impl FnOnce(&dyn GitProvider, &B, &Path) -> Result<Opened, BranchOperationFailure>
    + Send
    + 'static,
) {
    let git = Arc::clone(git);
    let backend = Arc::clone(backend);
    let sender = sender.clone();
    thread::spawn(
        move || match operation(git.as_ref(), &backend, &repo_path) {
            Ok(opened) => {
                sender.send(AppEvent::RepoOpened {
                    warning: opened.warning,
                });
            }
            Err(failure) => {
                sender.send(AppEvent::BranchOperationFailed { repo_path, failure });
            }
        },
    );
}

pub fn spawn_open_branch<B: Backend>(
    git: &Arc<dyn GitProvider>,
    backend: &Arc<B>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
    known: KnownCheckout,
) {
    spawn_branch_operation(
        git,
        backend,
        sender,
        repo_path,
        move |git, backend, repo| {
            worktree::open_branch(git, backend, repo, &branch_name, known)
                .map_err(|error| BranchOperationFailure::Failed(error.to_string()))
        },
    );
}

pub fn spawn_validate_branch_name(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(
        move || match git.is_valid_branch_name(&repo_path, &branch_name) {
            Ok(valid) => {
                sender.send(AppEvent::BranchNameValidated {
                    repo_path,
                    branch_name,
                    valid,
                    error: None,
                });
            }
            Err(error) => {
                sender.send(AppEvent::BranchNameValidated {
                    repo_path,
                    branch_name,
                    valid: false,
                    error: Some(format!("could not validate branch name: {error:#}")),
                });
            }
        },
    );
}

pub fn spawn_create_new_branch<B: Backend>(
    git: &Arc<dyn GitProvider>,
    backend: &Arc<B>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
    base: String,
) {
    spawn_branch_operation(
        git,
        backend,
        sender,
        repo_path,
        move |git, backend, repo| {
            worktree::create_branch(git, backend, repo, &branch_name, &base)
                .map_err(|error| BranchOperationFailure::Failed(error.to_string()))
        },
    );
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_worktree_removal<B: Backend>(
    git: &Arc<dyn GitProvider>,
    backend: &Arc<B>,
    sender: &EventSender,
    backend_name: &'static str,
    repo_path: PathBuf,
    branch_name: String,
    worktree_path: PathBuf,
    force: bool,
) {
    let git = Arc::clone(git);
    let backend = Arc::clone(backend);
    let sender = sender.clone();
    thread::spawn(move || {
        let outcome = worktree::remove_checkout(
            git.as_ref(),
            backend.as_ref(),
            backend_name,
            &repo_path,
            &worktree_path,
            force,
        );
        sender.send(AppEvent::WorktreeRemovalFinished {
            repo_path,
            branch_name,
            worktree_path,
            outcome,
        });
    });
}

/// Creates a local branch tracking `remote`'s branch of the same name, then opens it.
pub fn spawn_open_remote_branch<B: Backend>(
    git: &Arc<dyn GitProvider>,
    backend: &Arc<B>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
    remote: String,
) {
    spawn_branch_operation(
        git,
        backend,
        sender,
        repo_path,
        move |git, backend, repo| {
            let tracking_created = match git.create_tracking_branch(repo, &branch_name, &remote) {
                Ok(()) => true,
                Err(error) if is_local_branch_already_exists(&error) => false,
                Err(error) => return Err(BranchOperationFailure::Failed(error.to_string())),
            };
            worktree::open_branch(git, backend, repo, &branch_name, KnownCheckout::Missing).map_err(
                |error| BranchOperationFailure::LocalBranchAvailable {
                    branch_name: branch_name.clone(),
                    tracking_created,
                    message: error.to_string(),
                },
            )
        },
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    use crate::{
        backend::{
            CheckoutState,
            mock::{BackendCall, MockBackend, MockError},
        },
        event::WorktreeRemovalOutcome,
        git::{Repo, Worktree, mock::MockGitProvider},
    };

    use super::*;

    #[test]
    fn annotation_poller_reports_changes_and_one_failure_per_failing_run() {
        use crate::backend::{Annotations, Label, Tone};

        let annotations = |text: &str| Annotations {
            repos: vec![(
                PathBuf::from("/repo"),
                vec![Label {
                    text: text.into(),
                    tone: Tone::Progress,
                }],
            )],
            checkouts: Vec::new(),
        };
        let mut backend = MockBackend::native();
        backend.annotates = true;
        backend.annotations_results = Mutex::new(
            [
                Ok(annotations("a")),
                Ok(annotations("a")),
                Err(MockError("down".into())),
                Err(MockError("down".into())),
                Ok(annotations("b")),
            ]
            .into(),
        );
        let backend = Arc::new(backend);
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        spawn_annotation_poller(&backend, &sender);

        let mut received = Vec::new();
        while received.len() < 3 {
            received.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        sender.cancel();
        let labels = |event: &AppEvent| match event {
            AppEvent::AnnotationsLoaded(index) => index.repo(Path::new("/repo"))[0].text.clone(),
            AppEvent::AnnotationsFailed(message) => message.clone(),
            other => panic!("unexpected event {other:?}"),
        };
        assert_eq!(
            received.iter().map(labels).collect::<Vec<_>>(),
            ["a", "could not refresh status labels: down", "b"]
        );
    }

    #[test]
    fn backends_without_an_annotator_are_not_polled() {
        let backend = Arc::new(MockBackend::native());
        let (tx, rx) = mpsc::channel();
        spawn_annotation_poller(
            &backend,
            &EventSender::new(tx, Arc::new(AtomicBool::new(false))),
        );
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn parallel_work_runs_every_item() {
        let (tx, rx) = mpsc::channel();
        spawn_work_parallel(2, 0..6, |item| {
            let tx = tx.clone();
            move || tx.send(item).unwrap()
        });
        drop(tx);
        let mut received: Vec<_> = rx.iter().collect();
        received.sort_unstable();
        assert_eq!(received, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn event_sender_stops_after_cancellation() {
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        assert!(sender.send(AppEvent::ReposFound {
            repo: Repo {
                name: "repo".into(),
                path: "/repo".into(),
                is_git: true,
                worktrees: Vec::new(),
            },
        }));
        sender.cancel();
        assert!(!sender.send(AppEvent::ScanWarning(ScanWarning {
            path: PathBuf::new(),
            message: "cancelled".into(),
        })));
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn repo_discovery_streams_stubs_without_listing_worktrees() {
        let git_mock = Arc::new(MockGitProvider {
            repos: vec![Repo {
                name: "repo".into(),
                path: "/repo".into(),
                is_git: true,
                worktrees: vec![Worktree {
                    path: "/repo".into(),
                    branch: Some("main".into()),
                }],
            }],
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_repo_discovery(
            &git,
            &sender,
            vec![ResolvedSearchDir {
                path: "/search".into(),
                depth: 1,
                include_non_git: false,
            }],
        );

        let AppEvent::ReposFound { repo } = rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
            panic!("repository was not streamed")
        };
        assert_eq!(repo.path, Path::new("/repo"));
        assert_eq!(repo.worktrees, vec![]);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::ScanComplete
        ));
        assert!(git_mock.list_worktree_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn branch_loading_uses_a_fresh_worktree_listing() {
        let git_mock = Arc::new(MockGitProvider {
            branches: vec!["main".into(), "feature".into()],
            worktrees: vec![
                Worktree {
                    path: "/repo".into(),
                    branch: Some("main".into()),
                },
                Worktree {
                    path: "/repo-feature".into(),
                    branch: Some("feature".into()),
                },
            ],
            default_branch: Some("main".into()),
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_branch_loading(
            &git,
            &sender,
            Repo {
                name: "repo".into(),
                path: "/repo".into(),
                is_git: true,
                worktrees: Vec::new(),
            },
            Some("/repo-feature/src".into()),
            7,
        );

        let AppEvent::BranchesLoaded {
            repo_path,
            generation,
            branches,
            worktrees,
            ..
        } = rx.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("branches were not loaded")
        };
        assert_eq!(repo_path, Path::new("/repo"));
        assert_eq!(generation, 7);
        assert_eq!(worktrees.len(), 2);
        let feature = branches
            .iter()
            .find(|branch| branch.name == "feature")
            .unwrap();
        assert_eq!(feature.worktree_path(), Some(Path::new("/repo-feature")));
        assert!(feature.is_current);
        assert_eq!(
            *git_mock.list_worktree_calls.lock().unwrap(),
            [PathBuf::from("/repo")]
        );
    }

    #[test]
    fn remote_branch_loading_streams_each_remote_with_repo_scope() {
        let git = Arc::new(MockGitProvider {
            remotes: vec!["origin".into(), "upstream".into()],
            remote_branches_by_remote: HashMap::from([
                ("origin".into(), vec!["main".into(), "one".into()]),
                ("upstream".into(), vec!["two".into()]),
            ]),
            ..MockGitProvider::default()
        }) as Arc<dyn GitProvider>;
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_remote_branch_loading(&git, &sender, "/repo".into(), vec!["main".into()], 7);

        let events = [
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        assert!(matches!(
            &events[0],
            AppEvent::RemoteBranchesLoaded { repo_path, generation, remote, branches, .. }
                if repo_path == Path::new("/repo")
                    && *generation == 7
                    && remote == "origin"
                    && branches.iter().map(|branch| branch.name.as_str()).eq(["one"])
        ));
        assert!(matches!(
            &events[1],
            AppEvent::RemoteBranchesLoaded { repo_path, generation, remote, branches, .. }
                if repo_path == Path::new("/repo")
                    && *generation == 7
                    && remote == "upstream"
                    && branches.iter().map(|branch| branch.name.as_str()).eq(["two"])
        ));
    }

    #[test]
    fn git_fetch_streams_completions_and_marks_exactly_one_final() {
        let git_mock = Arc::new(MockGitProvider {
            remotes: vec!["origin".into(), "upstream".into()],
            remote_branches_by_remote: HashMap::from([
                ("origin".into(), vec!["one".into()]),
                ("upstream".into(), vec!["two".into()]),
            ]),
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_git_fetch(
            &git,
            &sender,
            &FetchDeduplicator::default(),
            "/repo".into(),
            Vec::new(),
            7,
        );

        let events = [
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AppEvent::GitFetchCompleted { is_final: true, .. }))
                .count(),
            1
        );
        let mut calls = git_mock.fetch_calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(
            calls,
            [
                (PathBuf::from("/repo"), "origin".into()),
                (PathBuf::from("/repo"), "upstream".into()),
            ]
        );
    }

    #[test]
    fn rapid_reentry_does_not_start_a_duplicate_in_flight_fetch() {
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let git_mock = Arc::new(MockGitProvider {
            remotes: vec!["origin".into()],
            fetch_gate: Some(Arc::clone(&gate)),
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        let deduplicator = FetchDeduplicator::default();

        spawn_git_fetch(&git, &sender, &deduplicator, "/repo".into(), Vec::new(), 7);
        let deadline = Instant::now() + Duration::from_secs(1);
        while git_mock.fetch_calls.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(git_mock.fetch_calls.lock().unwrap().len(), 1);

        spawn_git_fetch(&git, &sender, &deduplicator, "/repo".into(), Vec::new(), 8);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::GitFetchCompleted {
                generation: 8,
                remote: None,
                is_final: true,
                ..
            }
        ));
        assert_eq!(git_mock.fetch_calls.lock().unwrap().len(), 1);

        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::GitFetchCompleted { generation: 7, .. }
        ));
    }

    #[test]
    fn prune_failure_after_removal_reports_success_with_warning() {
        let git_mock = Arc::new(MockGitProvider::default());
        *git_mock.prune_failure.lock().unwrap() = Some("prune broke".into());
        let git: Arc<dyn GitProvider> = git_mock;
        let backend = Arc::new(MockBackend::native());
        backend
            .checkouts_results
            .lock()
            .unwrap()
            .push_back(Ok(vec![CheckoutState {
                path: "/repo-feature".into(),
                session: None,
            }]));
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_worktree_removal(
            &git,
            &backend,
            &sender,
            "the test backend",
            "/repo".into(),
            "feature".into(),
            "/repo-feature".into(),
            false,
        );

        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            AppEvent::WorktreeRemovalFinished {
                outcome: WorktreeRemovalOutcome::Removed { warning: Some(message) },
                ..
            } if message.contains("prune broke")
        ));
    }

    #[test]
    fn remote_branches_get_a_tracking_branch_then_a_kiosk_made_checkout() {
        let git_mock = Arc::new(MockGitProvider::default());
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let root = std::env::temp_dir().join("kiosk-worktrees");
        let backend = Arc::new(MockBackend::managed(&root));
        backend
            .open_checkout_results
            .lock()
            .unwrap()
            .push_back(Ok(Opened::default()));
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        let repo = std::env::temp_dir().join("repo");

        spawn_open_remote_branch(
            &git,
            &backend,
            &sender,
            repo.clone(),
            "feature".into(),
            "upstream".into(),
        );

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::RepoOpened { warning: None }
        ));
        assert_eq!(
            *git_mock.tracking_calls.lock().unwrap(),
            [(repo.clone(), "feature".into(), "upstream".into())]
        );
        let checkout = root.join("repo").join("feature");
        assert_eq!(
            *git_mock.add_worktree_calls.lock().unwrap(),
            [(repo.clone(), checkout.clone(), "feature".into(), None)]
        );
        assert_eq!(
            backend.calls(),
            [BackendCall::OpenCheckout { repo, checkout }]
        );
    }

    #[test]
    fn a_failed_open_after_tracking_says_the_local_branch_now_exists() {
        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let backend = Arc::new(MockBackend::native());
        backend
            .open_branch_results
            .lock()
            .unwrap()
            .push_back(Err(MockError("boom".into())));
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_open_remote_branch(
            &git,
            &backend,
            &sender,
            "/repo".into(),
            "feature".into(),
            "origin".into(),
        );

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::BranchOperationFailed {
                failure: BranchOperationFailure::LocalBranchAvailable {
                    tracking_created: true,
                    message,
                    ..
                },
                ..
            } if message == "boom"
        ));
        assert_eq!(
            backend.calls(),
            [BackendCall::OpenBranch {
                repo: "/repo".into(),
                branch: "feature".into(),
                known: KnownCheckout::Missing,
            }]
        );
    }
}
