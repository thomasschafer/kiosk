use kiosk_core::{
    agent,
    event::{AppEvent, SessionRuntimeUpdate},
    git::GitProvider,
    state::BranchEntry,
};
use rayon::ThreadPoolBuilder;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use kiosk_core::git::Repo;
use kiosk_core::tmux::TmuxProvider;

use super::EventSender;

/// Maximum number of concurrent `git worktree list` enrichment calls.
const ENRICHMENT_POOL_SIZE: usize = 8;

/// Maximum number of concurrent per-remote `git fetch` calls.
const FETCH_POOL_SIZE: usize = 4;

pub(super) fn spawn_repo_discovery<T: TmuxProvider + ?Sized + 'static>(
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
    search_dirs: Vec<(PathBuf, u16)>,
) {
    let git = Arc::clone(git);
    let tmux = Arc::clone(tmux);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }

        // Kick off session activity fetch immediately — it'll send its own event
        // as soon as tmux responds, independent of scan/enrichment progress.
        {
            let tmux = Arc::clone(&tmux);
            let sender = sender.clone();
            thread::spawn(move || {
                let sessions = tmux.list_sessions_with_activity();
                let session_activity: HashMap<String, u64> = sessions.into_iter().collect();
                sender.send(AppEvent::SessionActivityLoaded { session_activity });
            });
        }

        // Bounded pool for worktree enrichment — prevents thread explosion
        // with hundreds of repos.
        let enrich_pool = match ThreadPoolBuilder::new()
            .num_threads(ENRICHMENT_POOL_SIZE)
            .build()
        {
            Ok(pool) => Arc::new(pool),
            Err(e) => {
                eprintln!("Warning: failed to build enrichment pool: {e}");
                sender.send(AppEvent::ScanComplete { search_dirs });
                return;
            }
        };

        // Phase 1: Stream repos as they're found.
        // Each repo also kicks off enrichment on the pool immediately.
        let scan_callback = |repo: Repo,
                             git: &Arc<dyn GitProvider>,
                             sender: &EventSender,
                             pool: &rayon::ThreadPool| {
            // Send discovery event first so the repo exists in state
            // before any enrichment event can arrive on the channel.
            let path = repo.path.clone();
            sender.send(AppEvent::ReposFound { repo });

            let git = Arc::clone(git);
            let sender = sender.clone();
            pool.spawn(move || {
                let worktrees = git.list_worktrees(&path);
                sender.send(AppEvent::RepoEnriched {
                    repo_path: path,
                    worktrees,
                });
            });
        };

        if search_dirs.len() == 1 {
            let (dir, depth) = &search_dirs[0];
            let git_ref = &git;
            let sender_ref = &sender;
            let pool_ref = &enrich_pool;
            git.scan_repos_streaming(dir, *depth, &|repo| {
                if !sender_ref.cancel.load(Ordering::Relaxed) {
                    scan_callback(repo, git_ref, sender_ref, pool_ref);
                }
            });
        } else {
            // Multiple dirs: scan each in a parallel thread
            thread::scope(|s| {
                for (dir, depth) in &search_dirs {
                    let git = &git;
                    let sender = &sender;
                    let pool = &enrich_pool;
                    s.spawn(move || {
                        if sender.cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        git.scan_repos_streaming(dir, *depth, &|repo| {
                            if !sender.cancel.load(Ordering::Relaxed) {
                                scan_callback(repo, git, sender, pool);
                            }
                        });
                    });
                }
            });
        }

        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }

        // Signal scan complete so the UI can run collision resolution
        sender.send(AppEvent::ScanComplete { search_dirs });
    });
}

pub(super) fn spawn_worktree_creation(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch: String,
    wt_path: PathBuf,
    session_name: String,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        match git.add_worktree(&repo_path, &branch, &wt_path) {
            Ok(()) => sender.send(AppEvent::WorktreeCreated {
                path: wt_path,
                session_name,
            }),
            Err(e) => sender.send(AppEvent::GitError(format!("{e}"))),
        }
    });
}

