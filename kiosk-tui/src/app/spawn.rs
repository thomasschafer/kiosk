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
/// Maximum number of concurrent session agent detections.
const SESSION_STATUS_POOL_SIZE: usize = 8;

/// Maximum number of concurrent per-remote `git fetch` calls.
const FETCH_POOL_SIZE: usize = 4;

/// Spawns work across a thread pool (if available) or falls back to individual threads.
/// `build_work` is called for each item to produce a `FnOnce() + Send + 'static` closure.
fn spawn_work_parallel<T, F, W>(pool_size: usize, items: impl IntoIterator<Item = T>, build_work: F)
where
    F: Fn(T) -> W,
    W: FnOnce() + Send + 'static,
{
    let pool = ThreadPoolBuilder::new().num_threads(pool_size).build().ok();

    match &pool {
        Some(pool) => {
            for item in items {
                pool.spawn(build_work(item));
            }
        }
        None => {
            for item in items {
                thread::spawn(build_work(item));
            }
        }
    }
}

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
    // Fall back to the session list for existence/activity when a session has
    // no panes (e.g. a freshly created session that tmux list-panes hasn't
    // seen yet).
    let session_list: std::collections::HashMap<String, u64> =
        tmux.list_sessions_with_activity().into_iter().collect();
    agent::detect_all_for_sessions_batched(tmux, sessions, &all_pane_data)
        .into_iter()
        .map(|(session_name, result)| {
            let session_activity_ts = all_pane_data
                .get(&session_name)
                .map(|d| d.session_activity)
                .or_else(|| session_list.get(&session_name).copied());
            SessionRuntimeUpdate {
                session_exists: all_pane_data.contains_key(&session_name)
                    || session_list.contains_key(&session_name),
                session_name,
                session_activity_ts,
                agent_statuses: kiosk_core::state::normalized_agent_statuses(
                    &result.into_iter().map(|r| r.status).collect::<Vec<_>>(),
                ),
            }
        })
        .collect()
}

fn build_session_discovered_snapshot(
    sessions_with_activity: &[(String, u64)],
    attached_sessions: &HashSet<String>,
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
            )
        })
        .collect()
}

