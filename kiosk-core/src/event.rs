use std::{collections::HashMap, path::PathBuf};

use crate::{
    agent::AgentStatus,
    git::{Repo, Worktree},
};

/// Events that arrive asynchronously from background tasks.
/// These get merged into the main event loop alongside keyboard input.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Repository discovery completed (full batch — replaces repo list)
    ReposDiscovered {
        repos: Vec<Repo>,
        session_activity: HashMap<String, u64>,
    },

    /// Single repo discovered during streaming scan (appended to existing list)
    ReposFound { repo: Repo },

    /// All scan threads finished — triggers collision resolution and final sort.
    /// Carries `search_dirs` so collision resolution can use the correct search dir names.
    ScanComplete { search_dirs: Vec<(PathBuf, u16)> },

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

    /// Background git fetch completed for one remote (or all remotes if `is_final`).
    GitFetchCompleted {
        branches: Vec<crate::state::BranchEntry>,
        repo_path: PathBuf,
        /// True when this is the last remote to finish, so the UI can clear the spinner.
        is_final: bool,
    },

    /// Single repo enriched with worktree data (streamed from phase 2)
    RepoEnriched {
        repo_path: PathBuf,
        worktrees: Vec<Worktree>,
    },

    /// Session activity data loaded (from tmux, sent once)
    SessionActivityLoaded {
        session_activity: HashMap<String, u64>,
    },

    /// Agent states updated from background detection (full snapshot —
    /// `None` means no agent detected, allowing stale statuses to be cleared)
    AgentStatesUpdated {
        states: Vec<(String, Option<AgentStatus>)>,
    },

    /// A background git operation failed
    GitError(String),
}
