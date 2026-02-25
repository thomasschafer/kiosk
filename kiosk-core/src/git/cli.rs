use super::{
    parse_worktree_porcelain,
    provider::GitProvider,
    repo::{Repo, Worktree},
};
use crate::constants::GIT_DIR_ENTRY;
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub struct CliGitProvider;

impl GitProvider for CliGitProvider {
    fn scan_repos(&self, dirs: &[(PathBuf, u16)]) -> Vec<Repo> {
        let mut repos_with_dirs = Vec::new();

        for (dir, depth) in dirs {
            self.scan_dir_recursive(dir, dir, *depth, &mut repos_with_dirs, false);
        }

        Self::apply_collision_resolution(repos_with_dirs)
    }

    fn scan_repos_streaming(&self, dir: &Path, depth: u16, on_found: &dyn Fn(Vec<Repo>)) {
        // Use the per-repo streaming scanner, batching into single-element vecs
        Self::scan_dir_streaming(dir, depth, &|repo| {
            on_found(vec![repo]);
        });
    }

    fn discover_repos(&self, dirs: &[(PathBuf, u16)]) -> Vec<Repo> {
        let mut repos_with_dirs = Vec::new();

        for (dir, depth) in dirs {
            self.scan_dir_recursive(dir, dir, *depth, &mut repos_with_dirs, true);
        }

        Self::apply_collision_resolution(repos_with_dirs)
    }

    fn list_branches(&self, repo_path: &Path) -> Vec<String> {
        let output = Command::new("git")
            .args(["branch", "--format=%(refname:short)"])
            .current_dir(repo_path)
            .output();

        let Ok(output) = output else {
            return Vec::new();
        };

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect()
    }