pub(super) fn spawn_worktree_removal(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    worktree_path: PathBuf,
    branch_name: String,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        match git.remove_worktree(&worktree_path) {
            Ok(()) => sender.send(AppEvent::WorktreeRemoved {
                branch_name,
                worktree_path,
            }),
            Err(e) => sender.send(AppEvent::WorktreeRemoveFailed {
                branch_name,
                worktree_path,
                error: format!("{e}"),
            }),
        }
    });
}

pub(super) fn spawn_branch_and_worktree_creation(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    new_branch: String,
    base: String,
    wt_path: PathBuf,
    session_name: String,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        match git.create_branch_and_worktree(&repo_path, &new_branch, &base, &wt_path) {
            Ok(()) => sender.send(AppEvent::WorktreeCreated {
                path: wt_path,
                session_name,
            }),
            Err(e) => sender.send(AppEvent::GitError(format!("{e}"))),
        }
    });
}

pub(super) fn spawn_branch_loading<T: TmuxProvider + ?Sized + 'static>(
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
    mut repo: Repo,
    cwd: Option<PathBuf>,
) {
    let git = Arc::clone(git);
    let tmux = Arc::clone(tmux);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        let sessions_with_activity = tmux.list_sessions_with_activity();
        let active_sessions: Vec<String> = sessions_with_activity
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        let session_activity: HashMap<String, u64> = sessions_with_activity.into_iter().collect();
        repo.worktrees = git.list_worktrees(&repo.path);
        let local_names = git.list_branches(&repo.path);
        let default_branch = git.default_branch(&repo.path, &local_names);
        let branches = BranchEntry::build_sorted_with_activity(
            &repo,
            &local_names,
            &active_sessions,
            default_branch.as_deref(),
            &session_activity,
            cwd.as_deref(),
        );
        sender.send(AppEvent::BranchesLoaded {
            branches,
            worktrees: repo.worktrees,
            local_names,
            session_activity,
        });
    });
}

pub(super) fn spawn_remote_branch_loading(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    local_names: Vec<String>,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        let remotes = git.list_remotes(&repo_path);
        let mut branches = Vec::new();
        for remote in &remotes {
            let remote_names = git.list_remote_branches_for_remote(&repo_path, remote);
            branches.extend(BranchEntry::build_remote(
                remote,
                &remote_names,
                &local_names,
            ));
        }
        if !branches.is_empty() {
            sender.send(AppEvent::RemoteBranchesLoaded { branches });
        }
    });
}

pub(super) fn spawn_git_fetch(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        let remotes = git.list_remotes(&repo_path);
        if remotes.is_empty() {
            sender.send(AppEvent::GitFetchCompleted {
                branches: vec![],
                repo_path,
                is_final: true,
            });
            return;
        }

        let remaining = Arc::new(std::sync::atomic::AtomicUsize::new(remotes.len()));
        let local_names = Arc::new(git.list_branches(&repo_path));

        let pool = match ThreadPoolBuilder::new()
            .num_threads(FETCH_POOL_SIZE)
            .build()
        {
            Ok(pool) => pool,
            Err(e) => {
                log::warn!("failed to build fetch thread pool: {e}");
                sender.send(AppEvent::GitFetchCompleted {
                    branches: vec![],
                    repo_path,
                    is_final: true,
                });
                return;
            }
        };

        for remote in remotes {
            let git = Arc::clone(&git);
            let sender = sender.clone();
            let repo_path = repo_path.clone();
            let remaining = Arc::clone(&remaining);
            let local_names = Arc::clone(&local_names);
            pool.spawn(move || {
                if sender.cancel.load(Ordering::Relaxed) {
                    let old = remaining.fetch_sub(1, Ordering::AcqRel);
                    sender.send(AppEvent::GitFetchCompleted {
                        branches: vec![],
                        repo_path,
                        is_final: old == 1,
                    });
                    return;
                }
                let branches = match git.fetch_remote(&repo_path, &remote) {
                    Ok(()) => {
                        if sender.cancel.load(Ordering::Relaxed) {
                            let old = remaining.fetch_sub(1, Ordering::AcqRel);
                            sender.send(AppEvent::GitFetchCompleted {
                                branches: vec![],
                                repo_path,
                                is_final: old == 1,
                            });
                            return;
                        }
                        let remote_names = git.list_remote_branches_for_remote(&repo_path, &remote);
                        BranchEntry::build_remote(&remote, &remote_names, &local_names)
                    }
                    Err(e) => {
                        log::warn!("git fetch failed for remote {remote}: {e}");
                        vec![]
                    }
                };
                let old = remaining.fetch_sub(1, Ordering::AcqRel);
                sender.send(AppEvent::GitFetchCompleted {
                    branches,
                    repo_path,
                    is_final: old == 1,
                });
            });
        }
    });
}

