use crate::git::Repo;
use std::path::PathBuf;

/// Rich branch entry with worktree and session metadata
#[derive(Debug, Clone)]
pub struct BranchEntry {
    pub name: String,
    /// If a worktree already exists for this branch
    pub worktree_path: Option<PathBuf>,
    pub has_session: bool,
    pub is_current: bool,
}

/// What mode the app is in
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    RepoSelect,
    BranchSelect,
    NewBranchBase,
    /// Blocking loading state — shows spinner, no input except Ctrl+C
    Loading(String),
}

/// The new-branch flow state
#[derive(Debug, Clone)]
pub struct NewBranchFlow {
    /// The new branch name (what the user typed)
    pub new_name: String,
    /// Base branches to pick from
    pub bases: Vec<String>,
    pub filtered: Vec<(usize, i64)>,
    pub selected: Option<usize>,
    pub search: String,
}

/// Central application state. Components read from this, actions modify it.
#[derive(Debug, Clone)]
pub struct AppState {
    pub repos: Vec<Repo>,
    pub filtered_repos: Vec<(usize, i64)>,
    pub repo_selected: Option<usize>,
    pub repo_search: String,

    pub selected_repo_idx: Option<usize>,
    pub branches: Vec<BranchEntry>,
    pub filtered_branches: Vec<(usize, i64)>,
    pub branch_selected: Option<usize>,
    pub branch_search: String,

    pub new_branch_base: Option<NewBranchFlow>,

    pub split_command: Option<String>,
    pub mode: Mode,
    pub error: Option<String>,
}

impl AppState {
    pub fn new(repos: Vec<Repo>, split_command: Option<String>) -> Self {
        let filtered_repos: Vec<(usize, i64)> =
            repos.iter().enumerate().map(|(i, _)| (i, 0)).collect();
        let repo_selected = if filtered_repos.is_empty() {
            None
        } else {
            Some(0)
        };

        Self {
            repos,
            filtered_repos,
            repo_selected,
            repo_search: String::new(),
            selected_repo_idx: None,
            branches: Vec::new(),
            filtered_branches: Vec::new(),
            branch_selected: None,
            branch_search: String::new(),
            new_branch_base: None,
            split_command,
            mode: Mode::RepoSelect,
            error: None,
        }
    }
}

/// Determine where to put a new worktree for a branch, avoiding collisions.
///
/// Worktrees are placed in `.kiosk_worktrees/` inside the repo's parent directory:
/// ```text
/// ~/Development/.kiosk_worktrees/kiosk--feat-awesome/
/// ~/Development/.kiosk_worktrees/scooter--fix-bug/
/// ```
pub fn worktree_dir(repo: &Repo, branch: &str) -> anyhow::Result<PathBuf> {
    let parent = repo.path.parent().unwrap_or(&repo.path);
    let worktree_root = parent.join(".kiosk_worktrees");
    let safe_branch = branch.replace('/', "-");
    let base = format!("{}--{safe_branch}", repo.name);
    let candidate = worktree_root.join(&base);
    if !candidate.exists() {
        return Ok(candidate);
    }
    for i in 2..1000 {
        let candidate = worktree_root.join(format!("{base}-{i}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("Could not find an available worktree directory name after 1000 attempts")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo;
    use std::fs;
    use tempfile::tempdir;

    fn make_repo(dir: &std::path::Path, name: &str) -> Repo {
        Repo {
            name: name.to_string(),
            session_name: name.to_string(),
            path: dir.join(name),
            worktrees: vec![],
        }
    }

    #[test]
    fn test_worktree_dir_basic() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "myrepo");
        let result = worktree_dir(&repo, "main").unwrap();
        assert_eq!(
            result,
            tmp.path().join(".kiosk_worktrees").join("myrepo--main")
        );
    }

    #[test]
    fn test_worktree_dir_slash_in_branch() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let result = worktree_dir(&repo, "feat/awesome").unwrap();
        assert_eq!(
            result,
            tmp.path()
                .join(".kiosk_worktrees")
                .join("repo--feat-awesome")
        );
    }

    #[test]
    fn test_worktree_dir_dedup() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let first = tmp.path().join(".kiosk_worktrees").join("repo--main");
        fs::create_dir_all(&first).unwrap();
        let result = worktree_dir(&repo, "main").unwrap();
        assert_eq!(
            result,
            tmp.path().join(".kiosk_worktrees").join("repo--main-2")
        );
    }

    #[test]
    fn test_worktree_dir_bounded_error() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let wt_root = tmp.path().join(".kiosk_worktrees");
        // Create the base and 2..999 suffixed dirs to exhaust the loop
        fs::create_dir_all(wt_root.join("repo--main")).unwrap();
        for i in 2..1000 {
            fs::create_dir_all(wt_root.join(format!("repo--main-{i}"))).unwrap();
        }
        let result = worktree_dir(&repo, "main");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("1000 attempts"));
    }

    #[test]
    fn test_worktree_dir_in_kiosk_worktrees_subdir() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "myrepo");
        let result = worktree_dir(&repo, "dev").unwrap();
        assert!(result.to_string_lossy().contains(".kiosk_worktrees"));
    }
}
