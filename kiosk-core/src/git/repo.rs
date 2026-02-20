use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    #[allow(dead_code)]
    pub is_main: bool,
}

#[derive(Debug, Clone)]
pub struct Repo {
    pub name: String,
    pub path: PathBuf,
    pub worktrees: Vec<Worktree>,
    /// Base name for tmux sessions. Usually same as `name`, but disambiguated
    /// with a parent dir suffix when multiple repos share the same name.
    pub session_name: String,
}

impl Repo {
    /// Tmux session name for a given branch/worktree path.
    /// For the main worktree, returns `session_name`.
    /// For other worktrees, returns `session_name--safe_branch`.
    pub fn tmux_session_name(&self, worktree_path: &Path) -> String {
        if worktree_path == self.path {
            self.session_name.replace('.', "_")
        } else {
            worktree_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                // Replace the repo name prefix with session_name to carry disambiguation
                .replacen(&self.name, &self.session_name, 1)
                .replace('.', "_")
        }
    }
}
