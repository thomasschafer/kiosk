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
///
/// When multiple repos share the same name (e.g. `api` under `~/work/api` and
/// `~/personal/api`), their session names are disambiguated using the shortest
/// unique trailing suffix of path components above each repo, anchored at the
/// deepest matching search root.
///
/// The algorithm groups colliding repos and incrementally grows the suffix
/// (one path component at a time) until every member of the group has a unique
/// disambiguator.  This correctly handles the edge case where repos sit directly
/// under different search roots that happen to share the same leaf name
/// (e.g. `~/alice/projects/api` and `~/bob/projects/api` under search roots
/// `~/alice/projects` and `~/bob/projects` both named "projects") — the suffix
/// is extended to include the parent directories above the search root until
/// unique labels can be formed.
pub fn apply_repo_name_collision_resolution(repos: &mut [Repo], search_dirs: &[(PathBuf, u16)]) {
    // Group repo indices by name.
    let mut by_name: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, repo) in repos.iter().enumerate() {
        by_name.entry(repo.name.clone()).or_default().push(i);
    }

    for (name, indices) in by_name {
        if indices.len() <= 1 {
            continue;
        }

        // For each repo in the collision group, build an ordered list of path
        // components that can be used for disambiguation.  We anchor at the
        // deepest matching search root and walk *upward* (toward the fs root)
        // so that more context is added with each growth step.
        let component_chains: Vec<Vec<String>> = indices
            .iter()
            .map(|&i| disambiguator_components(&repos[i].path, search_dirs))
            .collect();

        // Find the minimum trailing-suffix length that yields unique labels
        // for all repos in this group.
        let max_len = component_chains
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(1);

        let mut chosen_labels: Option<Vec<String>> = None;
        for suffix_len in 1..=max_len {
            let labels: Vec<String> = component_chains
                .iter()
                .map(|comps| {
                    let start = comps.len().saturating_sub(suffix_len);
                    comps[start..].join("/")
                })
                .collect();

            let unique_count = labels
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len();
            if unique_count == labels.len() {
                chosen_labels = Some(labels);
                break;
            }
        }

        // Apply: if we found unique labels use them; otherwise fall back to
        // the full absolute path parent (handles identical or symlinked paths).
        if let Some(labels) = chosen_labels {
            for (&repo_idx, label) in indices.iter().zip(labels.iter()) {
                repos[repo_idx].session_name = format!("{name}--({label})");
            }
        } else {
            for &repo_idx in &indices {
                let label = repos[repo_idx]
                    .path
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                repos[repo_idx].session_name = format!("{name}--({label})");
            }
        }
    }
}

/// Build the list of path components above `repo_path` that can be used for
/// disambiguation, ordered from most-specific (closest to repo) to least
/// (toward the filesystem root).
///
/// Anchoring: we strip the deepest matching search root first.  If the repo
/// sits directly under the root (no intermediate directories), we continue
/// upward into the search root's own components so that repos under differently-
/// named roots (or differently-named parent dirs) can still be told apart.
fn disambiguator_components(repo_path: &Path, search_dirs: &[(PathBuf, u16)]) -> Vec<String> {
    let best_root = search_dirs
        .iter()
        .filter(|(dir, _)| repo_path.starts_with(dir))
        .max_by_key(|(dir, _)| dir.components().count())
        .map(|(dir, _)| dir.as_path());

    // Components between the search root and the repo dir.
    let relative_parent: Vec<String> = match best_root {
        Some(root) => repo_path
            .strip_prefix(root)
            .unwrap_or(repo_path)
            .parent()
            .unwrap_or(Path::new(""))
            .components()
            .filter_map(normal_component)
            .collect(),
        None => repo_path
            .parent()
            .unwrap_or(Path::new(""))
            .components()
            .filter_map(normal_component)
            .collect(),
    };

    if relative_parent.is_empty() {
        // Repo is directly under the search root.  Walk into the root's own
        // components so we can disambiguate repos under same-named roots.
        let root_components: Vec<String> = best_root
            .unwrap_or(Path::new(""))
            .components()
            .filter_map(normal_component)
            .collect();
        root_components
    } else {
        relative_parent
    }
}

