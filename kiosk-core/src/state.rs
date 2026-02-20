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

/// Determine where to put a new worktree for a branch, avoiding collisions
pub fn worktree_dir(repo: &Repo, branch: &str) -> PathBuf {
    let parent = repo.path.parent().unwrap_or(&repo.path);
    let safe_branch = branch.replace('/', "-");
    let base = format!("{}-{safe_branch}", repo.name);
    let candidate = parent.join(&base);
    if !candidate.exists() {
        return candidate;
    }
    for i in 2.. {
        let candidate = parent.join(format!("{base}-{i}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}
