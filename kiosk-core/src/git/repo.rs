use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    #[allow(dead_code)]
    pub is_main: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::WORKTREE_DIR_NAME;

    fn make_repo(name: &str, session_name: &str) -> Repo {
        Repo {
            name: name.to_string(),
            session_name: session_name.to_string(),
            path: PathBuf::from(format!("/home/user/{name}")),
            worktrees: vec![],
        }
    }

    #[test]
    fn test_tmux_session_name_main_worktree() {
        let repo = make_repo("myrepo", "myrepo");
        let name = repo.tmux_session_name(&PathBuf::from("/home/user/myrepo"));
        assert_eq!(name, "myrepo");
    }

    #[test]
    fn test_tmux_session_name_main_worktree_dots_replaced() {
        let repo = make_repo("my.repo.rs", "my.repo.rs");
        let name = repo.tmux_session_name(&PathBuf::from("/home/user/my.repo.rs"));
        assert_eq!(name, "my_repo_rs");
    }

    #[test]
    fn test_tmux_session_name_branch_worktree() {
        let repo = make_repo("kiosk", "kiosk");
        let name = repo.tmux_session_name(&PathBuf::from(format!(
            "/home/user/{WORKTREE_DIR_NAME}/kiosk--feat-awesome"
        )));
        assert_eq!(name, "kiosk--feat-awesome");
    }

    #[test]
    fn test_tmux_session_name_disambiguated() {
        let repo = make_repo("api", "api--(Work)");
        let name = repo.tmux_session_name(&PathBuf::from("/home/user/Work/api"));
        assert_eq!(name, "api--(Work)");
    }

    #[test]
    fn test_tmux_session_name_disambiguated_worktree() {
        let repo = make_repo("api", "api--(Work)");
        let name = repo.tmux_session_name(&PathBuf::from(format!(
            "/home/user/{WORKTREE_DIR_NAME}/api--feat-thing"
        )));
        assert_eq!(name, "api--(Work)--feat-thing");
    }

    #[test]
    fn test_repo_and_worktree_serde_round_trip() {
        let repo = Repo {
            name: "demo".to_string(),
            session_name: "demo".to_string(),
            path: PathBuf::from("/tmp/demo"),
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/demo"),
                branch: Some("main".to_string()),
                is_main: true,
            }],
        };

        let json = serde_json::to_string(&repo).unwrap();
        let decoded: Repo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, repo.name);
        assert_eq!(decoded.path, repo.path);
        assert_eq!(decoded.worktrees[0].branch, repo.worktrees[0].branch);
    }
}
