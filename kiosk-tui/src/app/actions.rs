use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use kiosk_core::{
    git::GitProvider,
    pending_delete::{PendingWorktreeDelete, save_pending_worktree_deletes},
    state::{AppState, Mode, NewBranchFlow, SearchableList, worktree_dir},
    tmux::TmuxProvider,
};
use std::sync::Arc;

use super::spawn::{
    spawn_branch_and_worktree_creation, spawn_branch_loading, spawn_tracking_worktree_creation,
    spawn_worktree_creation, spawn_worktree_removal,
};
use super::{EventSender, OpenAction};

pub(super) fn handle_go_back(state: &mut AppState) {
    match state.mode.clone() {
        Mode::BranchSelect => {
            state.mode = Mode::RepoSelect;
            state.branch_list.search.clear();
            state.branch_list.cursor = 0;
        }
        Mode::NewBranchBase => {
            state.new_branch_base = None;
            state.mode = Mode::BranchSelect;
        }
        Mode::ConfirmDelete { .. } => {
            state.mode = Mode::BranchSelect;
        }
        Mode::Help { previous } => {
            state.mode = *previous;
        }
        Mode::RepoSelect | Mode::Loading(_) => {}
    }
}

pub(super) fn handle_show_help(state: &mut AppState) {
    match state.mode.clone() {
        Mode::Help { previous } => {
            state.mode = *previous;
        }
        _ => {
            state.mode = Mode::Help {
                previous: Box::new(state.mode.clone()),
            };
        }
    }
}

pub(super) fn handle_start_new_branch(state: &mut AppState, git: &Arc<dyn GitProvider>) {
    if state.branch_list.search.is_empty() {
        state.error = Some("Type a branch name first".to_string());
        return;
    }
    let Some(repo_idx) = state.selected_repo_idx else {
        return;
    };
    let repo = &state.repos[repo_idx];
    let bases = git.list_branches(&repo.path);
    let list = SearchableList::new(bases.len());

    state.new_branch_base = Some(NewBranchFlow {
        new_name: state.branch_list.search.clone(),
        bases,
        list,
    });
    state.mode = Mode::NewBranchBase;
}

pub(super) fn handle_delete_worktree(state: &mut AppState) {
    if let Some(sel) = state.branch_list.selected
        && let Some(&(idx, _)) = state.branch_list.filtered.get(sel)
    {
        let branch = &state.branches[idx];
        if let Some(repo_idx) = state.selected_repo_idx {
            let repo_path = state.repos[repo_idx].path.clone();
            if state.is_branch_pending_delete(&repo_path, &branch.name) {
                state.error = Some("Worktree deletion already in progress".to_string());
                return;
            }
        }
        if branch.worktree_path.is_none() {
            state.error = Some("No worktree to delete".to_string());
        } else if branch.is_current {
            state.error = Some("Cannot delete the current branch's worktree".to_string());
        } else {
            state.mode = Mode::ConfirmDelete {
                branch_name: branch.name.clone(),
                has_session: branch.has_session,
            };
        }
    }
}

pub(super) fn handle_confirm_delete<T: TmuxProvider + ?Sized>(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    tmux: &T,
    sender: &EventSender,
) {
    if let Mode::ConfirmDelete {
        branch_name,
        has_session,
    } = &state.mode
    {
        let branch_name = branch_name.clone();
        let has_session = *has_session;
        if let Some(branch) = state.branches.iter().find(|b| b.name == branch_name)
            && let Some(worktree_path) = &branch.worktree_path
        {
            // Kill the tmux session first if it exists
            if has_session && let Some(repo_idx) = state.selected_repo_idx {
                let repo = &state.repos[repo_idx];
                let session_name = repo.tmux_session_name(worktree_path);
                tmux.kill_session(&session_name);
            }

            let worktree_path = worktree_path.clone();
            if let Some(repo_idx) = state.selected_repo_idx {
                let repo_path = state.repos[repo_idx].path.clone();
                let pending = PendingWorktreeDelete::new(
                    repo_path,
                    branch_name.clone(),
                    worktree_path.clone(),
                );
                state.mark_pending_worktree_delete(pending);
                if let Err(e) = save_pending_worktree_deletes(&state.pending_worktree_deletes) {
                    state.error = Some(format!("Failed to persist pending deletes: {e}"));
                }
            }
            state.mode = Mode::BranchSelect;
            spawn_worktree_removal(git, sender, worktree_path, branch_name);
        }
    }
}