fn normal_component(c: std::path::Component<'_>) -> Option<String> {
    match c {
        std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
        _ => None,
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

    fn make_repo(name: &str, path: &str) -> Repo {
        Repo {
            name: name.to_string(),
            session_name: name.to_string(),
            path: PathBuf::from(path),
            worktrees: vec![],
        }
    }

    // ── collision resolution ─────────────────────────────────────────────────

    #[test]
    fn collision_resolution_basic_different_roots() {
        // Two repos named "api" under different search roots.
        let mut repos = vec![
            make_repo("api", "/tmp/work/api"),
            make_repo("api", "/tmp/personal/api"),
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
    fn collision_resolution_no_collision_leaves_names_unchanged() {
        let mut repos = vec![
            make_repo("api", "/tmp/work/api"),
            make_repo("frontend", "/tmp/work/frontend"),
        ];
        let search_dirs = vec![(PathBuf::from("/tmp/work"), 2)];
        apply_repo_name_collision_resolution(&mut repos, &search_dirs);
        assert_eq!(repos[0].session_name, "api");
        assert_eq!(repos[1].session_name, "frontend");
    }

    #[test]
    fn collision_resolution_three_way_collision() {
        // Three repos under the same root.  The immediate parent dirs are
        // "alpha", "beta", "personal" — all unique at suffix_len=1.
        let mut repos = vec![
            make_repo("app", "/home/user/clients/alpha/app"),
            make_repo("app", "/home/user/clients/beta/app"),
            make_repo("app", "/home/user/personal/app"),
        ];
        let search_dirs = vec![(PathBuf::from("/home/user"), 2)];
        apply_repo_name_collision_resolution(&mut repos, &search_dirs);
        assert_eq!(repos[0].session_name, "app--(alpha)");
        assert_eq!(repos[1].session_name, "app--(beta)");
        assert_eq!(repos[2].session_name, "app--(personal)");
    }

    #[test]
    fn collision_resolution_nested_search_dir_picks_deepest() {
        // The repo is under a more specific search dir — use that one.
        let mut repos = vec![
            make_repo("api", "/home/user/projects/work/api"),
            make_repo("api", "/home/user/projects/personal/api"),
        ];
        let search_dirs = vec![
            (PathBuf::from("/home/user"), 2),
            (PathBuf::from("/home/user/projects"), 2),
        ];
        apply_repo_name_collision_resolution(&mut repos, &search_dirs);
        // Deepest match is /home/user/projects; relative parent is work / personal.
        assert_eq!(repos[0].session_name, "api--(work)");
        assert_eq!(repos[1].session_name, "api--(personal)");
    }

    #[test]
    fn collision_resolution_shared_search_root_leaf_still_disambiguates() {
        // Both repos sit directly under different search roots that share the
        // same leaf name ("projects").  The old algorithm would produce
        // "api--(projects)" for both — a collision.
        // The new algorithm walks into the root's own components and grows the
        // suffix until unique: suffix_len=1 → both "projects" (collision);
        // suffix_len=2 → "alice/projects" vs "bob/projects" (unique).
        let mut repos = vec![
            make_repo("api", "/home/alice/projects/api"),
            make_repo("api", "/home/bob/projects/api"),
        ];
        let search_dirs = vec![
            (PathBuf::from("/home/alice/projects"), 2),
            (PathBuf::from("/home/bob/projects"), 2),
        ];
        apply_repo_name_collision_resolution(&mut repos, &search_dirs);
        assert_eq!(repos[0].session_name, "api--(alice/projects)");
        assert_eq!(repos[1].session_name, "api--(bob/projects)");
    }

    #[test]
    fn collision_resolution_repo_directly_under_root_uses_root_leaf() {
        // Repo is directly inside the search root: relative parent is empty,
        // so we fall back to the search root's leaf name.
        let mut repos = vec![
            make_repo("api", "/work/api"),
            make_repo("api", "/personal/api"),
        ];
        let search_dirs = vec![
            (PathBuf::from("/work"), 2),
            (PathBuf::from("/personal"), 2),
        ];
        apply_repo_name_collision_resolution(&mut repos, &search_dirs);
        assert_eq!(repos[0].session_name, "api--(work)");
        assert_eq!(repos[1].session_name, "api--(personal)");
    }

    #[test]
    fn collision_resolution_mixed_colliding_and_unique() {
        // Only the "api" repos collide; "frontend" is unique and must not change.
        let mut repos = vec![
            make_repo("api", "/tmp/work/api"),
            make_repo("api", "/tmp/personal/api"),
            make_repo("frontend", "/tmp/work/frontend"),
        ];
        let search_dirs = vec![
            (PathBuf::from("/tmp/work"), 2),
            (PathBuf::from("/tmp/personal"), 2),
        ];
        apply_repo_name_collision_resolution(&mut repos, &search_dirs);
        assert_eq!(repos[0].session_name, "api--(work)");
        assert_eq!(repos[1].session_name, "api--(personal)");
        assert_eq!(repos[2].session_name, "frontend");
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
