use super::repo::{Repo, Worktree};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub trait GitProvider: Send + Sync {
    /// Fast directory scan: returns repos with empty worktrees (no git calls).
    fn scan_repos(&self, dirs: &[(PathBuf, u16)]) -> Vec<Repo>;
    /// Full discovery: dir scan + worktree enrichment (calls git per repo).
    fn discover_repos(&self, dirs: &[(PathBuf, u16)]) -> Vec<Repo>;
    fn list_branches(&self, repo_path: &Path) -> Vec<String>;
    fn list_remote_branches(&self, repo_path: &Path) -> Vec<String>;
    fn list_worktrees(&self, repo_path: &Path) -> Vec<Worktree>;
    fn add_worktree(&self, repo_path: &Path, branch: &str, worktree_path: &Path) -> Result<()>;
    fn create_branch_and_worktree(
        &self,
        repo_path: &Path,
        new_branch: &str,
        base: &str,
        worktree_path: &Path,
    ) -> Result<()>;
    fn remove_worktree(&self, worktree_path: &Path) -> Result<()>;
    /// Create a local tracking branch from a remote branch and add a worktree for it
    fn create_tracking_branch_and_worktree(
        &self,
        repo_path: &Path,
        branch: &str,
        worktree_path: &Path,
    ) -> Result<()>;
    /// Detect the default branch (main/master) for a repository.
    /// Accepts the already-fetched local branch list to avoid redundant git calls in the fallback.
    fn default_branch(&self, repo_path: &Path, local_branches: &[String]) -> Option<String>;
    /// Resolve the current working directory to a git repository root
    fn resolve_repo_from_cwd(&self) -> Option<PathBuf>;
}
