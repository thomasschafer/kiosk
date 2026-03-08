use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use kiosk_core::{
    config::KeysConfig,
    git::GitProvider,
    pending_delete::{PendingWorktreeDelete, save_pending_worktree_deletes},
    state::{
        AppState, BaseBranchSelection, HelpOverlayState, Mode, ModeTransition, SearchableList,
        SessionEntry, SetupStep, worktree_dir,
    },
    tmux::TmuxProvider,
};
use std::sync::Arc;

use super::spawn::{
    spawn_branch_and_worktree_creation, spawn_branch_loading, spawn_tracking_worktree_creation,
    spawn_worktree_creation, spawn_worktree_removal,
};
use super::{EventSender, OpenAction};

pub(super) fn session_search_items(sessions: &[SessionEntry]) -> Vec<String> {
    sessions
        .iter()
        .map(|session| {
            let branch = session.branch.as_deref().unwrap_or("");
            format!("{} {} {}", session.session_name, session.repo_name, branch)
        })
        .collect()
}

fn fuzzy_matches_in_source_order(
    query: &str,
    items: &[String],
    matcher: &SkimMatcherV2,
) -> Vec<(usize, i64)> {
    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| matcher.fuzzy_match(item, query).map(|score| (i, score)))
        .collect()
}

pub(super) fn rebuild_session_filter_preserving_search(
    list: &mut SearchableList,
    sessions: &[SessionEntry],
) {
    if list.input.text.is_empty() {
        list.filtered = (0..sessions.len()).map(|i| (i, 0)).collect();
    } else {
        let matcher = SkimMatcherV2::default();
        let items = session_search_items(sessions);
        list.filtered = fuzzy_matches_in_source_order(&list.input.text, &items, &matcher);
    }
}

fn apply_session_filter(
    list: &mut SearchableList,
    sessions: &[SessionEntry],
    matcher: &SkimMatcherV2,
) {
    if list.input.text.is_empty() {
        list.filtered = (0..sessions.len()).map(|i| (i, 0)).collect();
    } else {
        let items = session_search_items(sessions);
        list.filtered = fuzzy_matches_in_source_order(&list.input.text, &items, matcher);
    }
    list.selected = if list.filtered.is_empty() {
        None
    } else {
        Some(0)
    };
    list.scroll_offset = 0;
}

pub(super) fn handle_go_back(state: &mut AppState) {
    match state.mode().clone() {
        Mode::BranchSelect => {
            state.apply_transition(&ModeTransition::RepoSelect);
            state.branch_list.input.clear();
        }
        Mode::SelectBaseBranch => {
            state.clear_base_branch_selection();
            state.apply_transition(&ModeTransition::BranchSelect);
        }
        Mode::ConfirmWorktreeDelete { .. } => {
            state.apply_transition(&ModeTransition::BranchSelect);
        }
        Mode::Help { previous } => {
            state.clear_help_overlay();
            let _ = previous; // explicitly ignored; CloseHelp derives target from current mode
            state.apply_transition(&ModeTransition::CloseHelp);
        }
        Mode::Sessions => {
            state.apply_transition(&ModeTransition::RepoSelect);
        }
        Mode::Setup(_) | Mode::RepoSelect | Mode::Loading(_) => {}
    }
}

pub(super) fn handle_show_help(state: &mut AppState, keys: &KeysConfig) {
    if let Mode::Help { previous } = state.mode().clone() {
        let _ = previous; // explicitly ignored; CloseHelp derives target from current mode
        state.apply_transition(&ModeTransition::CloseHelp);
    } else {
        let catalog = keys.catalog_for_mode(state.mode());
        state.apply_transition(&ModeTransition::OpenHelp {
            overlay: HelpOverlayState {
                list: SearchableList::new(catalog.flattened.len()),
                rows: catalog.flattened,
            },
        });
    }
}