pub(super) fn spawn_tracking_worktree_creation(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch: String,
    wt_path: PathBuf,
    session_name: String,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        match git.create_tracking_branch_and_worktree(&repo_path, &branch, &wt_path) {
            Ok(()) => sender.send(AppEvent::WorktreeCreated {
                path: wt_path,
                session_name,
            }),
            Err(e) => sender.send(AppEvent::GitError(format!("{e}"))),
        }
    });
}

/// Spawns a background thread that periodically detects agent states for
/// sessions relevant to the current branch view. Only polls sessions that
/// correspond to branches with active tmux sessions, avoiding unnecessary
/// work for unrelated sessions.
///
/// Uses adaptive polling: when any agent is Running or Waiting, polls at
/// `base_interval`. When all agents are Idle/Unknown, backs off to
/// `3 × base_interval` to save resources.
pub(super) fn spawn_agent_status_poller<T: TmuxProvider + ?Sized + 'static>(
    tmux: &Arc<T>,
    sender: &EventSender,
    cancel: Arc<AtomicBool>,
    base_interval: std::time::Duration,
    session_names: Vec<String>,
) {
    let tmux = Arc::clone(tmux);
    let sender = sender.clone();
    let idle_interval = base_interval.saturating_mul(3);
    thread::spawn(move || {
        let is_cancelled =
            || cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed);
        let mut current_interval;
        loop {
            if is_cancelled() {
                return;
            }

            let states = detect_agent_statuses(&*tmux, &session_names);

            // Adapt interval: use fast polling when any agent is active,
            // slow polling when all are idle/unknown.
            let any_active = states.iter().any(|update| {
                update.agent_statuses.iter().any(|s| {
                    matches!(
                        s.state,
                        agent::AgentState::Running | agent::AgentState::Waiting
                    )
                })
            });
            current_interval = if any_active {
                base_interval
            } else {
                idle_interval
            };

            sender.send(AppEvent::AgentStatesUpdated { states });

            // Sleep in small increments so we can check cancel promptly
            let mut remaining = current_interval;
            while !remaining.is_zero() {
                if is_cancelled() {
                    return;
                }
                let sleep = remaining.min(std::time::Duration::from_millis(200));
                thread::sleep(sleep);
                remaining = remaining.saturating_sub(sleep);
            }
        }
    });
}

fn detect_agent_statuses<T: TmuxProvider + ?Sized>(
    tmux: &T,
    sessions: &[String],
) -> Vec<SessionRuntimeUpdate> {
    let sessions_with_activity: HashMap<String, u64> =
        tmux.list_sessions_with_activity().into_iter().collect();
    // Batch: fetch all pane info + session activity in a single tmux call,
    // then detect agents using the pre-fetched data. Only capture_pane_content
    // still requires per-pane calls.
    let all_pane_data = tmux.list_all_panes_with_activity();
    agent::detect_all_for_sessions_batched(tmux, sessions, &all_pane_data)
        .into_iter()
        .map(|(session_name, result)| {
            let session_activity_ts = sessions_with_activity
                .get(&session_name)
                .copied()
                .or_else(|| all_pane_data.get(&session_name).map(|d| d.session_activity));
            SessionRuntimeUpdate {
                session_exists: sessions_with_activity.contains_key(&session_name)
                    || all_pane_data.contains_key(&session_name),
                session_name,
                session_activity_ts,
                agent_statuses: result.into_iter().map(|r| r.status).collect(),
            }
        })
        .collect()
}

