use std::path::PathBuf;

use crate::git::Repo;

/// Events that arrive asynchronously from background tasks.
/// These get merged into the main event loop alongside keyboard input.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Repository discovery completed
    ReposDiscovered { repos: Vec<Repo> },

    /// A background git operation completed successfully
    WorktreeCreated { path: PathBuf, session_name: String },

    /// A background git operation failed
    GitError(String),
}
