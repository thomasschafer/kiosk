use std::{collections::HashMap, path::PathBuf};

use crate::git::{Repo, Worktree};

/// Events that arrive asynchronously from background tasks.
/// These get merged into the main event loop alongside keyboard input.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Repository discovery completed
    ReposDiscovered {
        repos: Vec<Repo>,
        session_activity: HashMap<String, u64>,
    },

    /// A background git operation completed successfully
    WorktreeCreated { path: PathBuf, session_name: String },

    /// A worktree was successfully removed
    WorktreeRemoved {
        branch_name: String,
        worktree_path: PathBuf,
    },

    /// A worktree removal failed
    WorktreeRemoveFailed {
        branch_name: String,
        worktree_path: PathBuf,
        error: String,
    },

    /// Local branches loaded
    BranchesLoaded {
        branches: Vec<crate::state::BranchEntry>,
        worktrees: Vec<crate::git::Worktree>,
        /// Local branch names, needed to spawn remote branch loading
        local_names: Vec<String>,
        /// Session activity timestamps
        session_activity: HashMap<String, u64>,
    },

    /// Remote branches loaded (appended after local)
    RemoteBranchesLoaded {
        branches: Vec<crate::state::BranchEntry>,
    },

    /// Background worktree enrichment completed (phase 2 of discovery)
    ReposEnriched {
        worktrees_by_repo: Vec<(PathBuf, Vec<Worktree>)>,
        session_activity: HashMap<String, u64>,
    },

    /// A background git operation failed
    GitError(String),
}
