use kiosk_core::{event::AppEvent, git::GitProvider, state::BranchEntry};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    thread,
};

use kiosk_core::git::Repo;
use kiosk_core::tmux::TmuxProvider;

use super::EventSender;

pub(super) fn spawn_repo_discovery(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    search_dirs: Vec<(PathBuf, u16)>,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        let repos = git.discover_repos(&search_dirs);
        sender.send(AppEvent::ReposDiscovered { repos });
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
) {
    let git = Arc::clone(git);
    let tmux = Arc::clone(tmux);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        let active_sessions = tmux.list_sessions();
        repo.worktrees = git.list_worktrees(&repo.path);
        let local_names = git.list_branches(&repo.path);
        let branches = BranchEntry::build_sorted(&repo, &local_names, &active_sessions);
        sender.send(AppEvent::BranchesLoaded {
            branches,
            worktrees: repo.worktrees,
            local_names,
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
        let remote_names = git.list_remote_branches(&repo_path);
        let branches = BranchEntry::build_remote(&remote_names, &local_names);
        if !branches.is_empty() {
            sender.send(AppEvent::RemoteBranchesLoaded { branches });
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
