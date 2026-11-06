use std::{path::PathBuf, sync::mpsc};

use crate::{
    backend::annotation::AnnotationIndex,
    git::{Repo, ScanWarning, Worktree},
    state::{BranchEntry, BranchId},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterTarget {
    Repos,
    Branches,
    Bases,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterKey {
    Repo(PathBuf),
    Branch(BranchId),
    Base(String),
    Help(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchOperationFailure {
    Failed(String),
    LocalBranchAvailable {
        branch_name: String,
        tracking_created: bool,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeRemovalOutcome {
    Removed { warning: Option<String> },
    DirtyRequiresForce,
    Failed(String),
}

/// Holds back the closing of a removed checkout's session until the picker has forgotten
/// the removal's pending record. Closing can end the picker (Rex closes the session the
/// picker floats in), and a record left behind would resume the removal on a later
/// checkout of the same branch at that path.
#[derive(Debug, Clone)]
pub struct CloseGate(mpsc::SyncSender<Result<(), String>>);

impl CloseGate {
    /// A gate, and the receiver the removal waits on until the picker decides.
    pub(crate) fn new() -> (Self, mpsc::Receiver<Result<(), String>>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (Self(sender), receiver)
    }

    /// Lets the removal close the session. Call only once the record is forgotten.
    pub(crate) fn open(self) {
        self.decide(Ok(()));
    }

    /// Keeps the session open, as the record could not be forgotten, saying why.
    pub(crate) fn hold(self, reason: String) {
        self.decide(Err(reason));
    }

    fn decide(self, decision: Result<(), String>) {
        // The removal has stopped waiting only if its thread is gone, and then there is
        // nothing left to close.
        let _ = self.0.send(decision);
    }
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    ReposFound {
        repo: Repo,
    },
    ScanComplete,
    /// The main checkout of the repository the picker was started in.
    CurrentRepoResolved {
        main_checkout: PathBuf,
    },
    CurrentRepoFailed(String),
    ScanWarning(ScanWarning),
    FilterCompleted {
        target: FilterTarget,
        generation: u64,
        matches: Vec<(FilterKey, i64)>,
        selected: Option<FilterKey>,
    },
    BranchesLoaded {
        repo_path: PathBuf,
        generation: u64,
        branches: Vec<BranchEntry>,
        worktrees: Vec<Worktree>,
        skipped_unsupported_refs: bool,
    },
    BranchLoadFailed {
        repo_path: PathBuf,
        generation: u64,
        message: String,
    },
    RemoteBranchesLoaded {
        repo_path: PathBuf,
        generation: u64,
        remote: String,
        branches: Vec<BranchEntry>,
        skipped_unsupported_refs: bool,
    },
    RemoteBranchLoadFailed {
        repo_path: PathBuf,
        generation: u64,
        message: String,
    },
    GitFetchCompleted {
        remote: Option<String>,
        branches: Vec<BranchEntry>,
        repo_path: PathBuf,
        generation: u64,
        error: Option<String>,
        is_final: bool,
        skipped_unsupported_refs: bool,
    },
    BranchNameValidated {
        repo_path: PathBuf,
        branch_name: String,
        valid: bool,
        error: Option<String>,
    },
    /// Git has removed a checkout that a session still shows. The removal closes the session
    /// once `gate` is opened, and then finishes with [`AppEvent::WorktreeRemovalFinished`].
    CheckoutRemoved {
        worktree_path: PathBuf,
        gate: CloseGate,
    },
    WorktreeRemovalFinished {
        repo_path: PathBuf,
        branch_name: String,
        worktree_path: PathBuf,
        outcome: WorktreeRemovalOutcome,
    },
    /// Main checkouts of the repositories the backend has something open for.
    OpenReposLoaded {
        roots: Vec<PathBuf>,
    },
    OpenFoldersLoaded {
        folders: Vec<PathBuf>,
    },
    OpenCheckoutsLoaded {
        repo_path: PathBuf,
        generation: u64,
        open: Vec<PathBuf>,
    },
    OpenCheckoutsFailed {
        repo_path: PathBuf,
        generation: u64,
        message: String,
    },
    RepoOpened {
        warning: Option<String>,
    },
    RepoOpenFailed(String),
    BranchOperationFailed {
        repo_path: PathBuf,
        failure: BranchOperationFailure,
    },
    OpenReposFailed(String),
    OpenFoldersFailed(String),
    /// The backend's labels changed.
    AnnotationsLoaded(AnnotationIndex),
    AnnotationsFailed(String),
}
