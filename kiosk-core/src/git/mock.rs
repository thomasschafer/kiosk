use super::{
    provider::GitProvider,
    repo::{Repo, Worktree},
};
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Default)]
pub struct MockGitProvider {
    pub repos: Vec<Repo>,
    pub branches: Vec<String>,
    pub worktrees: Vec<Worktree>,
    pub add_worktree_result: Mutex<Option<Result<()>>>,
    pub create_branch_result: Mutex<Option<Result<()>>>,
}

impl GitProvider for MockGitProvider {
    fn discover_repos(&self, _dirs: &[PathBuf]) -> Vec<Repo> {
        self.repos.clone()
    }

    fn list_branches(&self, _repo_path: &Path) -> Vec<String> {
        self.branches.clone()
    }

    fn list_worktrees(&self, _repo_path: &Path) -> Vec<Worktree> {
        self.worktrees.clone()
    }

    fn add_worktree(&self, _repo_path: &Path, _branch: &str, _worktree_path: &Path) -> Result<()> {
        self.add_worktree_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn create_branch_and_worktree(
        &self,
        _repo_path: &Path,
        _new_branch: &str,
        _base: &str,
        _worktree_path: &Path,
    ) -> Result<()> {
        self.create_branch_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }
}