fn session_agent_states(
    updates: Vec<SessionRuntimeUpdate>,
) -> Vec<(String, Vec<kiosk_core::agent::AgentStatus>)> {
    updates
        .into_iter()
        .map(|update| (update.session_name, update.agent_statuses))
        .collect()
}

fn enrich_missing_worktrees(
    git: &dyn GitProvider,
    repos: Vec<(String, String, PathBuf, Vec<kiosk_core::git::Worktree>)>,
) -> Vec<(String, String, PathBuf, Vec<kiosk_core::git::Worktree>)> {
    repos
        .into_iter()
        .map(|(name, session_name, path, worktrees)| {
            if worktrees.is_empty() {
                (name, session_name, path.clone(), git.list_worktrees(&path))
            } else {
                (name, session_name, path, worktrees)
            }
        })
        .collect()
}

fn discover_sessions_from_repos<T: TmuxProvider + ?Sized>(
    tmux: &T,
    repos: &[(String, String, PathBuf, Vec<kiosk_core::git::Worktree>)],
) -> (Vec<kiosk_core::state::SessionEntry>, Vec<String>) {
    let sessions_with_activity = tmux.list_sessions_with_activity();
    let active_sessions: HashMap<String, u64> = sessions_with_activity.into_iter().collect();
    let attached_clients: std::collections::HashSet<String> = active_sessions
        .keys()
        .filter(|session_name| !tmux.list_clients(session_name).is_empty())
        .cloned()
        .collect();

    let mut sessions = Vec::new();
    let mut session_names = Vec::new();

    for (repo_name, repo_session_name, repo_path, worktrees) in repos {
        for wt in worktrees {
            let session_name = kiosk_core::git::tmux_session_name_for_worktree(
                repo_name,
                repo_session_name,
                repo_path,
                &wt.path,
            );

            if let Some(&activity) = active_sessions.get(&session_name) {
                sessions.push(kiosk_core::state::SessionEntry {
                    session_name: session_name.clone(),
                    repo_name: repo_name.clone(),
                    branch: wt.branch.clone(),
                    path: wt.path.clone(),
                    agent_statuses: Vec::new(),
                    session_activity: activity,
                    attached: attached_clients.contains(&session_name),
                });
                session_names.push(session_name);
            }
        }
    }

    (sessions, session_names)
}

/// Spawn background session discovery for the sessions view.
/// Uses already-discovered repos from state to find active sessions.
pub(super) fn spawn_sessions_discovery<T: TmuxProvider + ?Sized + 'static>(
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
    state: &mut kiosk_core::state::AppState,
) {
    // Collect repo data while we have access to state
    let repos: Vec<(String, String, PathBuf, Vec<kiosk_core::git::Worktree>)> = state
        .repos
        .iter()
        .map(|r| {
            (
                r.name.clone(),
                r.session_name.clone(),
                r.path.clone(),
                r.worktrees.clone(),
            )
        })
        .collect();

    let git = Arc::clone(git);
    let tmux_clone = Arc::clone(tmux);
    let sender_clone = sender.clone();

    // Cancel any existing sessions poller
    state.cancel_all_agent_pollers();

    let poller_cancel = Arc::new(AtomicBool::new(false));
    state.install_sessions_poller(Arc::clone(&poller_cancel));

    let poller_cancel_for_poller = Arc::clone(&poller_cancel);
    let tmux_for_poller = Arc::clone(tmux);
    let sender_for_poller = sender.clone();

    thread::spawn(move || {
        if sender_clone.cancel.load(Ordering::Relaxed) {
            return;
        }

        // Re-enrich only repos that are still missing worktrees.
        let repos = enrich_missing_worktrees(&*git, repos);
        let (sessions, session_names) = discover_sessions_from_repos(&*tmux_clone, &repos);
        let states = if session_names.is_empty() {
            Vec::new()
        } else {
            session_agent_states(detect_agent_statuses(&*tmux_clone, &session_names))
        };
        sender_clone.send(AppEvent::SessionsSnapshot { sessions, states });

        // Start the sessions refresh/agent poller.
        spawn_sessions_agent_poller(
            &tmux_for_poller,
            &sender_for_poller,
            poller_cancel_for_poller,
            std::time::Duration::from_secs(3),
            repos,
        );
    });
}

