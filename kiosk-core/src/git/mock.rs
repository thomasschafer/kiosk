use super::{
    provider::GitProvider,
    repo::{Repo, Worktree},
};
use anyhow::Result;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Default)]
pub struct MockGitProvider {
    pub repos: Vec<Repo>,
    pub branches: Vec<String>,
    pub remote_branches: Vec<String>,
    pub worktrees: Vec<Worktree>,
    pub add_worktree_result: Mutex<Option<Result<()>>>,
    pub create_branch_result: Mutex<Option<Result<()>>>,
    pub remove_worktree_result: Mutex<Option<Result<()>>>,
    pub prune_worktrees_result: Mutex<Option<Result<()>>>,
    pub prune_worktrees_calls: Mutex<Vec<PathBuf>>,
    pub remotes: Vec<String>,
    pub fetch_remote_results: Mutex<HashMap<(PathBuf, String), Result<()>>>,
    pub fetch_remote_calls: Mutex<Vec<(PathBuf, String)>>,
    pub default_branch: Option<String>,
    pub current_repo_path: Option<PathBuf>,
}

impl GitProvider for MockGitProvider {
    fn scan_repos(&self, _dirs: &[(PathBuf, u16)]) -> Vec<Repo> {
        self.repos
            .iter()
            .map(|r| Repo {
                worktrees: vec![],
                ..r.clone()
            })
            .collect()
    }

    fn discover_repos(&self, _dirs: &[(PathBuf, u16)]) -> Vec<Repo> {
        self.repos.clone()
    }

    fn list_branches(&self, _repo_path: &Path) -> Vec<String> {
        self.branches.clone()
    }

    fn list_remote_branches(&self, _repo_path: &Path) -> Vec<String> {
        self.remote_branches.clone()
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

    fn remove_worktree(&self, _worktree_path: &Path) -> Result<()> {
        self.remove_worktree_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn prune_worktrees(&self, repo_path: &Path) -> Result<()> {
        self.prune_worktrees_calls
            .lock()
            .unwrap()
            .push(repo_path.to_path_buf());
        self.prune_worktrees_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn create_tracking_branch_and_worktree(
        &self,
        _repo_path: &Path,
        _branch: &str,
        _worktree_path: &Path,
    ) -> Result<()> {
        self.create_branch_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn list_remote_branches_for_remote(&self, _repo_path: &Path, _remote: &str) -> Vec<String> {
        self.remote_branches.clone()
    }

    fn list_remotes(&self, _repo_path: &Path) -> Vec<String> {
        self.remotes.clone()
    }

    fn fetch_remote(&self, repo_path: &Path, remote: &str) -> Result<()> {
        self.fetch_remote_calls
            .lock()
            .unwrap()
            .push((repo_path.to_path_buf(), remote.to_string()));
        self.fetch_remote_results
            .lock()
            .unwrap()
            .remove(&(repo_path.to_path_buf(), remote.to_string()))
            .unwrap_or(Ok(()))
    }

    fn default_branch(&self, _repo_path: &Path, _local_branches: &[String]) -> Option<String> {
        self.default_branch.clone()
    }

    fn resolve_repo_from_cwd(&self) -> Option<PathBuf> {
        self.current_repo_path.clone()
    }
}