    fn list_remote_branches(&self, repo_path: &Path) -> Vec<String> {
        let output = Command::new("git")
            .args(["branch", "-r", "--format=%(refname:short)"])
            .current_dir(repo_path)
            .output();

        let Ok(output) = output else {
            return Vec::new();
        };

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                // Skip HEAD pointer (e.g. "origin/HEAD -> origin/main")
                if line.contains("->") {
                    return None;
                }
                // Strip the remote prefix (e.g. "origin/feature" -> "feature")
                line.split_once('/').map(|(_, branch)| branch.to_string())
            })
            .collect()
    }

    fn list_worktrees(&self, repo_path: &Path) -> Vec<Worktree> {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(repo_path)
            .output();

        let Ok(output) = output else {
            return vec![Self::main_worktree(repo_path)];
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let worktrees = parse_worktree_porcelain(&stdout);

        if worktrees.is_empty() {
            vec![Self::main_worktree(repo_path)]
        } else {
            worktrees
        }
    }

    fn add_worktree(&self, repo_path: &Path, branch: &str, worktree_path: &Path) -> Result<()> {
        let output = Command::new("git")
            .args(["worktree", "add", &worktree_path.to_string_lossy(), branch])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed: {stderr}");
        }

        Ok(())
    }

    fn create_branch_and_worktree(
        &self,
        repo_path: &Path,
        new_branch: &str,
        base: &str,
        worktree_path: &Path,
    ) -> Result<()> {
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                new_branch,
                &worktree_path.to_string_lossy(),
                base,
            ])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add -b failed: {stderr}");
        }

        Ok(())
    }

    fn remove_worktree(&self, worktree_path: &Path) -> Result<()> {
        let canonical =
            std::fs::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.to_path_buf());
        let output = Command::new("git")
            .env("LC_ALL", "C")
            .args(["worktree", "remove", &canonical.to_string_lossy()])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("is not a working tree") {
                if canonical.exists() {
                    std::fs::remove_dir_all(&canonical)?;
                }
                return Ok(());
            }
            anyhow::bail!("git worktree remove failed: {stderr}");
        }

        Ok(())
    }

    fn prune_worktrees(&self, repo_path: &Path) -> Result<()> {
        let output = Command::new("git")
            .args(["worktree", "prune", "--expire", "now"])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree prune failed: {stderr}");
        }

        Ok(())
    }

    fn create_tracking_branch_and_worktree(
        &self,
        repo_path: &Path,
        branch: &str,
        worktree_path: &Path,
    ) -> Result<()> {
        // git worktree add <path> -b <branch> --track origin/<branch>
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                "-b",
                branch,
                "--track",
                &format!("origin/{branch}"),
            ])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add (tracking) failed: {stderr}");
        }

        Ok(())
    }

    fn fetch_all(&self, repo_path: &Path) -> Result<()> {
        let output = Command::new("git")
            .args(["fetch", "--all"])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git fetch --all failed: {stderr}");
        }

        Ok(())
    }

    fn default_branch(&self, repo_path: &Path, local_branches: &[String]) -> Option<String> {
        // Try symbolic-ref first; fall through on spawn/IO errors so the
        // local-branch heuristic below still runs.
        if let Ok(output) = Command::new("git")
            .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
            .current_dir(repo_path)
            .output()
            && output.status.success()
        {
            let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(branch) = refname.strip_prefix("refs/remotes/origin/") {
                return Some(branch.to_string());
            }
        }

        // Fall back to checking local branches
        for candidate in &["main", "master"] {
            if local_branches.iter().any(|b| b == candidate) {
                return Some((*candidate).to_string());
            }
        }

        None
    }

    fn resolve_repo_from_cwd(&self) -> Option<PathBuf> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }

        None
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::constants::WORKTREE_NAME_SEPARATOR;
    use std::fs;

    fn init_test_repo(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
        let dummy = dir.join("README.md");
        fs::write(&dummy, "# test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn test_discover_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("my-repo");
        fs::create_dir_all(&repo_dir).unwrap();
        init_test_repo(&repo_dir);

        fs::create_dir_all(tmp.path().join("not-a-repo")).unwrap();

        let provider = CliGitProvider;
        let repos = provider.discover_repos(&[(tmp.path().to_path_buf(), 1)]);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "my-repo");
        assert_eq!(repos[0].session_name, "my-repo");
        assert_eq!(repos[0].worktrees.len(), 1);
        assert_eq!(repos[0].worktrees[0].branch.as_deref(), Some("master"));
    }

    #[test]
    fn test_scan_repos_returns_empty_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("my-repo");
        fs::create_dir_all(&repo_dir).unwrap();
        init_test_repo(&repo_dir);

        let provider = CliGitProvider;
        let repos = provider.scan_repos(&[(tmp.path().to_path_buf(), 1)]);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "my-repo");
        assert_eq!(repos[0].session_name, "my-repo");
        assert!(
            repos[0].worktrees.is_empty(),
            "scan_repos should return repos with empty worktrees"
        );
    }

    #[test]
    fn test_scan_repos_collision_detection() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();

        let repo1 = tmp1.path().join("myrepo");
        let repo2 = tmp2.path().join("myrepo");
        fs::create_dir_all(&repo1).unwrap();
        fs::create_dir_all(&repo2).unwrap();
        init_test_repo(&repo1);
        init_test_repo(&repo2);

        let provider = CliGitProvider;
        let scanned = provider.scan_repos(&[
            (tmp1.path().to_path_buf(), 1),
            (tmp2.path().to_path_buf(), 1),
        ]);
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].name, "myrepo");
        assert_eq!(scanned[1].name, "myrepo");
        let session_names: std::collections::HashSet<String> =
            scanned.iter().map(|r| r.session_name.clone()).collect();
        assert_eq!(session_names.len(), 2);
    }

    #[test]
    fn test_discover_repos_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["zebra", "alpha", "Middle"] {
            let d = tmp.path().join(name);
            fs::create_dir_all(&d).unwrap();
            init_test_repo(&d);
        }

        let provider = CliGitProvider;
        let repos = provider.discover_repos(&[(tmp.path().to_path_buf(), 1)]);
        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Middle", "zebra"]);
        // All should have unique names, so session_names should match names
        for repo in &repos {
            assert_eq!(repo.session_name, repo.name);
        }
    }

    #[test]
    fn test_discover_repos_collision_detection() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();

        // Create repos with same name in different directories
        let repo1 = tmp1.path().join("myrepo");
        let repo2 = tmp2.path().join("myrepo");
        fs::create_dir_all(&repo1).unwrap();
        fs::create_dir_all(&repo2).unwrap();
        init_test_repo(&repo1);
        init_test_repo(&repo2);

        let provider = CliGitProvider;
        let discovered = provider.discover_repos(&[
            (tmp1.path().to_path_buf(), 1),
            (tmp2.path().to_path_buf(), 1),
        ]);
        assert_eq!(discovered.len(), 2);

        // Both should have same name but different session names
        assert_eq!(discovered[0].name, "myrepo");
        assert_eq!(discovered[1].name, "myrepo");

        // Session names should be disambiguated with parent dir names
        let session_names: std::collections::HashSet<String> =
            discovered.iter().map(|r| r.session_name.clone()).collect();
        assert_eq!(session_names.len(), 2); // Both should be unique

        // Both should contain the repo name and parent dir somehow
        for repo in &discovered {
            assert!(repo.session_name.contains("myrepo"));
            assert!(repo.session_name.contains(WORKTREE_NAME_SEPARATOR));
        }
    }

    #[test]
    fn test_list_branches() {
        let tmp = tempfile::tempdir().unwrap();
        init_test_repo(tmp.path());

        Command::new("git")
            .args(["branch", "feat/test"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        let provider = CliGitProvider;
        let branches = provider.list_branches(tmp.path());
        assert!(branches.contains(&"master".to_string()));
        assert!(branches.contains(&"feat/test".to_string()));
    }

    #[test]
    fn test_add_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_test_repo(&repo);

        Command::new("git")
            .args(["branch", "feat/wt-test"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let provider = CliGitProvider;
        let wt_path = tmp.path().join("repo-feat-wt-test");
        provider
            .add_worktree(&repo, "feat/wt-test", &wt_path)
            .unwrap();

        assert!(wt_path.exists());
        assert!(wt_path.join("README.md").exists());

        let worktrees = provider.list_worktrees(&repo);
        assert_eq!(worktrees.len(), 2);
    }

    #[test]
    fn test_create_branch_and_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_test_repo(&repo);

        let provider = CliGitProvider;
        let wt_path = tmp.path().join("repo-new-branch");
        provider
            .create_branch_and_worktree(&repo, "new-branch", "master", &wt_path)
            .unwrap();

        assert!(wt_path.exists());
        let branches = provider.list_branches(&repo);
        assert!(branches.contains(&"new-branch".to_string()));
    }

    #[test]
    fn test_add_worktree_fails_for_nonexistent_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_test_repo(tmp.path());

        let provider = CliGitProvider;
        let wt_path = tmp.path().join("wt-nope");
        let result = provider.add_worktree(tmp.path(), "nonexistent-branch", &wt_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_discover_repos_depth_1_skips_nested() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a repo nested two levels deep
        let nested = tmp.path().join("org").join("my-repo");
        fs::create_dir_all(&nested).unwrap();
        init_test_repo(&nested);

        let provider = CliGitProvider;
        // Depth 1 should NOT find it (it's 2 levels deep)
        let repos = provider.discover_repos(&[(tmp.path().to_path_buf(), 1)]);
        assert_eq!(repos.len(), 0);
    }

    #[test]
    fn test_discover_repos_depth_2_finds_nested() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a repo nested two levels deep
        let nested = tmp.path().join("org").join("my-repo");
        fs::create_dir_all(&nested).unwrap();
        init_test_repo(&nested);

        let provider = CliGitProvider;
        // Depth 2 should find it
        let repos = provider.discover_repos(&[(tmp.path().to_path_buf(), 2)]);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "my-repo");
    }

    #[test]
    fn test_discover_repos_depth_does_not_recurse_into_repos() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a repo at depth 1
        let repo_dir = tmp.path().join("parent-repo");
        fs::create_dir_all(&repo_dir).unwrap();
        init_test_repo(&repo_dir);

        // Create a nested repo inside it (submodule-like)
        let nested = repo_dir.join("sub-repo");
        fs::create_dir_all(&nested).unwrap();
        init_test_repo(&nested);

        let provider = CliGitProvider;
        // Should find the parent but not recurse into it (it has .git)
        let repos = provider.discover_repos(&[(tmp.path().to_path_buf(), 3)]);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "parent-repo");
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_fetch_all() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a "remote" bare repo
        let remote_dir = tmp.path().join("remote.git");
        fs::create_dir_all(&remote_dir).unwrap();
        run_git(&remote_dir, &["init", "--bare"]);

        // Create a local repo and add the bare repo as a remote
        let local_dir = tmp.path().join("local");
        fs::create_dir_all(&local_dir).unwrap();
        init_test_repo(&local_dir);
        run_git(
            &local_dir,
            &["remote", "add", "origin", &remote_dir.to_string_lossy()],
        );
        run_git(&local_dir, &["push", "origin", "master"]);

        // Create a second clone to push a new branch to the remote
        let clone_dir = tmp.path().join("clone");
        run_git(
            tmp.path(),
            &["clone", &remote_dir.to_string_lossy(), "clone"],
        );
        run_git(&clone_dir, &["config", "user.email", "test@test.com"]);
        run_git(&clone_dir, &["config", "user.name", "Test"]);
        run_git(&clone_dir, &["checkout", "-b", "new-feature"]);
        fs::write(clone_dir.join("feature.txt"), "feature").unwrap();
        run_git(&clone_dir, &["add", "."]);
        run_git(&clone_dir, &["commit", "-m", "feature"]);
        run_git(&clone_dir, &["push", "origin", "new-feature"]);

        let provider = CliGitProvider;

        // Before fetch, the local repo shouldn't see the new remote branch
        let before = provider.list_remote_branches(&local_dir);
        assert!(
            !before.contains(&"new-feature".to_string()),
            "Should not see new-feature before fetch: {before:?}"
        );

        // Fetch and verify the branch appears
        provider.fetch_all(&local_dir).unwrap();
        let after = provider.list_remote_branches(&local_dir);
        assert!(
            after.contains(&"new-feature".to_string()),
            "Should see new-feature after fetch: {after:?}"
        );
    }
}