pub(super) fn handle_open_branch(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
) -> Option<OpenAction> {
    match state.mode {
        Mode::BranchSelect => {
            if let Some(sel) = state.branch_list.selected
                && let Some(&(idx, _)) = state.branch_list.filtered.get(sel)
            {
                let branch = &state.branches[idx];
                let repo_idx = state.selected_repo_idx?;
                let repo = &state.repos[repo_idx];

                if let Some(wt_path) = &branch.worktree_path {
                    let session_name = repo.tmux_session_name(wt_path);
                    return Some(OpenAction::Open {
                        path: wt_path.clone(),
                        session_name,
                        split_command: state.split_command.clone(),
                    });
                }
                let is_remote = branch.is_remote;
                match worktree_dir(repo, &branch.name) {
                    Ok(wt_path) => {
                        let branch_name = branch.name.clone();
                        let session_name = repo.tmux_session_name(&wt_path);
                        if is_remote {
                            state.mode = Mode::Loading(format!(
                                "Checking out remote branch {branch_name}..."
                            ));
                            spawn_tracking_worktree_creation(
                                git,
                                sender,
                                repo.path.clone(),
                                branch_name,
                                wt_path,
                                session_name,
                            );
                        } else {
                            state.mode =
                                Mode::Loading(format!("Creating worktree for {branch_name}..."));
                            spawn_worktree_creation(
                                git,
                                sender,
                                repo.path.clone(),
                                branch_name,
                                wt_path,
                                session_name,
                            );
                        }
                    }
                    Err(e) => {
                        state.error = Some(format!("Failed to determine worktree path: {e}"));
                        return None;
                    }
                }
            }
        }
        Mode::NewBranchBase => {
            if let Some(flow) = &state.new_branch_base
                && let Some(sel) = flow.list.selected
                && let Some(&(idx, _)) = flow.list.filtered.get(sel)
            {
                let base = flow.bases[idx].clone();
                let new_name = flow.new_name.clone();
                let repo_idx = state.selected_repo_idx?;
                let repo = &state.repos[repo_idx];
                match worktree_dir(repo, &new_name) {
                    Ok(wt_path) => {
                        let session_name = repo.tmux_session_name(&wt_path);
                        state.mode =
                            Mode::Loading(format!("Creating branch {new_name} from {base}..."));
                        spawn_branch_and_worktree_creation(
                            git,
                            sender,
                            repo.path.clone(),
                            new_name,
                            base,
                            wt_path,
                            session_name,
                        );
                    }
                    Err(e) => {
                        state.error = Some(format!("Failed to determine worktree path: {e}"));
                        return None;
                    }
                }
            }
        }
        Mode::RepoSelect | Mode::ConfirmDelete { .. } | Mode::Loading(_) | Mode::Help { .. } => {}
    }
    None
}

pub(super) fn enter_branch_select<T: TmuxProvider + ?Sized + 'static>(
    state: &mut AppState,
    repo_idx: usize,
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
) {
    enter_branch_select_with_loading(state, repo_idx, git, tmux, sender, true);
}

pub(super) fn enter_branch_select_with_loading<T: TmuxProvider + ?Sized + 'static>(
    state: &mut AppState,
    repo_idx: usize,
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
    show_loading: bool,
) {
    state.selected_repo_idx = Some(repo_idx);
    let repo = state.repos[repo_idx].clone();
    if show_loading {
        state.mode = Mode::BranchSelect;
        state.branches.clear();
        state.branch_list.reset(0);
    }
    state.loading_branches = true;
    spawn_branch_loading(git, tmux, sender, repo);
}

pub(super) fn handle_search_push(state: &mut AppState, matcher: &SkimMatcherV2, c: char) {
    if let Some(list) = state.active_list_mut() {
        list.insert_char(c);
    }
    update_active_filter(state, matcher);
}

pub(super) fn handle_search_pop(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.backspace();
    }
    update_active_filter(state, matcher);
}

pub(super) fn handle_search_delete_forward(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.delete_forward();
    }
    update_active_filter(state, matcher);
}

pub(super) fn handle_search_delete_word(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.delete_word();
    }
    update_active_filter(state, matcher);
}

pub(super) fn handle_search_delete_word_forward(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.delete_word_forward();
    }
    update_active_filter(state, matcher);
}

pub(super) fn handle_search_delete_to_start(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.delete_to_start();
    }
    update_active_filter(state, matcher);
}

pub(super) fn handle_search_delete_to_end(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.delete_to_end();
    }
    update_active_filter(state, matcher);
}

fn update_active_filter(state: &mut AppState, matcher: &SkimMatcherV2) {
    match state.mode {
        Mode::RepoSelect => {
            let names: Vec<String> = state.repos.iter().map(|r| r.name.clone()).collect();
            apply_fuzzy_filter(&mut state.repo_list, &names, matcher);
        }
        Mode::BranchSelect => {
            let names: Vec<String> = state.branches.iter().map(|b| b.name.clone()).collect();
            apply_fuzzy_filter(&mut state.branch_list, &names, matcher);
        }
        Mode::NewBranchBase => {
            if let Some(flow) = &mut state.new_branch_base {
                let bases = flow.bases.clone();
                apply_fuzzy_filter(&mut flow.list, &bases, matcher);
            }
        }
        _ => {}
    }
}

fn apply_fuzzy_filter(list: &mut SearchableList, items: &[String], matcher: &SkimMatcherV2) {
    if list.search.is_empty() {
        list.filtered = items.iter().enumerate().map(|(i, _)| (i, 0)).collect();
    } else {
        let mut scored: Vec<(usize, i64)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                matcher
                    .fuzzy_match(item, &list.search)
                    .map(|score| (i, score))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        list.filtered = scored;
    }
    list.selected = if list.filtered.is_empty() {
        None
    } else {
        Some(0)
    };
}
