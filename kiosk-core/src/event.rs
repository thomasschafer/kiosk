use std::path::PathBuf;

/// Events that arrive asynchronously from background tasks.
/// These get merged into the main event loop alongside keyboard input.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A background git operation completed successfully
    WorktreeCreated { path: PathBuf },

    /// A background git operation failed
    GitError(String),

    /// Repos finished loading (for future async discovery)
    ReposLoaded(Vec<crate::git::Repo>),
}
