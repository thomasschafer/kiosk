use kiosk_core::{
    agent::{self, AgentKind},
    event::AppEvent,
    git::GitProvider,
    state::BranchEntry,
};
use rayon::ThreadPoolBuilder;
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{Arc, atomic::Ordering},
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
    local_names: Vec<String>,
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
        let local_names = Arc::new(local_names);

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

/// Interval between agent status polls
const AGENT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Spawns a background thread that periodically detects agent states for the
/// given tmux sessions. Runs until the sender's cancel flag is set.
pub fn spawn_agent_status_poller<T: TmuxProvider + ?Sized + 'static>(
    tmux: &Arc<T>,
    sender: &EventSender,
    sessions: Vec<(String, PathBuf)>,
) {
    let tmux = Arc::clone(tmux);
    let sender = sender.clone();
    thread::spawn(move || {
        loop {
            if sender.cancel.load(Ordering::Relaxed) {
                return;
            }

            let states = detect_agent_states(&*tmux, &sessions);
            if !states.is_empty() {
                sender.send(AppEvent::AgentStatesUpdated { states });
            }

            // Sleep in small increments so we can check cancel promptly
            let mut remaining = AGENT_POLL_INTERVAL;
            while !remaining.is_zero() {
                if sender.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let sleep = remaining.min(std::time::Duration::from_millis(200));
                thread::sleep(sleep);
                remaining = remaining.saturating_sub(sleep);
            }
        }
    });
}

fn detect_agent_states<T: TmuxProvider + ?Sized>(
    tmux: &T,
    sessions: &[(String, PathBuf)],
) -> Vec<(String, kiosk_core::AgentState)> {
    let mut states = Vec::new();

    for (session_name, _worktree_path) in sessions {
        let panes = tmux.list_panes_detailed(session_name);

        for pane in panes {
            let mut agent_kind = agent::detect::detect_agent_kind(&pane.command, None);

            // If pane command doesn't reveal the agent, check child processes
            if agent_kind == AgentKind::Unknown {
                let child_args = get_child_process_args(pane.pid);
                if let Some(ref args) = child_args {
                    agent_kind = agent::detect::detect_agent_kind(&pane.command, Some(args));
                }
            }

            if agent_kind != AgentKind::Unknown
                && let Some(content) = tmux.capture_pane_by_index(session_name, pane.pane_index, 30)
            {
                let state = agent::detect::detect_state(&content, agent_kind);
                states.push((session_name.clone(), state));
                break; // One agent per session is enough
            }
        }
    }

    states
}

/// Get command-line arguments of child processes for a given PID.
/// Portable across Linux (incl. WSL) and macOS.
fn get_child_process_args(pid: u32) -> Option<String> {
    // Try /proc first (Linux, WSL) — children file contains space-separated child PIDs
    if let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) {
        let mut args = String::new();
        for child_pid in children.split_whitespace() {
            if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{child_pid}/cmdline")) {
                // cmdline uses null bytes as separators
                let readable = cmdline.replace('\0', " ");
                args.push_str(&readable);
                args.push('\n');
            }
        }
        if !args.is_empty() {
            return Some(args);
        }
    }

    // Fallback: use pgrep + ps (works on Linux and macOS)
    let pgrep_output = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .ok()?;

    if !pgrep_output.status.success() {
        return None;
    }

    let pgrep_str = String::from_utf8_lossy(&pgrep_output.stdout).to_string();
    let child_pids: Vec<&str> = pgrep_str.lines().filter(|s| !s.is_empty()).collect();

    if child_pids.is_empty() {
        return None;
    }

    // ps -o args= -p <pid1> -p <pid2> ... works on both Linux and macOS
    let mut ps_cmd = Command::new("ps");
    ps_cmd.args(["-o", "args="]);
    for cpid in &child_pids {
        ps_cmd.args(["-p", cpid]);
    }
    let output = ps_cmd.output().ok()?;

    if output.status.success() {
        let args = String::from_utf8_lossy(&output.stdout).to_string();
        if !args.trim().is_empty() {
            return Some(args);
        }
    }

    None
}
