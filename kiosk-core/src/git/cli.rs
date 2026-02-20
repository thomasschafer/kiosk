use super::{
    parse_worktree_porcelain,
    provider::GitProvider,
    repo::{Repo, Worktree},
};
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub struct CliGitProvider;

impl GitProvider for CliGitProvider {
    fn discover_repos(&self, dirs: &[PathBuf]) -> Vec<Repo> {
        let mut repos = Vec::new();

        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                if path.join(".git").exists() {
                    if let Some(repo) = self.build_repo(&path) {
                        repos.push(repo);
                    }
                }
            }
        }

        repos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        repos
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

    fn list_worktrees(&self, repo_path: &Path) -> Vec<Worktree> {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(repo_path)
            .output();

        let Ok(output) = output else {
            return vec![self.main_worktree(repo_path)];
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let worktrees = parse_worktree_porcelain(&stdout);

        if worktrees.is_empty() {
            vec![self.main_worktree(repo_path)]
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
}

impl CliGitProvider {
    fn build_repo(&self, path: &Path) -> Option<Repo> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let worktrees = self.list_worktrees(path);
        Some(Repo {
            name,
            path: path.to_path_buf(),
            worktrees,
        })
    }

    fn main_worktree(&self, repo_path: &Path) -> Worktree {
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

#[cfg(test)]
mod tests {
    use super::*;
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
        let repos = provider.discover_repos(&[tmp.path().to_path_buf()]);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "my-repo");
        assert_eq!(repos[0].worktrees.len(), 1);
        assert_eq!(repos[0].worktrees[0].branch.as_deref(), Some("master"));
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
        let repos = provider.discover_repos(&[tmp.path().to_path_buf()]);
        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Middle", "zebra"]);
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
}
