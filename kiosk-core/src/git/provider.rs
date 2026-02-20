use super::repo::{Repo, Worktree};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub trait GitProvider: Send + Sync {
    fn discover_repos(&self, dirs: &[PathBuf]) -> Vec<Repo>;
    fn list_branches(&self, repo_path: &Path) -> Vec<String>;
    fn list_worktrees(&self, repo_path: &Path) -> Vec<Worktree>;
    fn add_worktree(&self, repo_path: &Path, branch: &str, worktree_path: &Path) -> Result<()>;
    fn create_branch_and_worktree(
        &self,
        repo_path: &Path,
        new_branch: &str,
        base: &str,
        worktree_path: &Path,
    ) -> Result<()>;
}
