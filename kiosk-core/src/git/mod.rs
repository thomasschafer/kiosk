pub mod cli;
pub mod mock;
pub mod provider;
pub mod repo;

pub use cli::CliGitProvider;
pub use provider::GitProvider;
pub use repo::{Repo, Worktree};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// Parse `git worktree list --porcelain` output into worktrees
pub fn parse_worktree_porcelain(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<std::path::PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_first = true;

    for line in output.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current_path = Some(std::path::PathBuf::from(p));
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

/// Build the tmux session name for a repo/worktree pair.
pub fn tmux_session_name_for_worktree(
    repo_name: &str,
    session_name: &str,
    repo_path: &Path,
    worktree_path: &Path,
) -> String {
    if worktree_path == repo_path {
        session_name.replace('.', "_")
    } else {
        worktree_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            // Carry session disambiguation through branch worktree names.
            .replacen(repo_name, session_name, 1)
            .replace('.', "_")
    }
}

/// Apply deterministic session-name disambiguation for repos with name collisions.
pub fn apply_repo_name_collision_resolution(repos: &mut [Repo], search_dirs: &[(PathBuf, u16)]) {
    let mut name_counts = std::collections::HashMap::<String, usize>::new();
    for repo in repos.iter() {
        *name_counts.entry(repo.name.clone()).or_insert(0) += 1;
    }
    if !name_counts.values().any(|&count| count > 1) {
        return;
    }

    for repo in repos {
        if name_counts[&repo.name] > 1 {
            let search_dir_name = search_dirs
                .iter()
                .filter(|(dir, _)| repo.path.starts_with(dir))
                .max_by_key(|(dir, _)| dir.components().count())
                .and_then(|(dir, _)| dir.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            repo.session_name = format!("{}--({search_dir_name})", repo.name);
        }
    }
}

/// Normalize a tmux session base name for matching.
pub fn normalize_session_base(session_name: &str) -> String {
    session_name.replace('.', "_")
}

/// Return true when an active tmux session belongs to the repo's session namespace.
pub fn repo_matches_active_session<S: std::hash::BuildHasher>(
    repo: &Repo,
    active_sessions: &HashSet<String, S>,
) -> bool {
    let base = normalize_session_base(&repo.session_name);
    let prefix = format!("{base}--");
    active_sessions
        .iter()
        .any(|session| session == &base || session.starts_with(&prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashSet, path::PathBuf};

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
    fn test_apply_repo_name_collision_resolution_disambiguates() {
        let mut repos = vec![
            Repo {
                name: "api".to_string(),
                session_name: "api".to_string(),
                path: PathBuf::from("/tmp/work/api"),
                worktrees: vec![],
            },
            Repo {
                name: "api".to_string(),
                session_name: "api".to_string(),
                path: PathBuf::from("/tmp/personal/api"),
                worktrees: vec![],
            },
        ];
        let search_dirs = vec![
            (PathBuf::from("/tmp/work"), 2),
            (PathBuf::from("/tmp/personal"), 2),
        ];

        apply_repo_name_collision_resolution(&mut repos, &search_dirs);

        assert_eq!(repos[0].session_name, "api--(work)");
        assert_eq!(repos[1].session_name, "api--(personal)");
    }

    #[test]
    fn test_repo_matches_active_session_main_and_branch() {
        let repo = Repo {
            name: "my.repo".to_string(),
            session_name: "my.repo".to_string(),
            path: PathBuf::from("/tmp/my.repo"),
            worktrees: vec![],
        };
        let active: HashSet<String> = ["my_repo".to_string(), "my_repo--feat".to_string()]
            .into_iter()
            .collect();
        assert!(repo_matches_active_session(&repo, &active));
    }
}
