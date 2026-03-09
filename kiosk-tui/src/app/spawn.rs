use kiosk_core::{
    agent,
    event::{AppEvent, SessionRuntimeUpdate},
    git::{GitProvider, Repo, apply_repo_name_collision_resolution, repo_matches_active_session},
    state::{BranchEntry, PollerHandle},
};
use rayon::ThreadPoolBuilder;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use kiosk_core::tmux::TmuxProvider;

use super::EventSender;

/// Maximum number of concurrent `git worktree list` enrichment calls.
const ENRICHMENT_POOL_SIZE: usize = 8;

/// Maximum number of concurrent per-remote `git fetch` calls.
const FETCH_POOL_SIZE: usize = 4;
/// Sessions membership updates can be slower than agent status updates.
const SESSIONS_MEMBERSHIP_POLL_INTERVAL: Duration = Duration::from_secs(3);
struct SessionsPollerConfig {
    status_interval: Duration,
    membership_interval: Duration,
    search_dirs: Vec<(PathBuf, u16)>,
}

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
    let all_pane_data = tmux.list_all_panes_with_activity();
    detect_agent_statuses_from_pane_data(tmux, sessions, &all_pane_data)
}

fn detect_agent_statuses_from_pane_data<T: TmuxProvider + ?Sized>(
    tmux: &T,
    sessions: &[String],
    all_pane_data: &HashMap<String, kiosk_core::tmux::provider::SessionPaneData>,
) -> Vec<SessionRuntimeUpdate> {
    agent::detect_all_for_sessions_batched(tmux, sessions, &all_pane_data)
        .into_iter()
        .map(|(session_name, result)| {
            let session_activity_ts = all_pane_data.get(&session_name).map(|d| d.session_activity);
            SessionRuntimeUpdate {
                session_exists: all_pane_data.contains_key(&session_name),
                session_name,
                session_activity_ts,
                agent_statuses: kiosk_core::state::normalized_agent_statuses(
                    &result.into_iter().map(|r| r.status).collect::<Vec<_>>(),
                ),
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

fn build_session_discovered_snapshot(
    sessions_with_activity: &[(String, u64)],
    attached_sessions: &HashSet<String>,
    current_session_name: Option<&str>,
) -> Vec<kiosk_core::state::SessionEntry> {
    let mut sessions_with_activity = sessions_with_activity.to_vec();
    sessions_with_activity
        .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    sessions_with_activity
        .into_iter()
        .map(|(session_name, session_activity)| {
            kiosk_core::state::SessionEntry::unresolved(
                session_name.clone(),
                Vec::new(),
                session_activity,
                attached_sessions.contains(&session_name),
                current_session_name == Some(session_name.as_str()),
            )
        })
        .collect()
}

fn detect_session_agent_states<T: TmuxProvider + ?Sized>(
    tmux: &T,
    sessions: &[kiosk_core::state::SessionEntry],
) -> Vec<(String, Vec<kiosk_core::agent::AgentStatus>)> {
    if sessions.is_empty() {
        return Vec::new();
    }

    let all_pane_data = tmux.list_all_panes_with_activity();
    let session_names: Vec<String> = sessions
        .iter()
        .map(|session| session.session_name.clone())
        .collect();
    session_agent_states(detect_agent_statuses_from_pane_data(
        tmux,
        &session_names,
        &all_pane_data,
    ))
}

fn resolve_repo_session_metadata(
    git: &dyn GitProvider,
    repo: Repo,
    session_activity: &HashMap<String, u64>,
    attached_sessions: &HashSet<String>,
    current_session_name: Option<&str>,
    by_session: &HashMap<String, Vec<kiosk_core::agent::AgentStatus>>,
) -> Vec<kiosk_core::state::SessionEntry> {
    let mut repo = repo;
    repo.worktrees = git.list_worktrees(&repo.path);
    let mut resolved = kiosk_core::state::active_session_entries(
        &[repo],
        session_activity,
        attached_sessions,
        current_session_name,
    );
    for session in &mut resolved {
        if let Some(agent_statuses) = by_session.get(session.session_name.as_str()) {
            session.agent_statuses = agent_statuses.clone();
        }
    }
    resolved
}

fn stream_session_metadata(
    git: Arc<dyn GitProvider>,
    search_dirs: Vec<(PathBuf, u16)>,
    sessions: &[kiosk_core::state::SessionEntry],
    sender: &EventSender,
    cancel: &Arc<AtomicBool>,
) {
    let active_sessions: HashSet<String> = sessions
        .iter()
        .map(|session| session.session_name.clone())
        .collect();
    if active_sessions.is_empty() {
        sender.send(AppEvent::SessionMetadataResolved {
            sessions: Vec::new(),
            complete: true,
        });
        return;
    }

    let session_activity: HashMap<String, u64> = sessions
        .iter()
        .map(|session| (session.session_name.clone(), session.session_activity))
        .collect();
    let attached_sessions: HashSet<String> = sessions
        .iter()
        .filter(|session| session.attached)
        .map(|session| session.session_name.clone())
        .collect();
    let current_session_name = sessions
        .iter()
        .find(|session| session.is_current)
        .map(|session| session.session_name.as_str());
    let by_session: HashMap<String, Vec<kiosk_core::agent::AgentStatus>> = sessions
        .iter()
        .map(|session| (session.session_name.clone(), session.agent_statuses.clone()))
        .collect();
    let is_cancelled = || cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed);
    let mut repos = git.scan_repos(&search_dirs);
    apply_repo_name_collision_resolution(&mut repos, &search_dirs);
    let candidate_repos: Vec<Repo> = repos
        .into_iter()
        .take_while(|_| !is_cancelled())
        .filter(|repo| repo_matches_active_session(repo, &active_sessions))
        .collect();

    if candidate_repos.is_empty() {
        sender.send(AppEvent::SessionMetadataResolved {
            sessions: Vec::new(),
            complete: true,
        });
        return;
    }

    let (tx, rx) = mpsc::channel::<Vec<kiosk_core::state::SessionEntry>>();
    for repo in candidate_repos {
        let tx = tx.clone();
        let git = Arc::clone(&git);
        let session_activity = session_activity.clone();
        let attached_sessions = attached_sessions.clone();
        let current_session_name = current_session_name.map(str::to_string);
        let by_session = by_session.clone();
        let cancel = Arc::clone(cancel);

        thread::spawn(move || {
            if cancel.load(Ordering::Relaxed) {
                return;
            }

            let resolved = resolve_repo_session_metadata(
                &*git,
                repo,
                &session_activity,
                &attached_sessions,
                current_session_name.as_deref(),
                &by_session,
            );
            let _ = tx.send(resolved);
        });
    }
    drop(tx);

    for resolved in rx {
        if is_cancelled() {
            return;
        }
        if !resolved.is_empty() {
            sender.send(AppEvent::SessionMetadataResolved {
                sessions: resolved,
                complete: false,
            });
        }
    }

    if !is_cancelled() {
        sender.send(AppEvent::SessionMetadataResolved {
            sessions: Vec::new(),
            complete: true,
        });
    }
}

fn refresh_sessions_view<T: TmuxProvider + ?Sized>(
    git: &Arc<dyn GitProvider>,
    tmux: &T,
    sender: &EventSender,
    search_dirs: &[(PathBuf, u16)],
    cancel: &Arc<AtomicBool>,
) -> Vec<kiosk_core::state::SessionEntry> {
    let sessions_with_activity = tmux.list_sessions_with_activity();
    let attached_sessions = tmux.list_attached_sessions();
    let current_session_name = tmux.current_session_name();
    let sessions = build_session_discovered_snapshot(
        &sessions_with_activity,
        &attached_sessions,
        current_session_name.as_deref(),
    );
    sender.send(AppEvent::SessionsDiscovered {
        sessions: sessions.clone(),
    });

    let is_cancelled = || cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed);
    if is_cancelled() {
        return sessions;
    }

    stream_session_metadata(
        Arc::clone(git),
        search_dirs.to_vec(),
        &sessions,
        sender,
        cancel,
    );

    if !is_cancelled() {
        let states = detect_session_agent_states(tmux, &sessions);
        if !states.is_empty() {
            sender.send(AppEvent::SessionAgentStatesUpdated { states });
        }
    }

    sessions
}

/// Spawn background session discovery for the sessions view.
pub(super) fn spawn_sessions_discovery<T: TmuxProvider + ?Sized + 'static>(
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
    state: &mut kiosk_core::state::AppState,
) {
    let search_dirs = state.search_dirs.clone();
    let status_interval = state.agent_poll_interval;
    let git = Arc::clone(git);
    let tmux_clone = Arc::clone(tmux);
    let sender_clone = sender.clone();

    let poller_handle = PollerHandle::new();
    if state
        .install_sessions_poller(poller_handle.clone())
        .is_err()
    {
        return;
    }
    let poller_cancel = poller_handle.cancel_token();

    let poller_cancel_for_poller = Arc::clone(&poller_cancel);
    let tmux_for_poller = Arc::clone(tmux);
    let sender_for_poller = sender.clone();
    let git_for_poller = Arc::clone(&git);
    let search_dirs_for_poller = search_dirs.clone();

    thread::spawn(move || {
        if poller_cancel.load(Ordering::Relaxed) || sender_clone.cancel.load(Ordering::Relaxed) {
            return;
        }

        refresh_sessions_view(
            &git,
            &*tmux_clone,
            &sender_clone,
            &search_dirs,
            &poller_cancel,
        );

        // Start sessions pollers: fast status updates + slower membership diffs.
        let config = SessionsPollerConfig {
            status_interval,
            membership_interval: SESSIONS_MEMBERSHIP_POLL_INTERVAL,
            search_dirs: search_dirs_for_poller,
        };
        spawn_sessions_agent_poller(
            &git_for_poller,
            &tmux_for_poller,
            &sender_for_poller,
            poller_cancel_for_poller,
            config,
        );
    });
}

/// Sessions poller for sessions mode.
/// Statuses update at `status_interval`; membership diffs run at the slower
/// `membership_interval`.
fn spawn_sessions_agent_poller<T: TmuxProvider + ?Sized + 'static>(
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
    cancel: Arc<AtomicBool>,
    config: SessionsPollerConfig,
) {
    let SessionsPollerConfig {
        status_interval,
        membership_interval,
        search_dirs,
    } = config;
    let git = Arc::clone(git);
    let tmux = Arc::clone(tmux);
    let sender = sender.clone();
    thread::spawn(move || {
        let is_cancelled =
            || cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed);
        let mut known_session_names: Vec<String> = Vec::new();
        let mut last_status_tick = std::time::Instant::now();
        let mut last_membership_tick = std::time::Instant::now();

        loop {
            if is_cancelled() {
                return;
            }

            let now = std::time::Instant::now();

            if now.duration_since(last_membership_tick) >= membership_interval {
                let sessions = refresh_sessions_view(&git, &*tmux, &sender, &search_dirs, &cancel);
                known_session_names = sessions
                    .iter()
                    .map(|session| session.session_name.clone())
                    .collect();
                if is_cancelled() {
                    return;
                }
                last_membership_tick = now;
                last_status_tick = now;
            }

            if now.duration_since(last_status_tick) >= status_interval
                && !known_session_names.is_empty()
            {
                let states =
                    session_agent_states(detect_agent_statuses(&*tmux, &known_session_names));
                sender.send(AppEvent::SessionAgentStatesUpdated { states });
                last_status_tick = now;
            }

            thread::sleep(Duration::from_millis(150));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        build_session_discovered_snapshot, detect_agent_statuses, resolve_repo_session_metadata,
    };
    use kiosk_core::{
        agent::{AgentKind, AgentState},
        git::{Repo, Worktree, mock::MockGitProvider},
        tmux::{mock::MockTmuxProvider, provider::PaneInfo, provider::TmuxProvider},
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
    fn build_session_discovered_snapshot_includes_active_sessions() {
        let mut tmux = MockTmuxProvider {
            sessions_with_activity: vec![("alpha".into(), 100), ("beta--feat".into(), 200)],
            ..Default::default()
        };
        tmux.clients.insert("beta--feat".into(), vec!["1".into()]);

        let attached = tmux.list_attached_sessions();
        let sessions =
            build_session_discovered_snapshot(&tmux.list_sessions_with_activity(), &attached, None);
        let session_names: Vec<String> = sessions
            .iter()
            .map(|session| session.session_name.clone())
            .collect();
        assert_eq!(
            session_names,
            vec!["beta--feat".to_string(), "alpha".to_string()]
        );
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].attached);
        assert!(!sessions[1].attached);
        assert!(
            sessions
                .iter()
                .all(|session| session.agent_statuses.is_empty())
        );
    }

    #[test]
    fn build_session_discovered_snapshot_uses_tmux_recency_not_current_session_priority() {
        let sessions = build_session_discovered_snapshot(
            &[
                ("alpha".to_string(), 100),
                ("beta".to_string(), 200),
                ("gamma".to_string(), 150),
            ],
            &std::collections::HashSet::new(),
            Some("alpha"),
        );

        let session_names: Vec<&str> = sessions
            .iter()
            .map(|session| session.session_name.as_str())
            .collect();
        assert_eq!(session_names, vec!["beta", "gamma", "alpha"]);
        assert!(sessions[2].is_current);
    }

    #[test]
    fn resolve_session_metadata_uses_repo_session_name_for_disambiguation() {
        let git = MockGitProvider {
            repos: vec![
                Repo {
                    name: "api".to_string(),
                    session_name: "api".to_string(),
                    path: PathBuf::from("/tmp/work/api"),
                    worktrees: Vec::new(),
                },
                Repo {
                    name: "api".to_string(),
                    session_name: "api".to_string(),
                    path: PathBuf::from("/tmp/personal/api"),
                    worktrees: Vec::new(),
                },
            ],
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/worktrees/api--feat"),
                branch: Some("feat".to_string()),
                is_main: false,
            }],
            ..Default::default()
        };
        let mut session_activity = std::collections::HashMap::new();
        session_activity.insert("api--(work)--feat".to_string(), 123);

        let resolved = resolve_repo_session_metadata(
            &git,
            Repo {
                name: "api".to_string(),
                session_name: "api--(work)".to_string(),
                path: PathBuf::from("/tmp/work/api"),
                worktrees: Vec::new(),
            },
            &session_activity,
            &std::collections::HashSet::new(),
            None,
            &std::collections::HashMap::new(),
        );
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].session_name, "api--(work)--feat");
        assert_eq!(resolved[0].repo_name(), Some("api"));
    }
}
