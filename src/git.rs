use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
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

fn list_worktrees(repo_path: &Path) -> Vec<Worktree> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output();

    let Ok(output) = output else {
        return vec![main_worktree(repo_path)];
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_first = true;

    for line in stdout.lines() {
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

/// Add a new worktree
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