pub(super) fn handle_start_new_branch(state: &mut AppState) {
    if state.branch_list.input.text.is_empty() {
        state.set_error("Type a branch name first");
        return;
    }
    if state.selected_repo_idx.is_none() {
        return;
    }
    // Derive base branches from the already-loaded branch list, preserving its ordering
    // and filtering out remote-only branches (which can't serve as local bases).
    let bases: Vec<String> = state
        .branches
        .iter()
        .filter(|b| b.remote.is_none())
        .map(|b| b.name.clone())
        .collect();
    if bases.is_empty() {
        state.set_error("No local branches to use as base");
        return;
    }
    let list = SearchableList::new(bases.len());

    state.apply_transition(&ModeTransition::SelectBaseBranch {
        flow: BaseBranchSelection {
            new_name: state.branch_list.input.text.clone(),
            bases,
            list,
        },
    });
}

pub(super) fn handle_delete_worktree(state: &mut AppState) {
    if let Some(sel) = state.branch_list.selected
        && let Some(&(idx, _)) = state.branch_list.filtered.get(sel)
    {
        let branch = &state.branches[idx];
        if let Some(repo_idx) = state.selected_repo_idx {
            let repo_path = state.repos[repo_idx].path.clone();
            if state.is_branch_pending_delete(&repo_path, &branch.name) {
                state.set_error("Worktree deletion already in progress");
                return;
            }
        }
        if branch.worktree_path.is_none() {
            state.set_error("No worktree to delete");
        } else if branch.is_current {
            state.set_error("Cannot delete the current branch's worktree");
        } else {
            state.apply_transition(&ModeTransition::ConfirmWorktreeDelete {
                branch_name: branch.name.clone(),
                has_session: branch.has_session,
            });
        }
    }
}

pub(super) fn handle_confirm_delete<T: TmuxProvider + ?Sized>(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    tmux: &T,
    sender: &EventSender,
) {
    if let Mode::ConfirmWorktreeDelete {
        branch_name,
        has_session,
    } = state.mode()
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
                    state.set_error(&format!("Failed to persist pending deletes: {e}"));
                }
            }
            state.apply_transition(&ModeTransition::BranchSelect);
            spawn_worktree_removal(git, sender, worktree_path, branch_name);
        }
    }
}

pub(super) fn handle_open_branch(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
) -> Option<OpenAction> {
    match state.mode() {
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
                        path: Some(wt_path.clone()),
                        session_name,
                        split_command: state.split_command.clone(),
                    });
                }
                let is_remote = branch.remote.is_some();
                match worktree_dir(repo, &branch.name) {
                    Ok(wt_path) => {
                        let branch_name = branch.name.clone();
                        let repo_path = repo.path.clone();
                        let session_name = repo.tmux_session_name(&wt_path);
                        if is_remote {
                            state.apply_transition(&ModeTransition::Loading {
                                message: format!("Checking out remote branch {branch_name}..."),
                            });
                            spawn_tracking_worktree_creation(
                                git,
                                sender,
                                repo_path,
                                branch_name,
                                wt_path,
                                session_name,
                            );
                        } else {
                            state.apply_transition(&ModeTransition::Loading {
                                message: format!("Creating worktree for {branch_name}..."),
                            });
                            spawn_worktree_creation(
                                git,
                                sender,
                                repo_path,
                                branch_name,
                                wt_path,
                                session_name,
                            );
                        }
                    }
                    Err(e) => {
                        state.set_error(&format!("Failed to determine worktree path: {e}"));
                        return None;
                    }
                }
            }
        }
        Mode::SelectBaseBranch => {
            if let Some(flow) = state.base_branch_selection()
                && let Some(sel) = flow.list.selected
                && let Some(&(idx, _)) = flow.list.filtered.get(sel)
            {
                let base = flow.bases[idx].clone();
                let new_name = flow.new_name.clone();
                let repo_idx = state.selected_repo_idx?;
                let repo = &state.repos[repo_idx];
                match worktree_dir(repo, &new_name) {
                    Ok(wt_path) => {
                        let repo_path = repo.path.clone();
                        let session_name = repo.tmux_session_name(&wt_path);
                        state.apply_transition(&ModeTransition::Loading {
                            message: format!("Creating branch {new_name} from {base}..."),
                        });
                        spawn_branch_and_worktree_creation(
                            git,
                            sender,
                            repo_path,
                            new_name,
                            base,
                            wt_path,
                            session_name,
                        );
                    }
                    Err(e) => {
                        state.set_error(&format!("Failed to determine worktree path: {e}"));
                        return None;
                    }
                }
            }
        }
        Mode::RepoSelect
        | Mode::Sessions
        | Mode::ConfirmWorktreeDelete { .. }
        | Mode::Loading(_)
        | Mode::Help { .. }
        | Mode::Setup(_) => {}
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
    let cwd = state.cwd_worktree_path.clone();
    if show_loading {
        state.apply_transition(&ModeTransition::BranchSelect);
        state.branches.clear();
        state.branch_list.reset(0);
    }
    state.loading_branches = true;
    state.fetching_remotes = false;
    spawn_branch_loading(git, tmux, sender, repo, cwd);
}

