use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

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
}

/// Discover git repos one level deep inside each search dir
pub fn discover_repos(search_dirs: &[PathBuf]) -> Vec<Repo> {
    let mut repos = Vec::new();

    for dir in search_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.join(".git").exists() {
                if let Some(repo) = build_repo(&path) {
                    repos.push(repo);
                }
            }
        }
    }

    repos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    repos
}

fn build_repo(path: &Path) -> Option<Repo> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let worktrees = list_worktrees(path);
    Some(Repo {
        name,
        path: path.to_path_buf(),
        worktrees,
    })
}

/// Parse `git worktree list --porcelain` output into worktrees
pub fn parse_worktree_porcelain(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_first = true;

    for line in output.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(b.to_string());
        } else if line.is_empty() {
            if let Some(path) = current_path.take() {
                worktrees.push(Worktree {
                    path,
                    branch: current_branch.take(),
                    is_main: is_first,
                });
                is_first = false;
            }
            current_branch = None;
        }
    }

    // Handle last entry (no trailing blank line)
    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch,
            is_main: is_first,
        });
    }

    worktrees
}

fn list_worktrees(repo_path: &Path) -> Vec<Worktree> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output();

    let Ok(output) = output else {
        return vec![main_worktree(repo_path)];
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let worktrees = parse_worktree_porcelain(&stdout);

    if worktrees.is_empty() {
        vec![main_worktree(repo_path)]
    } else {
        worktrees
    }
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

/// List local branches for a repo
pub fn list_branches(repo_path: &Path) -> Vec<String> {
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

/// Add a new worktree for an existing branch
pub fn add_worktree(repo_path: &Path, branch: &str, worktree_path: &Path) -> Result<()> {
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

/// Create a new branch from a base and set up a worktree for it
pub fn create_branch_and_worktree(
    repo_path: &Path,
    new_branch: &str,
    base_branch: &str,
    worktree_path: &Path,
) -> Result<()> {
    let output = Command::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            new_branch,
            &worktree_path.to_string_lossy(),
            base_branch,
        ])
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add -b failed: {stderr}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

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
        // Need an initial commit for worktrees to work
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
    fn test_parse_worktree_porcelain_single() {
        let output = "worktree /home/user/project\nHEAD abc123\nbranch refs/heads/main\n\n";
        let wts = parse_worktree_porcelain(output);
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].path, PathBuf::from("/home/user/project"));
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert!(wts[0].is_main);
    }

    #[test]
    fn test_parse_worktree_porcelain_multiple() {
        let output = "\
worktree /home/user/project
HEAD abc123
branch refs/heads/main

worktree /home/user/project-feat
HEAD def456
branch refs/heads/feat/thing

";
        let wts = parse_worktree_porcelain(output);
        assert_eq!(wts.len(), 2);
        assert!(wts[0].is_main);
        assert!(!wts[1].is_main);
        assert_eq!(wts[1].branch.as_deref(), Some("feat/thing"));
    }

    #[test]
    fn test_parse_worktree_porcelain_detached() {
        let output = "worktree /home/user/project\nHEAD abc123\ndetached\n\n";
        let wts = parse_worktree_porcelain(output);
        assert_eq!(wts.len(), 1);
        assert!(wts[0].branch.is_none());
    }

    #[test]
    fn test_parse_worktree_porcelain_no_trailing_newline() {
        let output = "worktree /home/user/project\nbranch refs/heads/main";
        let wts = parse_worktree_porcelain(output);
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn test_parse_worktree_porcelain_empty() {
        let wts = parse_worktree_porcelain("");
        assert!(wts.is_empty());
    }

    #[test]
    fn test_discover_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("my-repo");
        fs::create_dir_all(&repo_dir).unwrap();
        init_test_repo(&repo_dir);

        // Non-git dir should be skipped
        fs::create_dir_all(tmp.path().join("not-a-repo")).unwrap();

        let repos = discover_repos(&[tmp.path().to_path_buf()]);
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

        let repos = discover_repos(&[tmp.path().to_path_buf()]);
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

        let branches = list_branches(tmp.path());
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

        let wt_path = tmp.path().join("repo-feat-wt-test");
        add_worktree(&repo, "feat/wt-test", &wt_path).unwrap();

        assert!(wt_path.exists());
        assert!(wt_path.join("README.md").exists());

        let worktrees = list_worktrees(&repo);
        assert_eq!(worktrees.len(), 2);
    }

    #[test]
    fn test_create_branch_and_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_test_repo(&repo);

        let wt_path = tmp.path().join("repo-new-branch");
        create_branch_and_worktree(&repo, "new-branch", "master", &wt_path).unwrap();

        assert!(wt_path.exists());
        let branches = list_branches(&repo);
        assert!(branches.contains(&"new-branch".to_string()));
    }

    #[test]
    fn test_add_worktree_fails_for_nonexistent_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_test_repo(tmp.path());

        let wt_path = tmp.path().join("wt-nope");
        let result = add_worktree(tmp.path(), "nonexistent-branch", &wt_path);
        assert!(result.is_err());
    }
}