/// Spawn agent detection threads for all sessions.
///
/// Uses plain `thread::spawn` rather than a thread pool because this is pure
/// I/O (tmux subprocess calls with latency ~5-50ms). Creating a rayon pool on
/// every ~2s poll would spin up and tear down 8 OS threads constantly.
fn stream_session_agent_states<T: TmuxProvider + ?Sized + 'static>(
    tmux: &Arc<T>,
    session_names: Vec<String>,
    sender: &EventSender,
    cancel: &Arc<AtomicBool>,
) {
    if session_names.is_empty() {
        return;
    }

    let all_pane_data = tmux.list_all_panes_with_activity();
    let (tx, rx) = mpsc::channel::<(String, Vec<kiosk_core::agent::AgentStatus>)>();

    for session_name in session_names {
        let tx = tx.clone();
        let tmux = Arc::clone(tmux);
        let pane_data = all_pane_data.get(&session_name).cloned();
        let cancel = Arc::clone(cancel);
        thread::spawn(move || {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let statuses = pane_data
                .map(|data| {
                    kiosk_core::state::normalized_agent_statuses(
                        &agent::detect_all_for_session_from_pane_data(&*tmux, &data)
                            .into_iter()
                            .map(|result| result.status)
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default();
            let _ = tx.send((session_name, statuses));
        });
    }
    drop(tx);

    for state in rx {
        if cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        sender.send(AppEvent::SessionAgentStatesPatched {
            states: vec![state],
        });
    }
}

fn resolve_repo_session_metadata(
    git: &dyn GitProvider,
    repo: Repo,
    session_activity: &HashMap<String, u64>,
) -> Vec<kiosk_core::event::SessionMetadataPatch> {
    let mut repo = repo;
    repo.worktrees = git.list_worktrees(&repo.path);
    repo.worktrees
        .iter()
        .filter_map(|worktree| {
            let session_name = repo.tmux_session_name(&worktree.path);
            session_activity.contains_key(&session_name).then_some(
                kiosk_core::event::SessionMetadataPatch {
                    session_name,
                    metadata: kiosk_core::state::SessionMetadata::Resolved(
                        kiosk_core::state::ResolvedSessionMetadata {
                            repo_name: repo.name.clone(),
                            branch: worktree.branch.clone(),
                            path: worktree.path.clone(),
                        },
                    ),
                },
            )
        })
        .collect()
}

fn stream_session_metadata(
    git: &Arc<dyn GitProvider>,
    search_dirs: &[(PathBuf, u16)],
    sessions: &[kiosk_core::state::SessionEntry],
    sender: &EventSender,
    cancel: &Arc<AtomicBool>,
) {
    let active_sessions: HashSet<String> = sessions
        .iter()
        .map(|session| session.session_name.clone())
        .collect();
    if active_sessions.is_empty() {
        sender.send(AppEvent::SessionMetadataPatched {
            patches: Vec::new(),
            complete: true,
        });
        return;
    }

    let session_activity: HashMap<String, u64> = sessions
        .iter()
        .map(|session| (session.session_name.clone(), session.session_activity))
        .collect();
    let is_cancelled = || cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed);
    let mut repos = git.scan_repos(search_dirs);
    apply_repo_name_collision_resolution(&mut repos, search_dirs);
    let candidate_repos: Vec<Repo> = repos
        .into_iter()
        .take_while(|_| !is_cancelled())
        .filter(|repo| repo_matches_active_session(repo, &active_sessions))
        .collect();

    if candidate_repos.is_empty() {
        sender.send(AppEvent::SessionMetadataPatched {
            patches: Vec::new(),
            complete: true,
        });
        return;
    }

    let (tx, rx) = mpsc::channel::<Vec<kiosk_core::event::SessionMetadataPatch>>();

    spawn_work_parallel(ENRICHMENT_POOL_SIZE, candidate_repos, |repo| {
        let tx = tx.clone();
        let git = Arc::clone(git);
        let session_activity = session_activity.clone();
        let cancel = Arc::clone(cancel);
        move || {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = tx.send(resolve_repo_session_metadata(
                &*git,
                repo,
                &session_activity,
            ));
        }
    });
    drop(tx);

    for patches in rx {
        if is_cancelled() {
            return;
        }
        if !patches.is_empty() {
            sender.send(AppEvent::SessionMetadataPatched {
                patches,
                complete: false,
            });
        }
    }

    if !is_cancelled() {
        sender.send(AppEvent::SessionMetadataPatched {
            patches: Vec::new(),
            complete: true,
        });
    }
}

fn refresh_sessions_view<T: TmuxProvider + ?Sized + 'static>(
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
    search_dirs: &[(PathBuf, u16)],
    cancel: &Arc<AtomicBool>,
) -> Vec<kiosk_core::state::SessionEntry> {
    let sessions_with_activity = tmux.list_sessions_with_activity();
    let attached_sessions = tmux.list_attached_sessions();
    let sessions = build_session_discovered_snapshot(&sessions_with_activity, &attached_sessions);
    sender.send(AppEvent::SessionsDiscovered {
        sessions: sessions.clone(),
    });

    let is_cancelled = || cancel.load(Ordering::Relaxed) || sender.cancel.load(Ordering::Relaxed);
    if is_cancelled() {
        return sessions;
    }

    let metadata_git = Arc::clone(git);
    let metadata_sender = sender.clone();
    let metadata_search_dirs = search_dirs.to_vec();
    let metadata_sessions = sessions.clone();
    let metadata_cancel = Arc::clone(cancel);
    thread::spawn(move || {
        stream_session_metadata(
            &metadata_git,
            &metadata_search_dirs,
            &metadata_sessions,
            &metadata_sender,
            &metadata_cancel,
        );
    });

    let status_tmux = Arc::clone(tmux);
    let status_sender = sender.clone();
    let status_cancel = Arc::clone(cancel);
    let session_names: Vec<String> = sessions
        .iter()
        .map(|session| session.session_name.clone())
        .collect();
    thread::spawn(move || {
        stream_session_agent_states(&status_tmux, session_names, &status_sender, &status_cancel);
    });

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
    if let Err(err) = state.install_sessions_poller(poller_handle.clone()) {
        log::warn!("sessions discovery aborted: poller install failed: {err:?}");
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
            &tmux_clone,
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
                let sessions = refresh_sessions_view(&git, &tmux, &sender, &search_dirs, &cancel);
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
                stream_session_agent_states(&tmux, known_session_names.clone(), &sender, &cancel);
                last_status_tick = now;
            }

            // Sleep in small increments so cancellation is checked promptly.
            // Cap the tick at 150 ms regardless of the configured interval so
            // the loop remains responsive; also cap at status_interval / 4 so
            // fast intervals (< 600 ms) aren't rounded up to 150 ms.
            let tick = status_interval.div_f32(4.0).min(Duration::from_millis(150));
            thread::sleep(tick);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        build_session_discovered_snapshot, detect_agent_statuses, resolve_repo_session_metadata,
        stream_session_agent_states,
    };
    use crate::app::EventSender;
    use kiosk_core::{
        agent::{AgentKind, AgentState},
        event::AppEvent,
        git::{Repo, Worktree, mock::MockGitProvider},
        tmux::{mock::MockTmuxProvider, provider::PaneInfo, provider::TmuxProvider},
    };
    use std::{
        path::PathBuf,
        sync::{Arc, atomic::AtomicBool, mpsc},
    };

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
            build_session_discovered_snapshot(&tmux.list_sessions_with_activity(), &attached);
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
        );

        let session_names: Vec<&str> = sessions
            .iter()
            .map(|session| session.session_name.as_str())
            .collect();
        assert_eq!(session_names, vec!["beta", "gamma", "alpha"]);
    }

    #[test]
    fn stream_session_agent_states_streams_one_patch_event_per_session() {
        let mut tmux = MockTmuxProvider::default();
        tmux.pane_info.insert(
            "alpha".into(),
            vec![PaneInfo {
                pane_id: "%1".into(),
                command: "bash".into(),
                pid: 1,
            }],
        );
        tmux.pane_info.insert(
            "beta".into(),
            vec![PaneInfo {
                pane_id: "%2".into(),
                command: "bash".into(),
                pid: 2,
            }],
        );
        tmux.session_activity_ts.insert("alpha".into(), 10);
        tmux.session_activity_ts.insert("beta".into(), 20);
        let tmux = Arc::new(tmux);

        let (tx, rx) = mpsc::channel();
        let sender = EventSender {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
        };

        let cancel = Arc::new(AtomicBool::new(false));
        stream_session_agent_states(&tmux, vec!["alpha".into(), "beta".into()], &sender, &cancel);

        let events: Vec<AppEvent> = rx.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| matches!(
            event,
            AppEvent::SessionAgentStatesPatched { states } if states.len() == 1
        )));
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
        );
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].session_name, "api--(work)--feat");
        match &resolved[0].metadata {
            kiosk_core::state::SessionMetadata::Resolved(metadata) => {
                assert_eq!(metadata.repo_name, "api");
            }
            kiosk_core::state::SessionMetadata::Unresolved => {
                panic!("metadata should be resolved");
            }
        }
    }
}