pub(super) fn handle_search_push(state: &mut AppState, matcher: &SkimMatcherV2, c: char) {
    if let Some(input) = state.active_text_input() {
        input.insert_char(c);
    }
    post_text_edit(state, matcher);
}

pub(super) fn handle_search_pop(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(input) = state.active_text_input() {
        input.backspace();
    }
    post_text_edit(state, matcher);
}

pub(super) fn handle_search_delete_word(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(input) = state.active_text_input() {
        input.delete_word();
    }
    post_text_edit(state, matcher);
}

pub(super) fn handle_search_delete_forward(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(input) = state.active_text_input() {
        input.delete_forward_char();
    }
    post_text_edit(state, matcher);
}

pub(super) fn handle_search_delete_word_forward(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(input) = state.active_text_input() {
        input.delete_word_forward();
    }
    post_text_edit(state, matcher);
}

pub(super) fn handle_search_delete_to_start(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(input) = state.active_text_input() {
        input.delete_to_start();
    }
    post_text_edit(state, matcher);
}

pub(super) fn handle_search_delete_to_end(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(input) = state.active_text_input() {
        input.delete_to_end();
    }
    post_text_edit(state, matcher);
}

/// Dispatch post-edit updates: setup completions or fuzzy filter.
fn post_text_edit(state: &mut AppState, matcher: &SkimMatcherV2) {
    if matches!(state.mode(), Mode::Setup(SetupStep::SearchDirs)) {
        update_setup_completions(state);
    } else {
        update_active_filter(state, matcher);
    }
}

// ── Setup action handlers ──

pub(super) fn handle_setup_continue(state: &mut AppState) {
    let setup = state
        .setup()
        .cloned()
        .unwrap_or_else(kiosk_core::state::SetupState::new);
    state.apply_transition(&ModeTransition::Setup {
        step: SetupStep::SearchDirs,
        setup,
    });
}

pub(super) fn handle_setup_add_dir(state: &mut AppState) -> Option<super::OpenAction> {
    let setup = state.setup()?;

    // If a completion is selected, navigate into it instead of adding
    if let Some(sel) = setup.selected_completion {
        fill_setup_completion(state, sel);
        return None;
    }

    let input_text = setup.input.text.trim().to_string();
    if input_text.is_empty() {
        if setup.dirs.is_empty() {
            state.set_error("Add at least one directory");
            return None;
        }
        return Some(super::OpenAction::SetupComplete);
    }

    let setup = state.setup_mut()?;
    if !setup.dirs.contains(&input_text) {
        setup.dirs.push(input_text);
    }
    setup.input.clear();
    setup.completions.clear();
    setup.selected_completion = None;
    None
}