impl CliGitProvider {
    /// Streaming scan: emits each repo via callback as it's found.
    fn scan_dir_streaming(dir: &Path, depth: u16, on_found: &dyn Fn(Repo)) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("Warning: Failed to read directory {}: {err}", dir.display());
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            if path.join(GIT_DIR_ENTRY).exists() {
                let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                if let Some(repo) = Self::build_repo_stub(&canonical) {
                    on_found(repo);
                }
            } else if depth > 1 {
                Self::scan_dir_streaming(&path, depth - 1, on_found);
            }
        }
    }

    fn scan_dir_recursive<'a>(
        &self,
        dir: &Path,
        search_root: &'a Path,
        depth: u16,
        repos: &mut Vec<(Repo, &'a Path)>,
        with_worktrees: bool,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("Warning: Failed to read directory {}: {err}", dir.display());
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // If this directory is a git repo, add it
            if path.join(GIT_DIR_ENTRY).exists() {
                let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                let repo = if with_worktrees {
                    self.build_repo(&canonical)
                } else {
                    Self::build_repo_stub(&canonical)
                };
                if let Some(repo) = repo {
                    repos.push((repo, search_root));
                }
            } else if depth > 1 {
                // Recurse into subdirectories if we have remaining depth
                self.scan_dir_recursive(&path, search_root, depth - 1, repos, with_worktrees);
            }
        }
    }

    /// Sorts repos by name and disambiguates session names for collisions.
    fn apply_collision_resolution(mut repos_with_dirs: Vec<(Repo, &Path)>) -> Vec<Repo> {
        repos_with_dirs.sort_by(|a, b| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()));

        let mut name_counts = std::collections::HashMap::<String, usize>::new();
        for (repo, _) in &repos_with_dirs {
            *name_counts.entry(repo.name.clone()).or_insert(0) += 1;
        }

        let mut repos = Vec::new();
        for (mut repo, search_dir) in repos_with_dirs {
            if name_counts[&repo.name] > 1 {
                let parent_dir_name = search_dir.file_name().unwrap_or_default().to_string_lossy();
                repo.session_name = format!("{}--({parent_dir_name})", repo.name);
            } else {
                repo.session_name.clone_from(&repo.name);
            }
            repos.push(repo);
        }

        repos
    }

    /// Create a repo stub from a path — no git calls, empty worktrees.
    fn build_repo_stub(path: &Path) -> Option<Repo> {
        let name = path.file_name()?.to_string_lossy().to_string();
        Some(Repo {
            session_name: name.clone(),
            name,
            path: path.to_path_buf(),
            worktrees: vec![],
        })
    }

    fn build_repo(&self, path: &Path) -> Option<Repo> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let worktrees = self.list_worktrees(path);
        Some(Repo {
            session_name: name.clone(),
            name,
            path: path.to_path_buf(),
            worktrees,
        })
    }

    fn main_worktree(repo_path: &Path) -> Worktree {
        let branch = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(repo_path)
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            });

        Worktree {
            path: repo_path.to_path_buf(),
            branch,
            is_main: true,
        }
    }
}
