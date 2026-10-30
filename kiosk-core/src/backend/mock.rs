//! A scripted backend that records its calls, for tests.

use std::{
    collections::VecDeque,
    fmt,
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::{
    Backend, CheckoutState, KnownCheckout, ManagedWorktrees, NativeWorktrees, Opened, RemoveError,
    Removed, WorktreePlacement, Worktrees,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockError(pub String);

impl fmt::Display for MockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MockError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendCall {
    OpenRepos,
    OpenFolders,
    Checkouts {
        repo: PathBuf,
    },
    OpenCheckout {
        repo: PathBuf,
        checkout: PathBuf,
    },
    OpenFolder {
        folder: PathBuf,
    },
    Notify {
        title: String,
        body: String,
    },
    OpenBranch {
        repo: PathBuf,
        branch: String,
        known: KnownCheckout,
    },
    CreateBranch {
        repo: PathBuf,
        branch: String,
        base: String,
    },
    Remove {
        session: String,
        force: bool,
    },
    Close {
        session: String,
    },
}

/// Which worktree support the mock claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockWorktrees {
    Native,
    Managed(WorktreePlacement),
}

type Queue<T> = Mutex<VecDeque<Result<T, MockError>>>;

/// Every call is recorded, and answered with the next result queued for its method. A call
/// with nothing queued fails, so tests notice calls they did not expect.
pub struct MockBackend {
    pub worktree_support: MockWorktrees,
    pub calls: Mutex<Vec<BackendCall>>,
    pub open_repos_results: Queue<Vec<PathBuf>>,
    pub open_folders_results: Queue<Vec<PathBuf>>,
    pub checkouts_results: Queue<Vec<CheckoutState<String>>>,
    pub open_checkout_results: Queue<Opened>,
    pub open_folder_results: Queue<Opened>,
    pub notify_results: Queue<()>,
    pub open_branch_results: Queue<Opened>,
    pub create_branch_results: Queue<Opened>,
    pub remove_results: Mutex<VecDeque<Result<Removed, RemoveError<MockError>>>>,
    pub close_results: Queue<()>,
}

impl MockBackend {
    pub fn native() -> Self {
        Self::with(MockWorktrees::Native)
    }

    pub fn managed(root: impl Into<PathBuf>) -> Self {
        Self::with(MockWorktrees::Managed(
            WorktreePlacement::new(root.into()).expect("an absolute mock worktree root"),
        ))
    }

    fn with(worktree_support: MockWorktrees) -> Self {
        Self {
            worktree_support,
            calls: Mutex::default(),
            open_repos_results: Mutex::default(),
            open_folders_results: Mutex::default(),
            checkouts_results: Mutex::default(),
            open_checkout_results: Mutex::default(),
            open_folder_results: Mutex::default(),
            notify_results: Mutex::default(),
            open_branch_results: Mutex::default(),
            create_branch_results: Mutex::default(),
            remove_results: Mutex::default(),
            close_results: Mutex::default(),
        }
    }

    pub fn calls(&self) -> Vec<BackendCall> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: BackendCall) {
        self.calls.lock().unwrap().push(call);
    }
}

fn next<T>(queue: &Queue<T>, method: &str) -> Result<T, MockError> {
    queue
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| Err(MockError(format!("mock has no queued result for {method}"))))
}

impl Backend for MockBackend {
    type SessionId = String;
    type Error = MockError;

    fn open_repos(&self) -> Result<Vec<PathBuf>, MockError> {
        self.record(BackendCall::OpenRepos);
        next(&self.open_repos_results, "open_repos")
    }

    fn open_folders(&self) -> Result<Vec<PathBuf>, MockError> {
        self.record(BackendCall::OpenFolders);
        next(&self.open_folders_results, "open_folders")
    }

    fn checkouts(&self, repo: &Path) -> Result<Vec<CheckoutState<String>>, MockError> {
        self.record(BackendCall::Checkouts { repo: repo.into() });
        next(&self.checkouts_results, "checkouts")
    }

    fn open_checkout(&self, repo: &Path, checkout: &Path) -> Result<Opened, MockError> {
        self.record(BackendCall::OpenCheckout {
            repo: repo.into(),
            checkout: checkout.into(),
        });
        next(&self.open_checkout_results, "open_checkout")
    }

    fn open_folder(&self, folder: &Path) -> Result<Opened, MockError> {
        self.record(BackendCall::OpenFolder {
            folder: folder.into(),
        });
        next(&self.open_folder_results, "open_folder")
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), MockError> {
        self.record(BackendCall::Notify {
            title: title.into(),
            body: body.into(),
        });
        next(&self.notify_results, "notify")
    }

    fn worktrees(&self) -> Worktrees<'_, Self> {
        match self.worktree_support {
            MockWorktrees::Native => Worktrees::Native(self),
            MockWorktrees::Managed(_) => Worktrees::Managed(self),
        }
    }
}

impl NativeWorktrees<Self> for MockBackend {
    fn open_branch(
        &self,
        repo: &Path,
        branch: &str,
        known: KnownCheckout,
    ) -> Result<Opened, MockError> {
        self.record(BackendCall::OpenBranch {
            repo: repo.into(),
            branch: branch.into(),
            known,
        });
        next(&self.open_branch_results, "open_branch")
    }

    fn create_branch(&self, repo: &Path, branch: &str, base: &str) -> Result<Opened, MockError> {
        self.record(BackendCall::CreateBranch {
            repo: repo.into(),
            branch: branch.into(),
            base: base.into(),
        });
        next(&self.create_branch_results, "create_branch")
    }

    fn remove(&self, session: &String, force: bool) -> Result<Removed, RemoveError<MockError>> {
        self.record(BackendCall::Remove {
            session: session.clone(),
            force,
        });
        self.remove_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Err(RemoveError::Failed(MockError(
                    "mock has no queued result for remove".into(),
                )))
            })
    }
}

impl ManagedWorktrees<Self> for MockBackend {
    fn placement(&self) -> &WorktreePlacement {
        match &self.worktree_support {
            MockWorktrees::Managed(placement) => placement,
            MockWorktrees::Native => panic!("a native mock backend has no worktree placement"),
        }
    }

    fn close(&self, session: &String) -> Result<(), MockError> {
        self.record(BackendCall::Close {
            session: session.clone(),
        });
        next(&self.close_results, "close")
    }
}