/// Fill the selected (or first) completion into the input and re-generate.
/// Shared by Tab-complete and Enter-with-selection flows.
fn fill_setup_completion(state: &mut AppState, index: usize) {
    let Some(setup) = state.setup_mut() else {
        return;
    };
    let Some(completion) = setup.completions.get(index).cloned() else {
        return;
    };
    let with_slash = if completion.ends_with('/') {
        completion
    } else {
        format!("{completion}/")
    };
    setup.input.text = with_slash;
    setup.input.cursor = setup.input.text.len();
    setup.completions.clear();
    setup.selected_completion = None;
    update_setup_completions(state);
}

pub(super) fn handle_setup_tab_complete(state: &mut AppState) {
    let Some(setup) = state.setup_mut() else {
        return;
    };

    // Generate completions if not already present
    if setup.completions.is_empty() {
        setup.completions = crate::components::path_input::complete(&setup.input.text);
        setup.selected_completion = None;
        return;
    }

    if setup.completions.len() == 1 {
        fill_setup_completion(state, 0);
        return;
    }

    // Multiple: fill to common prefix if it extends the input
    let common = crate::components::path_input::common_prefix(&setup.completions);
    if !common.is_empty() && common != setup.input.text {
        setup.input.text = common;
        setup.input.cursor = setup.input.text.len();
        setup.completions = crate::components::path_input::complete(&setup.input.text);
        setup.selected_completion = None;
        return;
    }

    // Common prefix already matches input: fill in the selected (or first) completion
    let sel = setup.selected_completion.unwrap_or(0);
    fill_setup_completion(state, sel);
}

pub(super) fn handle_setup_move_selection(state: &mut AppState, delta: i32) {
    let Some(setup) = state.setup_mut() else {
        return;
    };
    if setup.completions.is_empty() {
        return;
    }
    let len = setup.completions.len();
    setup.selected_completion = match setup.selected_completion {
        None => {
            if delta > 0 {
                Some(0)
            } else {
                Some(len - 1)
            }
        }
        Some(current) => {
            if delta > 0 {
                let next = current.saturating_add(delta.unsigned_abs() as usize);
                if next >= len { None } else { Some(next) }
            } else if current == 0 {
                None
            } else {
                Some(current.saturating_sub(delta.unsigned_abs() as usize))
            }
        }
    };
}

/// Cancel in setup: deselect completion if one is highlighted, otherwise quit.
pub(super) fn handle_setup_cancel(state: &mut AppState) -> Option<super::OpenAction> {
    let Some(setup) = state.setup_mut() else {
        return Some(super::OpenAction::Quit);
    };
    if setup.selected_completion.is_some() {
        setup.selected_completion = None;
        None
    } else {
        Some(super::OpenAction::Quit)
    }
}

fn update_setup_completions(state: &mut AppState) {
    let Some(setup) = state.setup_mut() else {
        return;
    };
    setup.completions = crate::components::path_input::complete(&setup.input.text);
    setup.selected_completion = None;
}

fn update_active_filter(state: &mut AppState, matcher: &SkimMatcherV2) {
    match state.mode() {
        Mode::RepoSelect => {
            let names: Vec<String> = state.repos.iter().map(|r| r.name.clone()).collect();
            apply_fuzzy_filter(&mut state.repo_list, &names, matcher);
        }
        Mode::BranchSelect => {
            let names: Vec<String> = state.branches.iter().map(|b| b.name.clone()).collect();
            apply_fuzzy_filter(&mut state.branch_list, &names, matcher);
        }
        Mode::Sessions => {
            apply_session_filter(
                &mut state.sessions_view.list,
                &state.sessions_view.sessions,
                matcher,
            );
        }
        Mode::SelectBaseBranch => {
            if let Some(flow) = state.base_branch_selection_mut() {
                let bases = flow.bases.clone();
                apply_fuzzy_filter(&mut flow.list, &bases, matcher);
            }
        }
        Mode::Help { .. } => {
            if let Some(overlay) = state.help_overlay_mut() {
                let search_items: Vec<String> = overlay
                    .rows
                    .iter()
                    .map(|row| {
                        format!(
                            "{} {} {} {}",
                            row.section_name, row.key_display, row.command, row.description
                        )
                    })
                    .collect();
                apply_fuzzy_filter(&mut overlay.list, &search_items, matcher);
                // Stable-sort filtered results by section_index so that
                // compute_help_layout never emits duplicate section headers
                // when fuzzy scoring reorders items across sections.
                overlay.list.filtered.sort_by_key(|(row_idx, _score)| {
                    overlay.rows.get(*row_idx).map_or(0, |r| r.section_index)
                });
            }
        }
        _ => {}
    }
}