/// Sessions poller for the sessions view — refreshes session membership and
/// agent statuses on each cycle.
fn spawn_sessions_agent_poller<T: TmuxProvider + ?Sized + 'static>(
    tmux: &Arc<T>,
    sender: &EventSender,
    cancel: Arc<AtomicBool>,
    base_interval: std::time::Duration,
    repos: Vec<(String, String, PathBuf, Vec<kiosk_core::git::Worktree>)>,
) {
    let tmux = Arc::clone(tmux);
    let sender = sender.clone();
    let idle_interval = base_interval.saturating_mul(3);
    thread::spawn(move || {
        let is_cancelled =
            || cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed);
        let mut current_interval = base_interval;
        loop {
            // Sleep first since we already did an initial detection
            let mut remaining = current_interval;
            while !remaining.is_zero() {
                if is_cancelled() {
                    return;
                }
                let sleep = remaining.min(std::time::Duration::from_millis(200));
                thread::sleep(sleep);
                remaining = remaining.saturating_sub(sleep);
            }

            if is_cancelled() {
                return;
            }

            let (sessions, session_names) = discover_sessions_from_repos(&*tmux, &repos);
            if session_names.is_empty() {
                sender.send(AppEvent::SessionsSnapshot {
                    sessions,
                    states: Vec::new(),
                });
                current_interval = idle_interval;
                continue;
            }

            let states = session_agent_states(detect_agent_statuses(&*tmux, &session_names));
            let any_active = states.iter().any(|(_, statuses)| {
                statuses.iter().any(|s| {
                    matches!(
                        s.state,
                        kiosk_core::agent::AgentState::Running
                            | kiosk_core::agent::AgentState::Waiting
                    )
                })
            });
            current_interval = if any_active {
                base_interval
            } else {
                idle_interval
            };
            sender.send(AppEvent::SessionsSnapshot { sessions, states });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{detect_agent_statuses, discover_sessions_from_repos, enrich_missing_worktrees};
    use kiosk_core::{
        agent::{AgentKind, AgentState},
        git::{Worktree, mock::MockGitProvider},
        tmux::{mock::MockTmuxProvider, provider::PaneInfo},
    };
    use std::path::PathBuf;

    #[allow(clippy::similar_names)]
    #[test]
    fn poller_detects_wrapper_command_via_pane_title_for_claude_waiting() {
        let mut tmux = MockTmuxProvider::default();
        let session = "kiosk--feat-agent-status".to_string();
        tmux.sessions_with_activity = vec![(session.clone(), 123)];

        tmux.pane_info.insert(
            session.clone(),
            vec![PaneInfo {
                pane_id: "%149".to_string(),
                command: "2.1.63".to_string(),
                pid: 82078,
            }],
        );
        tmux.pane_titles
            .insert("%149".to_string(), "✳ Claude Code".to_string());

        let history = (1..=120)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!("{history}\nBash command\nDo you want to proceed?\n❯ 1. Yes\n2. No");
        tmux.pane_content.insert("%149".to_string(), content);

        let states = detect_agent_statuses(&tmux, std::slice::from_ref(&session));
        assert_eq!(states.len(), 1);
        let update = &states[0];
        assert_eq!(update.session_name, session);
        assert!(update.session_exists);
        assert!(update.session_activity_ts.is_some());
        assert!(
            !update.agent_statuses.is_empty(),
            "wrapper command should resolve to Claude via pane title"
        );
        let status = &update.agent_statuses[0];
        assert_eq!(status.kind, AgentKind::ClaudeCode);
        assert_eq!(status.state, AgentState::Waiting);
    }

    #[test]
    fn poller_reports_session_exists_from_session_list_without_panes() {
        let mut tmux = MockTmuxProvider::default();
        let session = "kiosk--empty-session".to_string();
        tmux.sessions_with_activity = vec![(session.clone(), 456)];

        let states = detect_agent_statuses(&tmux, std::slice::from_ref(&session));
        assert_eq!(states.len(), 1);
        let update = &states[0];
        assert_eq!(update.session_name, session);
        assert!(update.session_exists);
        assert_eq!(update.session_activity_ts, Some(456));
        assert!(update.agent_statuses.is_empty());
    }

    #[test]
    fn discover_sessions_includes_only_active_kiosk_sessions() {
        let mut tmux = MockTmuxProvider {
            sessions_with_activity: vec![("alpha".into(), 100), ("beta--feat".into(), 200)],
            ..Default::default()
        };
        tmux.clients.insert("beta--feat".into(), vec!["1".into()]);

        let repos = vec![
            (
                "alpha".to_string(),
                "alpha".to_string(),
                PathBuf::from("/tmp/alpha"),
                vec![Worktree {
                    path: PathBuf::from("/tmp/alpha"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            ),
            (
                "beta".to_string(),
                "beta".to_string(),
                PathBuf::from("/tmp/beta"),
                vec![Worktree {
                    path: PathBuf::from("/tmp/beta--feat"),
                    branch: Some("feat".to_string()),
                    is_main: false,
                }],
            ),
            (
                "gamma".to_string(),
                "gamma".to_string(),
                PathBuf::from("/tmp/gamma"),
                vec![Worktree {
                    path: PathBuf::from("/tmp/gamma--dev"),
                    branch: Some("dev".to_string()),
                    is_main: false,
                }],
            ),
        ];

        let (sessions, session_names) = discover_sessions_from_repos(&tmux, &repos);
        assert_eq!(session_names, vec!["alpha".to_string(), "beta--feat".to_string()]);
        assert_eq!(sessions.len(), 2);
        assert!(!sessions[0].attached);
        assert!(sessions[1].attached);
    }

    #[test]
    fn discover_sessions_uses_repo_session_name_for_disambiguation() {
        let tmux = MockTmuxProvider {
            sessions_with_activity: vec![("api--(work)--feat".into(), 123)],
            ..Default::default()
        };

        let repos = vec![(
            "api".to_string(),
            "api--(work)".to_string(),
            PathBuf::from("/tmp/work/api"),
            vec![Worktree {
                path: PathBuf::from("/tmp/worktrees/api--feat"),
                branch: Some("feat".to_string()),
                is_main: false,
            }],
        )];

        let (sessions, session_names) = discover_sessions_from_repos(&tmux, &repos);
        assert_eq!(session_names, vec!["api--(work)--feat".to_string()]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_name, "api--(work)--feat");
    }

    #[test]
    fn enrich_missing_worktrees_only_fills_empty_repos() {
        let git = MockGitProvider {
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/enriched"),
                branch: Some("feature".to_string()),
                is_main: false,
            }],
            ..Default::default()
        };

        let repos = vec![
            (
                "already".to_string(),
                "already".to_string(),
                PathBuf::from("/tmp/already"),
                vec![Worktree {
                    path: PathBuf::from("/tmp/already"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            ),
            (
                "empty".to_string(),
                "empty".to_string(),
                PathBuf::from("/tmp/empty"),
                Vec::new(),
            ),
        ];

        let enriched = enrich_missing_worktrees(&git, repos);
        assert_eq!(enriched[0].3[0].path, PathBuf::from("/tmp/already"));
        assert_eq!(enriched[1].3[0].path, PathBuf::from("/tmp/enriched"));
    }
}