fn apply_fuzzy_filter(list: &mut SearchableList, items: &[String], matcher: &SkimMatcherV2) {
    if list.input.text.is_empty() {
        list.filtered = items.iter().enumerate().map(|(i, _)| (i, 0)).collect();
    } else {
        let mut scored: Vec<(usize, i64)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                matcher
                    .fuzzy_match(item, &list.input.text)
                    .map(|score| (i, score))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| items[a.0].len().cmp(&items[b.0].len()))
                .then_with(|| items[a.0].cmp(&items[b.0]))
        });
        list.filtered = scored;
    }
    list.selected = if list.filtered.is_empty() {
        None
    } else {
        Some(0)
    };
    list.scroll_offset = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_list(search: &str) -> SearchableList {
        SearchableList {
            input: kiosk_core::state::TextInput {
                text: search.to_string(),
                cursor: search.len(),
            },
            filtered: Vec::new(),
            selected: None,
            scroll_offset: 0,
        }
    }

    fn filtered_names<'a>(list: &SearchableList, items: &'a [String]) -> Vec<&'a str> {
        list.filtered
            .iter()
            .map(|(i, _)| items[*i].as_str())
            .collect()
    }

    #[test]
    fn empty_search_preserves_original_order() {
        let items: Vec<String> = vec!["zebra", "apple", "mango"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut list = make_list("");
        let matcher = SkimMatcherV2::default();

        apply_fuzzy_filter(&mut list, &items, &matcher);

        assert_eq!(
            filtered_names(&list, &items),
            vec!["zebra", "apple", "mango"]
        );
        assert_eq!(list.selected, Some(0));
    }

    #[test]
    fn equal_scores_sorted_by_length_then_alphabetically() {
        // All items start with "cli" so the match occurs at the same position,
        // giving equal fuzzy scores — tiebreakers should apply.
        let items: Vec<String> = vec!["cli-extension-dep-graph", "cli-tools", "cli", "cli-abc"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut list = make_list("cli");
        let matcher = SkimMatcherV2::default();

        apply_fuzzy_filter(&mut list, &items, &matcher);

        let names = filtered_names(&list, &items);
        assert_eq!(names[0], "cli", "shortest match should be first");
        assert_eq!(names[1], "cli-abc");
        assert_eq!(names[2], "cli-tools");
        assert_eq!(names[3], "cli-extension-dep-graph");
    }

    #[test]
    fn higher_score_wins_over_length() {
        let items: Vec<String> = vec!["x-main-utils", "main"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut list = make_list("main");
        let matcher = SkimMatcherV2::default();

        apply_fuzzy_filter(&mut list, &items, &matcher);

        let names = filtered_names(&list, &items);
        // Both should match; "main" is shorter so should be first if scores are equal,
        // or first regardless if its score is higher
        assert_eq!(names[0], "main");
        assert_eq!(names[1], "x-main-utils");
    }

    #[test]
    fn no_matches_gives_empty_filtered_and_none_selected() {
        let items: Vec<String> = vec!["alpha", "beta"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut list = make_list("zzzzz");
        let matcher = SkimMatcherV2::default();

        apply_fuzzy_filter(&mut list, &items, &matcher);

        assert!(list.filtered.is_empty());
        assert_eq!(list.selected, None);
    }

    #[test]
    fn alphabetical_tiebreak_when_same_score_and_length() {
        let items: Vec<String> = vec!["bfoo", "afoo", "cfoo"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut list = make_list("foo");
        let matcher = SkimMatcherV2::default();

        apply_fuzzy_filter(&mut list, &items, &matcher);

        let names = filtered_names(&list, &items);
        assert_eq!(names, vec!["afoo", "bfoo", "cfoo"]);
    }

    #[test]
    fn sessions_mode_search_filters_session_list() {
        let mut state = AppState::new(Vec::new(), None);
        state.apply_transition(&ModeTransition::Sessions);
        state.sessions_view.sessions = vec![
            SessionEntry {
                session_name: "scooter--add-scoop-to-readme".to_string(),
                repo_name: "scooter".to_string(),
                branch: Some("add-scoop-to-readme".to_string()),
                path: PathBuf::from("/tmp/scooter-add-scoop"),
                agent_statuses: Vec::new(),
                session_activity: 10,
                attached: false,
            },
            SessionEntry {
                session_name: "scooter--main".to_string(),
                repo_name: "scooter".to_string(),
                branch: Some("main".to_string()),
                path: PathBuf::from("/tmp/scooter-main"),
                agent_statuses: Vec::new(),
                session_activity: 20,
                attached: false,
            },
            SessionEntry {
                session_name: "kiosk--feat-agent-session-switcher".to_string(),
                repo_name: "kiosk".to_string(),
                branch: Some("feat-agent-session-switcher".to_string()),
                path: PathBuf::from("/tmp/kiosk-feat"),
                agent_statuses: Vec::new(),
                session_activity: 30,
                attached: false,
            },
        ];
        state.sessions_view.list = SearchableList::new(state.sessions_view.sessions.len());

        let matcher = SkimMatcherV2::default();
        for c in "add-scoop".chars() {
            handle_search_push(&mut state, &matcher, c);
        }

        assert_eq!(state.sessions_view.list.filtered.len(), 1);
        assert_eq!(state.sessions_view.list.filtered[0].0, 0);
        assert_eq!(state.sessions_view.list.selected, Some(0));
    }

    #[test]
    fn sessions_mode_search_preserves_canonical_session_order() {
        let mut state = AppState::new(Vec::new(), None);
        state.apply_transition(&ModeTransition::Sessions);
        state.sessions_view.sessions = vec![
            SessionEntry {
                session_name: "kiosk--feat-agent-session-switcher".to_string(),
                repo_name: "kiosk".to_string(),
                branch: Some("feat-agent-session-switcher".to_string()),
                path: PathBuf::from("/tmp/kiosk-feat"),
                agent_statuses: Vec::new(),
                session_activity: 30,
                attached: true,
            },
            SessionEntry {
                session_name: "scooter--main".to_string(),
                repo_name: "scooter".to_string(),
                branch: Some("main".to_string()),
                path: PathBuf::from("/tmp/scooter-main"),
                agent_statuses: Vec::new(),
                session_activity: 20,
                attached: false,
            },
            SessionEntry {
                session_name: "dotfiles--main".to_string(),
                repo_name: "dotfiles".to_string(),
                branch: Some("main".to_string()),
                path: PathBuf::from("/tmp/dotfiles-main"),
                agent_statuses: Vec::new(),
                session_activity: 10,
                attached: false,
            },
        ];
        state.sessions_view.list = SearchableList::new(state.sessions_view.sessions.len());

        let matcher = SkimMatcherV2::default();
        handle_search_push(&mut state, &matcher, 'o');

        let filtered_names = state
            .sessions_view
            .list
            .filtered
            .iter()
            .map(|(idx, _)| state.sessions_view.sessions[*idx].session_name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            filtered_names,
            vec![
                "kiosk--feat-agent-session-switcher",
                "scooter--main",
                "dotfiles--main",
            ]
        );
        assert_eq!(state.sessions_view.list.selected, Some(0));
    }
}
