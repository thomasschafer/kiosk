mod actions;
mod spawn;

use crate::{components, keymap};
use actions::{
    enter_branch_select, enter_branch_select_with_loading, handle_confirm_delete,
    handle_delete_worktree, handle_go_back, handle_open_branch, handle_search_delete_forward,
    handle_search_delete_to_end, handle_search_delete_to_start, handle_search_delete_word,
    handle_search_delete_word_forward, handle_search_pop, handle_search_push, handle_setup_add_dir,
    handle_setup_cancel, handle_setup_continue, handle_setup_move_selection,
    handle_setup_tab_complete, handle_show_help, handle_start_new_branch,
};
use crossterm::event::{self, Event, KeyEventKind};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use kiosk_core::{
    action::Action,
    config::{KeysConfig, keys::Command},
    event::AppEvent,
    git::GitProvider,
    pending_delete::save_pending_worktree_deletes,
    state::{AppState, BranchEntry, Mode, SearchableList},
    tmux::TmuxProvider,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Padding, Paragraph},
};
use spawn::spawn_repo_discovery;
use std::{
    fmt::Write as _,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// What to do after the TUI exits
pub enum OpenAction {
    Open {
        path: PathBuf,
        session_name: String,
        split_command: Option<String>,
    },
    /// Setup wizard completed — dirs are stored in `AppState.setup`
    SetupComplete,
    Quit,
}

/// Handle for dispatching background work
#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<AppEvent>,
    cancel: Arc<AtomicBool>,
}

impl EventSender {
    /// Send an event from a background thread to the main loop
    pub fn send(&self, event: AppEvent) {
        let _ = self.tx.send(event);
    }
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn run(
    terminal: &mut DefaultTerminal,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<dyn TmuxProvider>,
    theme: &crate::theme::Theme,
    keys: &kiosk_core::config::KeysConfig,
    search_dirs: Vec<(std::path::PathBuf, u16)>,
) -> anyhow::Result<Option<OpenAction>> {
    let matcher = SkimMatcherV2::default();
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let event_sender = EventSender {
        tx,
        cancel: Arc::clone(&cancel),
    };
    let spinner_start = Instant::now();

    // Start repo discovery in background
    if state.loading_repos || state.repos.is_empty() {
        state.loading_repos = true;
        state.seen_repo_paths.clear();
        spawn_repo_discovery(git, tmux, &event_sender, search_dirs);
    }

    loop {
        terminal.draw(|f| draw(f, state, theme, keys, &spinner_start))?;

        // Auto-dismiss error after 5 seconds
        if let Some(set_at) = state.error_set_at
            && set_at.elapsed() >= Duration::from_secs(5)
        {
            state.clear_error();
        }

        // Check background channel (non-blocking)
        if let Ok(app_event) = rx.try_recv() {
            if let Some(result) = process_app_event(app_event, state, git, tmux, &event_sender) {
                return Ok(Some(result));
            }
            continue;
        }

        // Poll terminal events with a timeout so we can update spinner + check channel
        if event::poll(Duration::from_millis(80))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // In loading mode, only allow Ctrl+C
            if matches!(state.mode, Mode::Loading(_)) {
                if key.code == crossterm::event::KeyCode::Char('c')
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    // Signal cancellation to background threads
                    cancel.store(true, Ordering::Relaxed);
                    return Ok(Some(OpenAction::Quit));
                }
                continue;
            }

            // Clear error on any keypress
            state.clear_error();

            let ctx = ActionContext {
                git,
                tmux,
                keys,
                matcher: &matcher,
                sender: &event_sender,
            };
            if let Some(action) = keymap::resolve_action(key, state, keys)
                && let Some(result) = process_action(action, state, &ctx)
            {
                return Ok(Some(result));
            }
        }
    }
}

fn draw(
    f: &mut Frame,
    state: &mut AppState,
    theme: &crate::theme::Theme,
    keys: &kiosk_core::config::KeysConfig,
    spinner_start: &Instant,
) {
    // Loading mode: full-screen spinner
    if let Mode::Loading(ref msg) = state.mode {
        draw_loading(f, f.area(), msg, theme, spinner_start);
        return;
    }

    let outer = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(f.area());

    let main_area = outer[0];
    let footer_area = outer[1];

    let page_rows = active_list_page_rows(f.area(), main_area, &state.mode);
    state.set_active_list_page_rows(page_rows);

    // Determine the effective mode for footer hints
    let effective_mode = match &state.mode {
        Mode::Help { previous } => previous.as_ref(),
        other => other,
    };

    match &state.mode {
        Mode::RepoSelect => components::repo_list::draw(f, main_area, state, theme, keys),
        Mode::BranchSelect => components::branch_picker::draw(f, main_area, state, theme, keys),
        Mode::SelectBaseBranch => {
            components::branch_picker::draw(f, main_area, state, theme, keys);
            components::new_branch::draw(f, state, theme);
        }
        Mode::ConfirmWorktreeDelete { .. } => {
            components::branch_picker::draw(f, main_area, state, theme, keys);
            draw_confirm_delete_dialog(f, main_area, state, theme, keys);
        }
        Mode::Setup(_) => {
            components::setup::draw(f, state, theme);
        }
        Mode::Help { previous } => {
            // Draw the previous mode as background
            match previous.as_ref() {
                Mode::RepoSelect => {
                    components::repo_list::draw(f, main_area, state, theme, keys);
                }
                Mode::BranchSelect => {
                    components::branch_picker::draw(f, main_area, state, theme, keys);
                }
                Mode::SelectBaseBranch => {
                    components::branch_picker::draw(f, main_area, state, theme, keys);
                    components::new_branch::draw(f, state, theme);
                }
                Mode::ConfirmWorktreeDelete { .. } => {
                    components::branch_picker::draw(f, main_area, state, theme, keys);
                    draw_confirm_delete_dialog(f, main_area, state, theme, keys);
                }
                Mode::Setup(_) => {
                    components::setup::draw(f, state, theme);
                }
                // Loading is handled by the early-return guard; Help cannot nest.
                Mode::Loading(_) | Mode::Help { .. } => {}
            }
            // Draw help overlay on top
            components::help::draw(f, state, theme);
        }
        Mode::Loading(_) => unreachable!(),
    }

    // Error toast overlay (rendered on top of everything)
    components::error_toast::draw(f, f.area(), state, theme);

    // Footer with key hints
    let footer_hints = build_footer_hints(effective_mode, keys);
    let footer = Paragraph::new(Line::from(
        footer_hints
            .into_iter()
            .enumerate()
            .flat_map(|(i, (key, desc))| {
                let mut spans = Vec::new();
                if i > 0 {
                    spans.push(Span::styled(" │ ", Style::default().fg(theme.border)));
                }
                spans.push(Span::styled(
                    key,
                    Style::default().fg(theme.hint).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(format!(": {desc}")));
                spans
            })
            .collect::<Vec<_>>(),
    ))
    .alignment(Alignment::Center);
    f.render_widget(footer, footer_area);
}

fn build_footer_hints(mode: &Mode, keys: &KeysConfig) -> Vec<(String, &'static str)> {
    let keymap = keys.keymap_for_mode(mode);
    mode.footer_commands()
        .iter()
        .filter_map(|cmd| {
            let key = KeysConfig::find_key(&keymap, cmd)?;
            Some((key.to_string(), cmd.labels().hint))
        })
        .collect()
}

fn list_rows_from_list_area(list_area: Rect) -> usize {
    usize::from(list_area.height.saturating_sub(2)).max(1)
}

fn active_list_page_rows(full_area: Rect, main_area: Rect, mode: &Mode) -> usize {
    match mode {
        Mode::RepoSelect | Mode::BranchSelect | Mode::ConfirmWorktreeDelete { .. } => {
            let chunks =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(main_area);
            list_rows_from_list_area(chunks[1])
        }
        Mode::SelectBaseBranch => {
            let popup = components::centered_rect(60, 60, full_area);
            let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(popup);
            list_rows_from_list_area(chunks[1])
        }
        Mode::Help { .. } => {
            let popup = components::centered_rect(80, 85, full_area);
            let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(popup);
            list_rows_from_list_area(chunks[1])
        }
        Mode::Setup(_) | Mode::Loading(_) => 1,
    }
}

fn build_loading_dialog<'a>(
    spinner_prefix: String,
    message: &str,
    accent_color: Color,
) -> components::dialog::Dialog<'a> {
    let text = Line::from(vec![
        Span::styled(
            spinner_prefix,
            Style::default()
                .fg(accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(message.to_string()),
    ]);

    components::dialog::Dialog::new(vec![text])
        .border_color(accent_color)
        .padding(Padding::horizontal(1))
        .alignment(Alignment::Center)
}

fn draw_loading(
    f: &mut Frame,
    area: Rect,
    message: &str,
    theme: &crate::theme::Theme,
    start: &Instant,
) {
    let elapsed = start.elapsed().as_millis() as usize;
    let frame_idx = (elapsed / 80) % SPINNER_FRAMES.len();
    let spinner = SPINNER_FRAMES[frame_idx];

    build_loading_dialog(format!("{spinner} "), message, theme.accent).render(f, area);
}

fn build_confirm_delete_dialog<'a>(
    branch_name: &str,
    has_session: bool,
    confirm_key: &str,
    cancel_key: &str,
    accent_color: Color,
    hint_color: Color,
) -> components::dialog::Dialog<'a> {
    let action_text = if has_session {
        "Delete worktree and kill tmux session for branch "
    } else {
        "Delete worktree for branch "
    };

    let message_line = Line::from(vec![
        Span::raw(action_text),
        Span::styled(
            format!("\"{branch_name}\""),
            Style::default()
                .fg(accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("?"),
    ]);

    let blank_line = Line::raw("");

    let hints_line = Line::from(vec![
        Span::raw("confirm ("),
        Span::styled(
            confirm_key.to_string(),
            Style::default().fg(hint_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(")"),
        Span::raw(" / "),
        Span::raw("cancel ("),
        Span::styled(
            cancel_key.to_string(),
            Style::default().fg(hint_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(")"),
    ]);

    components::dialog::Dialog::new(vec![message_line, blank_line, hints_line])
        .border_color(accent_color)
        .title(" Confirm delete ")
        .padding(Padding::uniform(1))
        .alignment(Alignment::Center)
}

fn draw_confirm_delete_dialog(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &crate::theme::Theme,
    keys: &kiosk_core::config::KeysConfig,
) {
    if let Mode::ConfirmWorktreeDelete {
        branch_name,
        has_session,
    } = &state.mode
    {
        let keymap = keys.keymap_for_mode(&Mode::ConfirmWorktreeDelete {
            branch_name: branch_name.clone(),
            has_session: *has_session,
        });
        let confirm_key = KeysConfig::find_key(&keymap, &Command::Confirm)
            .map_or("enter".to_string(), |k| k.to_string());
        let cancel_key = KeysConfig::find_key(&keymap, &Command::Cancel)
            .map_or("esc".to_string(), |k| k.to_string());

        build_confirm_delete_dialog(
            branch_name,
            *has_session,
            &confirm_key,
            &cancel_key,
            theme.accent,
            theme.hint,
        )
        .render(f, area);
    }
}

/// Deduplicate `incoming` branches against `state.branches`, append any new ones,
/// and rebuild the filtered list preserving search.
fn extend_branches_deduped(state: &mut AppState, incoming: Vec<BranchEntry>) {
    if incoming.is_empty() {
        return;
    }
    let existing_names: std::collections::HashSet<&str> =
        state.branches.iter().map(|b| b.name.as_str()).collect();
    let new_branches: Vec<_> = incoming
        .into_iter()
        .filter(|b| !existing_names.contains(b.name.as_str()))
        .collect();
    if !new_branches.is_empty() {
        state.branches.extend(new_branches);
        let names: Vec<&str> = state.branches.iter().map(|b| b.name.as_str()).collect();
        rebuild_filtered_preserving_search(&mut state.branch_list, &names);
    }
}

/// Rebuild a `SearchableList`'s filtered entries from new item names while preserving
/// the current search text, cursor position, and selection (clamped to bounds).
fn rebuild_filtered_preserving_search(list: &mut SearchableList, names: &[&str]) {
    if list.input.text.is_empty() {
        list.filtered = (0..names.len()).map(|i| (i, 0)).collect();
    } else {
        let matcher = SkimMatcherV2::default();
        let mut scored: Vec<(usize, i64)> = names
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                matcher
                    .fuzzy_match(name, &list.input.text)
                    .map(|score| (i, score))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        list.filtered = scored;
    }
    if let Some(sel) = list.selected {
        if sel >= list.filtered.len() {
            list.selected = if list.filtered.is_empty() {
                None
            } else {
                Some(0)
            };
        }
    } else if !list.filtered.is_empty() {
        list.selected = Some(0);
    }
}

/// Sort repos by activity/name, remap the selected index to follow the same repo,
/// and rebuild the filtered list preserving any active search query.
fn sort_repos_preserving_selection(state: &mut AppState) {
    let selected_repo_path = state
        .selected_repo_idx
        .and_then(|idx| state.repos.get(idx).map(|r| r.path.clone()));

    kiosk_core::state::sort_repos(
        &mut state.repos,
        state.current_repo_path.as_deref(),
        &state.session_activity,
    );

    state.selected_repo_idx =
        selected_repo_path.and_then(|path| state.repos.iter().position(|r| r.path == path));

    let names: Vec<&str> = state.repos.iter().map(|r| r.name.as_str()).collect();
    rebuild_filtered_preserving_search(&mut state.repo_list, &names);
}

/// Handle events from background tasks
#[allow(clippy::too_many_lines)]
fn process_app_event<T: TmuxProvider + ?Sized + 'static>(
    event: AppEvent,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    tmux: &Arc<T>,
    sender: &EventSender,
) -> Option<OpenAction> {
    match event {
        AppEvent::ReposDiscovered {
            mut repos,
            session_activity,
        } => {
            if !session_activity.is_empty() {
                state.session_activity = session_activity;
            }

            // Preserve worktrees from the pre-loaded initial repo (if any)
            if let Some(initial_path) = state.current_repo_path.as_deref()
                && let Some(existing) = state.repos.iter().find(|r| r.path == initial_path)
                && !existing.worktrees.is_empty()
            {
                let worktrees = existing.worktrees.clone();
                if let Some(scanned) = repos.iter_mut().find(|r| r.path == initial_path) {
                    scanned.worktrees = worktrees;
                }
            }

            state.repos = repos;
            state.loading_repos = false;
            state.loading_branches = false;

            sort_repos_preserving_selection(state);

            // Only switch to RepoSelect from Loading — don't kick users out of BranchSelect
            if matches!(state.mode, Mode::Loading(_)) {
                state.mode = Mode::RepoSelect;
            }
        }
        AppEvent::ReposFound { repo } => {
            // O(1) dedup via HashSet
            if state.seen_repo_paths.insert(repo.path.clone()) {
                state.repos.push(repo);

                let names: Vec<&str> = state.repos.iter().map(|r| r.name.as_str()).collect();
                rebuild_filtered_preserving_search(&mut state.repo_list, &names);
            }

            // Switch to RepoSelect from Loading (so user sees repos appearing)
            if matches!(state.mode, Mode::Loading(_)) {
                state.mode = Mode::RepoSelect;
            }
        }
        AppEvent::ScanComplete { search_dirs } => {
            // Scan is done — clear dedup set (no longer needed until next scan)
            state.seen_repo_paths.clear();

            // Run collision resolution: disambiguate repos with the same name
            // by appending the search dir name to session_name.
            // Uses `path.starts_with(dir)` with longest-prefix matching, which
            // is equivalent to the batch-mode `apply_collision_resolution` that
            // pairs each repo with its direct search dir — a repo found under
            // `dir` always has `path.starts_with(dir)` true, and the longest
            // matching dir is the one that discovered it.
            let mut name_counts = std::collections::HashMap::<String, usize>::new();
            for repo in &state.repos {
                *name_counts.entry(repo.name.clone()).or_insert(0) += 1;
            }
            if name_counts.values().any(|&count| count > 1) {
                for repo in &mut state.repos {
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

            // Sort repos now that all have been discovered
            sort_repos_preserving_selection(state);

            state.loading_repos = false;

            if matches!(state.mode, Mode::Loading(_)) {
                state.mode = Mode::RepoSelect;
            }
        }
        AppEvent::SessionActivityLoaded { session_activity } => {
            state.session_activity = session_activity;

            // Re-sort with activity data
            sort_repos_preserving_selection(state);
        }
        AppEvent::RepoEnriched {
            repo_path,
            worktrees,
        } => {
            // Just update worktrees in place — no need to re-sort since
            // worktree data doesn't affect sort order (only session activity does).
            if let Some(repo) = state.repos.iter_mut().find(|r| r.path == repo_path) {
                repo.worktrees = worktrees;
            }

            if state.reconcile_pending_worktree_deletes()
                && let Err(e) = save_pending_worktree_deletes(&state.pending_worktree_deletes)
            {
                state.set_error(format!("Failed to persist pending deletes: {e}"));
            }
        }
        AppEvent::WorktreeCreated { path, session_name } => {
            return Some(OpenAction::Open {
                path,
                session_name,
                split_command: state.split_command.clone(),
            });
        }
        AppEvent::WorktreeRemoved {
            branch_name: _,
            worktree_path,
        } => {
            state.clear_pending_worktree_delete_by_path(&worktree_path);
            if let Err(e) = save_pending_worktree_deletes(&state.pending_worktree_deletes) {
                state.set_error(format!("Failed to persist pending deletes: {e}"));
            }
            // Return to branch select and refresh the branch list
            if let Some(repo_idx) = state.selected_repo_idx {
                enter_branch_select_with_loading(state, repo_idx, git, tmux, sender, false);
            } else {
                state.mode = Mode::BranchSelect;
            }
        }
        AppEvent::WorktreeRemoveFailed {
            branch_name,
            worktree_path,
            error,
        } => {
            if let Some(repo) = state.selected_repo_idx.and_then(|idx| state.repos.get(idx)) {
                state.clear_pending_worktree_delete_by_branch(&repo.path.clone(), &branch_name);
            } else {
                state.clear_pending_worktree_delete_by_path(&worktree_path);
            }
            let mut error_message = format!("Failed to remove worktree for {branch_name}: {error}");
            if let Err(e) = save_pending_worktree_deletes(&state.pending_worktree_deletes) {
                let _ = write!(
                    error_message,
                    " (also failed to persist pending deletes: {e})"
                );
            }
            state.set_error(error_message);
            state.loading_branches = false;
            state.mode = Mode::BranchSelect;
        }
        AppEvent::BranchesLoaded {
            branches,
            worktrees,
            local_names,
            session_activity,
        } => {
            state.session_activity = session_activity;
            if let Some(repo) = state
                .selected_repo_idx
                .and_then(|idx| state.repos.get_mut(idx))
            {
                repo.worktrees = worktrees;
            }
            state.branches = branches;
            state.branch_list.reset(state.branches.len());
            state.loading_branches = false;
            if state.reconcile_pending_worktree_deletes()
                && let Err(e) = save_pending_worktree_deletes(&state.pending_worktree_deletes)
            {
                state.set_error(format!("Failed to persist pending deletes: {e}"));
            }
            state.mode = Mode::BranchSelect;

            // Kick off remote branch loading and background fetch
            if let Some(repo_path) = state
                .selected_repo_idx
                .and_then(|idx| state.repos.get(idx))
                .map(|r| r.path.clone())
            {
                spawn::spawn_remote_branch_loading(
                    git,
                    sender,
                    repo_path.clone(),
                    local_names.clone(),
                );
                state.fetching_remotes = true;
                spawn::spawn_git_fetch(git, sender, repo_path, local_names);
            }
        }
        AppEvent::RemoteBranchesLoaded { branches } => {
            if state.mode == Mode::BranchSelect {
                extend_branches_deduped(state, branches);
            }
        }
        AppEvent::GitFetchCompleted {
            branches,
            repo_path,
            error,
        } => {
            let current_repo_path = state
                .selected_repo_idx
                .and_then(|idx| state.repos.get(idx).map(|r| &r.path));
            if state.mode == Mode::BranchSelect && current_repo_path == Some(&repo_path) {
                state.fetching_remotes = false;
                if let Some(err) = error {
                    state.error = Some(format!("git fetch failed: {err}"));
                }
                extend_branches_deduped(state, branches);
            }
        }
        AppEvent::GitError(msg) => {
            // Return to the appropriate mode
            if state.base_branch_selection.is_some() {
                state.base_branch_selection = None;
                state.mode = Mode::BranchSelect;
            } else {
                state.mode = Mode::BranchSelect;
            }
            state.loading_branches = false;
            state.set_error(msg);
        }
    }
    None
}

fn handle_movement_actions(action: &Action, state: &mut AppState) -> bool {
    let page_rows_usize = state.active_list_page_rows();
    let page_rows: i32 = page_rows_usize.try_into().unwrap_or(i32::MAX);
    let page_step = page_rows.max(1);
    let half_page_step = (page_step / 2).max(1);

    let Some(list) = state.active_list_mut() else {
        return false;
    };
    match action {
        Action::HalfPageUp => {
            list.move_selection(-half_page_step);
        }
        Action::HalfPageDown => {
            list.move_selection(half_page_step);
        }
        Action::PageUp => {
            list.move_selection(-page_step);
        }
        Action::PageDown => {
            list.move_selection(page_step);
        }
        Action::MoveTop => {
            list.move_to_top();
        }
        Action::MoveBottom => {
            list.move_to_bottom();
        }
        _ => return false,
    }
    update_active_list_scroll_offset(state, page_rows_usize);
    true
}

fn update_active_list_scroll_offset(state: &mut AppState, viewport_rows: usize) {
    if let Mode::Help { .. } = state.mode {
        if let Some(overlay) = &mut state.help_overlay {
            update_help_scroll_offset(overlay, viewport_rows);
        }
    } else if let Some(list) = state.active_list_mut() {
        list.update_scroll_offset_for_selection(viewport_rows);
    }
}

fn update_help_scroll_offset(
    overlay: &mut kiosk_core::state::HelpOverlayState,
    viewport_rows: usize,
) {
    let (row_item_indices, total_visual_rows) = components::help::help_visual_metrics(overlay);
    let len = overlay.list.filtered.len();
    if len == 0 {
        overlay.list.scroll_offset = 0;
        return;
    }

    let selected = overlay.list.selected.unwrap_or(0).min(len - 1);
    let selected_visual = row_item_indices.get(selected).copied().unwrap_or(0);

    let viewport_rows = viewport_rows.max(1);
    let max_visual_offset = total_visual_rows.saturating_sub(viewport_rows);
    let current_visual_offset = overlay.list.scroll_offset.min(max_visual_offset);
    let anchor_top = usize::from(viewport_rows > 2);
    let anchor_bottom = viewport_rows.saturating_sub(2);

    let desired_visual_offset =
        if selected_visual < current_visual_offset.saturating_add(anchor_top) {
            selected_visual.saturating_sub(anchor_top)
        } else if selected_visual > current_visual_offset.saturating_add(anchor_bottom) {
            selected_visual.saturating_sub(anchor_bottom)
        } else {
            current_visual_offset
        }
        .min(max_visual_offset);

    overlay.list.scroll_offset = desired_visual_offset;
}

/// Handle simple cursor and error actions
fn handle_simple_actions(action: &Action, state: &mut AppState) -> bool {
    match action {
        Action::CursorLeft => {
            if let Some(input) = state.active_text_input() {
                input.cursor_left();
            }
            true
        }
        Action::CursorRight => {
            if let Some(input) = state.active_text_input() {
                input.cursor_right();
            }
            true
        }
        Action::CursorWordLeft => {
            if let Some(input) = state.active_text_input() {
                input.cursor_word_left();
            }
            true
        }
        Action::CursorWordRight => {
            if let Some(input) = state.active_text_input() {
                input.cursor_word_right();
            }
            true
        }
        Action::CursorStart => {
            if let Some(input) = state.active_text_input() {
                input.cursor_start();
            }
            true
        }
        Action::CursorEnd => {
            if let Some(input) = state.active_text_input() {
                input.cursor_end();
            }
            true
        }
        Action::CancelDeleteWorktree => {
            state.mode = Mode::BranchSelect;
            true
        }
        _ => false,
    }
}

struct ActionContext<'a, T: TmuxProvider + ?Sized + 'static> {
    git: &'a Arc<dyn GitProvider>,
    tmux: &'a Arc<T>,
    keys: &'a KeysConfig,
    matcher: &'a SkimMatcherV2,
    sender: &'a EventSender,
}

#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_lines)]
fn process_action<T: TmuxProvider + ?Sized + 'static>(
    action: Action,
    state: &mut AppState,
    ctx: &ActionContext<'_, T>,
) -> Option<OpenAction> {
    // Handle movement and simple actions first
    if handle_movement_actions(&action, state) || handle_simple_actions(&action, state) {
        return None;
    }

    match action {
        Action::Quit => return Some(OpenAction::Quit),

        Action::OpenRepo => {
            if let Some(sel) = state.repo_list.selected
                && let Some(&(idx, _)) = state.repo_list.filtered.get(sel)
            {
                let repo = &state.repos[idx];
                let session_name = repo.tmux_session_name(&repo.path);
                return Some(OpenAction::Open {
                    path: repo.path.clone(),
                    session_name,
                    split_command: state.split_command.clone(),
                });
            }
        }

        Action::EnterRepo => {
            if let Some(sel) = state.repo_list.selected
                && let Some(&(idx, _)) = state.repo_list.filtered.get(sel)
            {
                enter_branch_select(state, idx, ctx.git, ctx.tmux, ctx.sender);
            }
        }

        Action::GoBack => handle_go_back(state),

        Action::OpenBranch => {
            if let Some(result) = handle_open_branch(state, ctx.git, ctx.sender) {
                return Some(result);
            }
        }

        Action::StartNewBranchFlow => {
            handle_start_new_branch(state);
        }

        Action::MoveSelection(delta) => {
            if matches!(
                state.mode,
                Mode::Setup(kiosk_core::state::SetupStep::SearchDirs)
            ) {
                handle_setup_move_selection(state, delta);
            } else {
                if let Some(list) = state.active_list_mut() {
                    list.move_selection(delta);
                }
                let page_rows = state.active_list_page_rows();
                update_active_list_scroll_offset(state, page_rows);
            }
        }

        Action::SearchPush(c) => {
            handle_search_push(state, ctx.matcher, c);
        }
        Action::SearchPop => {
            handle_search_pop(state, ctx.matcher);
        }
        Action::SearchDeleteForward => {
            handle_search_delete_forward(state, ctx.matcher);
        }
        Action::SearchDeleteWordForward => {
            handle_search_delete_word_forward(state, ctx.matcher);
        }
        Action::SearchDeleteToStart => {
            handle_search_delete_to_start(state, ctx.matcher);
        }
        Action::SearchDeleteToEnd => {
            handle_search_delete_to_end(state, ctx.matcher);
        }

        Action::DeleteWorktree => handle_delete_worktree(state),
        Action::ConfirmDeleteWorktree => {
            handle_confirm_delete(state, ctx.git, ctx.tmux.as_ref(), ctx.sender);
        }

        Action::SearchDeleteWord => {
            handle_search_delete_word(state, ctx.matcher);
        }

        Action::ShowHelp => handle_show_help(state, ctx.keys),

        // Setup actions
        Action::SetupContinue => handle_setup_continue(state),
        Action::SetupAddDir => {
            if let Some(result) = handle_setup_add_dir(state) {
                return Some(result);
            }
        }
        Action::SetupTabComplete => handle_setup_tab_complete(state),
        Action::SetupCancel => {
            if let Some(result) = handle_setup_cancel(state) {
                return Some(result);
            }
        }

        // Movement, cursor, and cancel actions are handled by helper functions above.
        // If we reach here, it means the action wasn't applicable in the current mode
        // (e.g., movement action in a mode with no active list). This is safe to ignore.
        Action::HalfPageUp
        | Action::HalfPageDown
        | Action::PageUp
        | Action::PageDown
        | Action::MoveTop
        | Action::MoveBottom
        | Action::CursorLeft
        | Action::CursorRight
        | Action::CursorWordLeft
        | Action::CursorWordRight
        | Action::CursorStart
        | Action::CursorEnd
        | Action::CancelDeleteWorktree => {}
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::git::mock::MockGitProvider;
    use kiosk_core::git::{Repo, Worktree};
    use kiosk_core::state::{AppState, BranchEntry, Mode, SearchableList};
    use kiosk_core::tmux::{TmuxProvider, mock::MockTmuxProvider};

    fn make_sender() -> EventSender {
        let (tx, _rx) = mpsc::channel();
        EventSender {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn make_repo(name: &str) -> Repo {
        Repo {
            name: name.to_string(),
            session_name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{name}")),
            worktrees: vec![Worktree {
                path: PathBuf::from(format!("/tmp/{name}")),
                branch: Some("main".to_string()),
                is_main: true,
            }],
        }
    }

    fn default_ctx<'a>(
        git: &'a Arc<dyn GitProvider>,
        tmux: &'a Arc<dyn TmuxProvider>,
        keys: &'a KeysConfig,
        matcher: &'a SkimMatcherV2,
        sender: &'a EventSender,
    ) -> ActionContext<'a, dyn TmuxProvider> {
        ActionContext {
            git,
            tmux,
            keys,
            matcher,
            sender,
        }
    }

    #[test]
    fn test_enter_repo_populates_branches() {
        let repos = vec![make_repo("alpha"), make_repo("beta")];
        let mut state = AppState::new(repos, None);
        state.repo_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".into(), "dev".into()],
            ..Default::default()
        });
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = EventSender {
            tx,
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        let result = process_action(Action::EnterRepo, &mut state, &ctx);
        assert!(result.is_none());
        assert_eq!(state.mode, Mode::BranchSelect);
        assert!(state.loading_branches);
        assert!(state.branches.is_empty());

        // Wait for the background thread to send the event
        let event = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &git, &tmux, &sender);
        assert_eq!(state.mode, Mode::BranchSelect);
        assert!(!state.loading_branches);
        assert_eq!(state.branches.len(), 2);
    }

    #[test]
    fn test_remote_branches_appended() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "main".to_string(),
            worktree_path: None,
            has_session: false,
            is_current: true,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.reset(1);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        // Simulate remote branches arriving
        let remote_branches = vec![
            BranchEntry {
                name: "feature-x".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_remote: true,
                is_default: false,
                session_activity_ts: None,
            },
            BranchEntry {
                name: "feature-y".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_remote: true,
                is_default: false,
                session_activity_ts: None,
            },
        ];

        process_app_event(
            AppEvent::RemoteBranchesLoaded {
                branches: remote_branches,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert_eq!(state.branches.len(), 3);
        assert_eq!(state.branch_list.filtered.len(), 3);
        assert!(!state.branches[0].is_remote); // main stays first
        assert!(state.branches[1].is_remote); // feature-x
        assert!(state.branches[2].is_remote); // feature-y
    }

    #[test]
    fn test_remote_branches_filtered_with_search() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "main".to_string(),
            worktree_path: None,
            has_session: false,
            is_current: true,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.reset(1);
        state.branch_list.input.text = "feat".to_string();
        state.branch_list.input.cursor = 4;
        // With search "feat", main shouldn't match
        state.branch_list.filtered = vec![];
        state.branch_list.selected = None;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::RemoteBranchesLoaded {
                branches: vec![BranchEntry {
                    name: "feature-x".to_string(),
                    worktree_path: None,
                    has_session: false,
                    is_current: false,
                    is_remote: true,
                    is_default: false,
                    session_activity_ts: None,
                }],
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // "feat" should match "feature-x" but not "main"
        assert_eq!(state.branches.len(), 2);
        assert_eq!(state.branch_list.filtered.len(), 1);
        let matched_idx = state.branch_list.filtered[0].0;
        assert_eq!(state.branches[matched_idx].name, "feature-x");
    }

    #[test]
    fn test_go_back_from_branch_to_repo() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::GoBack, &mut state, &ctx);
        assert_eq!(state.mode, Mode::RepoSelect);
    }

    #[test]
    fn test_go_back_from_new_branch_to_branch() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::SelectBaseBranch;
        state.base_branch_selection = Some(kiosk_core::state::BaseBranchSelection {
            new_name: "feat".into(),
            bases: vec!["main".into()],
            list: SearchableList::new(1),
        });

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::GoBack, &mut state, &ctx);
        assert_eq!(state.mode, Mode::BranchSelect);
        assert!(state.base_branch_selection.is_none());
    }

    #[test]
    fn test_show_help_initializes_overlay_and_toggles_back() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        assert!(matches!(state.mode, Mode::Help { .. }));
        let overlay = state.help_overlay.as_ref().unwrap();
        assert!(!overlay.rows.is_empty());
        assert_eq!(overlay.list.filtered.len(), overlay.rows.len());

        process_action(Action::ShowHelp, &mut state, &ctx);

        assert_eq!(state.mode, Mode::RepoSelect);
        assert!(state.help_overlay.is_none());
    }

    #[test]
    fn test_help_search_and_movement_use_help_list_state() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        let initial_count = state
            .help_overlay
            .as_ref()
            .map_or(0, |overlay| overlay.list.filtered.len());
        process_action(Action::SearchPush('d'), &mut state, &ctx);
        process_action(Action::SearchPush('e'), &mut state, &ctx);

        let filtered_count = state
            .help_overlay
            .as_ref()
            .map_or(0, |overlay| overlay.list.filtered.len());
        assert!(filtered_count > 0);
        assert!(filtered_count <= initial_count);

        let before = state
            .help_overlay
            .as_ref()
            .and_then(|overlay| overlay.list.selected)
            .unwrap_or(0);
        process_action(Action::MoveSelection(1), &mut state, &ctx);
        let after = state
            .help_overlay
            .as_ref()
            .and_then(|overlay| overlay.list.selected)
            .unwrap_or(0);
        assert!(after >= before);
    }

    #[test]
    fn test_help_up_from_bottom_keeps_scroll_offset() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(20);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        for _ in 0..200 {
            process_action(Action::MoveSelection(1), &mut state, &ctx);
        }
        let offset_before = state
            .help_overlay
            .as_ref()
            .map_or(0, |overlay| overlay.list.scroll_offset);

        process_action(Action::MoveSelection(-1), &mut state, &ctx);
        let offset_after = state
            .help_overlay
            .as_ref()
            .map_or(0, |overlay| overlay.list.scroll_offset);

        assert_eq!(offset_before, offset_after);
    }

    #[test]
    fn test_help_down_keeps_selection_one_above_bottom_before_end() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(20);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        for _ in 0..20 {
            process_action(Action::MoveSelection(1), &mut state, &ctx);
        }

        let overlay = state.help_overlay.as_ref().expect("help overlay");
        let (indices, _total) = components::help::help_visual_metrics(overlay);
        let selected_logical = overlay.list.selected.expect("selected logical");
        let selected_visual = indices
            .get(selected_logical)
            .copied()
            .expect("selected visual");
        let offset_visual = overlay.list.scroll_offset;
        let row_in_view = selected_visual.saturating_sub(offset_visual);
        assert!(
            row_in_view <= 18,
            "Expected selected row to stay above visual bottom before end"
        );
    }

    #[test]
    fn test_open_branch_with_existing_worktree() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "main".into(),
            worktree_path: Some(PathBuf::from("/tmp/alpha")),
            has_session: false,
            is_current: true,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        let result = process_action(Action::OpenBranch, &mut state, &ctx);
        assert!(result.is_some());
        match result.unwrap() {
            OpenAction::Open {
                path, session_name, ..
            } => {
                assert_eq!(path, PathBuf::from("/tmp/alpha"));
                assert_eq!(session_name, "alpha");
            }
            OpenAction::Quit | OpenAction::SetupComplete => panic!("Expected OpenAction::Open"),
        }
    }

    #[test]
    fn test_open_branch_creates_worktree() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "dev".into(),
            worktree_path: None,
            has_session: false,
            is_current: false,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        let result = process_action(Action::OpenBranch, &mut state, &ctx);
        assert!(result.is_none());
        assert!(matches!(state.mode, Mode::Loading(_)));
    }

    #[test]
    fn test_search_push_filters() {
        let repos = vec![make_repo("alpha"), make_repo("beta")];
        let mut state = AppState::new(repos, None);
        assert_eq!(state.repo_list.filtered.len(), 2);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::SearchPush('a'), &mut state, &ctx);
        assert_eq!(state.repo_list.input.text, "a");
        // "alpha" matches "a", "beta" also matches "a" — but both should be present
        assert!(!state.repo_list.filtered.is_empty());
    }

    #[test]
    fn test_move_selection() {
        let repos = vec![make_repo("alpha"), make_repo("beta"), make_repo("gamma")];
        let mut state = AppState::new(repos, None);
        assert_eq!(state.repo_list.selected, Some(0));

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::MoveSelection(1), &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(1));

        process_action(Action::MoveSelection(1), &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(2));

        // Should clamp at max
        process_action(Action::MoveSelection(1), &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(2));
    }

    #[test]
    fn test_move_selection_updates_scroll_anchor() {
        let repos: Vec<_> = (0..40).map(|i| make_repo(&format!("repo-{i}"))).collect();
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(20);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        for _ in 0..25 {
            process_action(Action::MoveSelection(1), &mut state, &ctx);
        }

        let selected = state.repo_list.selected.unwrap_or(0);
        assert_eq!(selected, 25);
        assert_eq!(
            selected - state.repo_list.scroll_offset,
            18,
            "Selection should remain one row above bottom while scrolling down"
        );
    }

    #[test]
    fn test_page_movement_uses_active_list_page_rows() {
        let repos: Vec<_> = (0..20).map(|i| make_repo(&format!("repo-{i}"))).collect();
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(8);
        assert_eq!(state.repo_list.selected, Some(0));

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::HalfPageDown, &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(4));

        process_action(Action::PageDown, &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(12));

        process_action(Action::PageUp, &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(4));
    }

    #[test]
    fn test_page_movement_clamps_to_bounds() {
        let repos: Vec<_> = (0..6).map(|i| make_repo(&format!("repo-{i}"))).collect();
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(20);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::PageDown, &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(5));

        process_action(Action::HalfPageDown, &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(5));

        process_action(Action::PageUp, &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(0));

        process_action(Action::HalfPageUp, &mut state, &ctx);
        assert_eq!(state.repo_list.selected, Some(0));
    }

    #[test]
    fn test_half_page_uses_viewport_rows_when_list_is_shorter() {
        let repos: Vec<_> = (0..13).map(|i| make_repo(&format!("repo-{i}"))).collect();
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(20);
        assert_eq!(state.repo_list.selected, Some(0));

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::HalfPageDown, &mut state, &ctx);
        assert_eq!(
            state.repo_list.selected,
            Some(10),
            "Half-page should move by half viewport rows (20/2)"
        );

        process_action(Action::HalfPageDown, &mut state, &ctx);
        assert_eq!(
            state.repo_list.selected,
            Some(12),
            "Should clamp to list end"
        );
    }

    #[test]
    fn test_open_repo_returns_repo_path() {
        let repos = vec![make_repo("alpha"), make_repo("beta")];
        let mut state = AppState::new(repos, Some("hx".into()));
        state.repo_list.selected = Some(1);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        let result = process_action(Action::OpenRepo, &mut state, &ctx);
        assert!(result.is_some());
        match result.unwrap() {
            OpenAction::Open {
                path,
                session_name,
                split_command,
            } => {
                assert_eq!(path, PathBuf::from("/tmp/beta"));
                assert_eq!(session_name, "beta");
                assert_eq!(split_command.as_deref(), Some("hx"));
            }
            OpenAction::Quit | OpenAction::SetupComplete => panic!("Expected OpenAction::Open"),
        }
    }

    #[test]
    fn test_new_branch_empty_name_shows_error() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.selected_repo_idx = Some(0);
        state.branch_list.input.text = String::new(); // empty

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".into()],
            ..Default::default()
        });
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::StartNewBranchFlow, &mut state, &ctx);

        assert_eq!(
            state.mode,
            Mode::BranchSelect,
            "Should stay in BranchSelect"
        );
        assert!(
            state.error.is_some(),
            "Should show an error for empty branch name"
        );
        assert!(state.error.unwrap().contains("branch name"));
    }

    #[test]
    fn test_new_branch_with_name_enters_flow() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.selected_repo_idx = Some(0);
        state.branch_list.input.text = "feat/new".to_string();
        state.branches = vec![BranchEntry {
            name: "main".into(),
            worktree_path: Some(PathBuf::from("/tmp/alpha")),
            has_session: false,
            is_current: true,
            is_remote: false,
            is_default: true,
            session_activity_ts: None,
        }];

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".into()],
            ..Default::default()
        });
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::StartNewBranchFlow, &mut state, &ctx);

        assert_eq!(state.mode, Mode::SelectBaseBranch);
        assert!(state.base_branch_selection.is_some());
        assert_eq!(state.base_branch_selection.unwrap().new_name, "feat/new");
    }

    #[test]
    fn test_delete_worktree_no_worktree_shows_error() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "dev".to_string(),
            worktree_path: None,
            has_session: false,
            is_current: false,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::DeleteWorktree, &mut state, &ctx);

        assert_eq!(state.mode, Mode::BranchSelect);
        assert!(state.error.is_some());
        assert!(state.error.unwrap().contains("No worktree"));
    }

    #[test]
    fn test_delete_worktree_current_branch_shows_error() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "main".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/alpha")),
            has_session: false,
            is_current: true,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::DeleteWorktree, &mut state, &ctx);

        assert_eq!(state.mode, Mode::BranchSelect);
        assert!(state.error.is_some());
        assert!(state.error.unwrap().contains("current branch"));
    }

    #[test]
    fn test_delete_worktree_valid_shows_confirm() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "dev".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/alpha-dev")),
            has_session: false,
            is_current: false,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::DeleteWorktree, &mut state, &ctx);

        assert_eq!(
            state.mode,
            Mode::ConfirmWorktreeDelete {
                branch_name: "dev".to_string(),
                has_session: false,
            }
        );
        assert!(state.error.is_none());
    }

    #[test]
    fn test_delete_worktree_with_session_shows_session_warning() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.branches = vec![BranchEntry {
            name: "dev".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/alpha-dev")),
            has_session: true,
            is_current: false,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::DeleteWorktree, &mut state, &ctx);

        assert_eq!(
            state.mode,
            Mode::ConfirmWorktreeDelete {
                branch_name: "dev".to_string(),
                has_session: true,
            }
        );
    }

    #[test]
    fn test_confirm_delete_kills_tmux_session() {
        let mut repos = vec![make_repo("alpha")];
        repos[0].worktrees.push(Worktree {
            path: PathBuf::from("/tmp/alpha-dev"),
            branch: Some("dev".to_string()),
            is_main: false,
        });
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::ConfirmWorktreeDelete {
            branch_name: "dev".to_string(),
            has_session: true,
        };
        state.branches = vec![BranchEntry {
            name: "dev".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/alpha-dev")),
            has_session: true,
            is_current: false,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = ActionContext {
            git: &git,
            tmux: &tmux,
            keys: &keys,
            matcher: &matcher,
            sender: &sender,
        };

        process_action(Action::ConfirmDeleteWorktree, &mut state, &ctx);

        let killed = tmux.killed_sessions.lock().unwrap();
        assert_eq!(killed.as_slice(), &["alpha-dev"]);
        assert!(matches!(state.mode, Mode::BranchSelect));
        assert_eq!(state.pending_worktree_deletes.len(), 1);
    }

    #[test]
    fn test_confirm_delete_without_session_does_not_kill() {
        let mut repos = vec![make_repo("alpha")];
        repos[0].worktrees.push(Worktree {
            path: PathBuf::from("/tmp/alpha-dev"),
            branch: Some("dev".to_string()),
            is_main: false,
        });
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::ConfirmWorktreeDelete {
            branch_name: "dev".to_string(),
            has_session: false,
        };
        state.branches = vec![BranchEntry {
            name: "dev".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/alpha-dev")),
            has_session: false,
            is_current: false,
            is_remote: false,
            is_default: false,
            session_activity_ts: None,
        }];

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = ActionContext {
            git: &git,
            tmux: &tmux,
            keys: &keys,
            matcher: &matcher,
            sender: &sender,
        };

        process_action(Action::ConfirmDeleteWorktree, &mut state, &ctx);

        let killed = tmux.killed_sessions.lock().unwrap();
        assert!(killed.is_empty());
        assert!(matches!(state.mode, Mode::BranchSelect));
        assert_eq!(state.pending_worktree_deletes.len(), 1);
    }

    #[test]
    fn test_worktree_removed_event_clears_pending_delete() {
        let mut state = AppState::new(vec![make_repo("alpha")], None);
        state.selected_repo_idx = Some(0);
        state.mark_pending_worktree_delete(kiosk_core::pending_delete::PendingWorktreeDelete::new(
            PathBuf::from("/tmp/alpha"),
            "dev".to_string(),
            PathBuf::from("/tmp/alpha-dev"),
        ));

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".to_string(), "dev".to_string()],
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/alpha"),
                branch: Some("main".to_string()),
                is_main: true,
            }],
            ..Default::default()
        });
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::WorktreeRemoved {
                branch_name: "dev".to_string(),
                worktree_path: PathBuf::from("/tmp/alpha-dev"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert!(state.pending_worktree_deletes.is_empty());
    }

    #[test]
    fn test_worktree_remove_failed_event_clears_pending_and_sets_error() {
        let mut state = AppState::new(vec![make_repo("alpha")], None);
        state.selected_repo_idx = Some(0);
        state.mark_pending_worktree_delete(kiosk_core::pending_delete::PendingWorktreeDelete::new(
            PathBuf::from("/tmp/alpha"),
            "dev".to_string(),
            PathBuf::from("/tmp/alpha-dev"),
        ));

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::WorktreeRemoveFailed {
                branch_name: "dev".to_string(),
                worktree_path: PathBuf::from("/tmp/alpha-dev"),
                error: "boom".to_string(),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert!(state.pending_worktree_deletes.is_empty());
        assert!(
            state
                .error
                .as_deref()
                .is_some_and(|msg| msg.contains("Failed to remove"))
        );
    }

    #[test]
    fn test_cursor_movement_multibyte() {
        // "café" = 5 bytes: c(1) a(1) f(1) é(2)
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.repo_list.input.text = "café".to_string();
        state.repo_list.input.cursor = state.repo_list.input.text.len(); // 5 (byte len)

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        // Move left from end should skip over the 2-byte 'é'
        process_action(Action::CursorLeft, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 3); // before 'é' (byte offset of 'é')

        // Move left again should land before 'f'
        process_action(Action::CursorLeft, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 2);

        // Move right should skip over 'f' (1 byte)
        process_action(Action::CursorRight, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 3);

        // Move right should skip over 'é' (2 bytes)
        process_action(Action::CursorRight, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 5);
    }

    #[test]
    fn test_backspace_multibyte() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.repo_list.input.text = "café".to_string();
        state.repo_list.input.cursor = state.repo_list.input.text.len(); // 5

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        // Backspace should remove 'é' (2 bytes)
        process_action(Action::SearchPop, &mut state, &ctx);
        assert_eq!(state.repo_list.input.text, "caf");
        assert_eq!(state.repo_list.input.cursor, 3);
    }

    #[test]
    fn test_cursor_movement_in_search() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.repo_list.input.text = "hello".to_string();
        state.repo_list.input.cursor = 5; // at end

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        // Move cursor left
        process_action(Action::CursorLeft, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 4);

        // Move cursor to start
        process_action(Action::CursorStart, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 0);

        // Move cursor to end
        process_action(Action::CursorEnd, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 5);

        // Move cursor right at end stays at end
        process_action(Action::CursorRight, &mut state, &ctx);
        assert_eq!(state.repo_list.input.cursor, 5);
    }

    // ── update_help_scroll_offset direct tests ──

    fn make_help_overlay(
        num_rows: usize,
        num_sections: usize,
    ) -> kiosk_core::state::HelpOverlayState {
        use kiosk_core::config::Command;
        use kiosk_core::config::keys::FlattenedKeybindingRow;

        let section_names: Vec<&'static str> = vec![
            "general",
            "text_edit",
            "list_navigation",
            "repo_select",
            "branch_select",
            "modal",
        ];
        let rows: Vec<FlattenedKeybindingRow> = (0..num_rows)
            .map(|i| {
                let sec = i % num_sections;
                FlattenedKeybindingRow {
                    section_index: sec,
                    section_name: section_names[sec.min(section_names.len() - 1)],
                    key_display: format!("K-{i:02}"),
                    command: Command::MoveDown,
                    description: Command::MoveDown.labels().description,
                }
            })
            .collect();
        kiosk_core::state::HelpOverlayState {
            list: SearchableList::new(rows.len()),
            rows,
        }
    }

    #[test]
    fn test_update_help_scroll_offset_selection_at_top() {
        let mut overlay = make_help_overlay(30, 3);
        // Selection is at 0 (top), viewport 20 — offset should be 0
        update_help_scroll_offset(&mut overlay, 20);
        assert_eq!(overlay.list.scroll_offset, 0);
    }

    #[test]
    fn test_update_help_scroll_offset_selection_at_middle() {
        let mut overlay = make_help_overlay(30, 3);
        overlay.list.selected = Some(15);
        update_help_scroll_offset(&mut overlay, 20);
        // Visual row for selection 15 should be visible in viewport
        let (indices, _) = components::help::help_visual_metrics(&overlay);
        let sel_visual = indices[15];
        assert!(sel_visual >= overlay.list.scroll_offset);
        assert!(sel_visual < overlay.list.scroll_offset + 20);
    }

    #[test]
    fn test_update_help_scroll_offset_selection_at_bottom() {
        let mut overlay = make_help_overlay(30, 3);
        overlay.list.selected = Some(29);
        update_help_scroll_offset(&mut overlay, 20);
        let (indices, _) = components::help::help_visual_metrics(&overlay);
        let sel_visual = indices[29];
        assert!(sel_visual >= overlay.list.scroll_offset);
        assert!(sel_visual < overlay.list.scroll_offset + 20);
    }

    #[test]
    fn test_update_help_scroll_offset_empty_filtered_list() {
        let mut overlay = make_help_overlay(10, 2);
        overlay.list.filtered = vec![];
        overlay.list.selected = None;
        update_help_scroll_offset(&mut overlay, 20);
        assert_eq!(overlay.list.scroll_offset, 0);
    }

    #[test]
    fn test_update_help_scroll_offset_tiny_viewport() {
        let mut overlay = make_help_overlay(20, 2);
        overlay.list.selected = Some(10);
        // viewport_rows = 1 — should not panic, selection should be visible
        update_help_scroll_offset(&mut overlay, 1);
        let (indices, _) = components::help::help_visual_metrics(&overlay);
        let sel_visual = indices[10];
        assert_eq!(overlay.list.scroll_offset, sel_visual);
    }

    #[test]
    fn test_update_help_scroll_offset_viewport_2_rows() {
        let mut overlay = make_help_overlay(20, 2);
        overlay.list.selected = Some(10);
        update_help_scroll_offset(&mut overlay, 2);
        let (indices, _) = components::help::help_visual_metrics(&overlay);
        let sel_visual = indices[10];
        assert!(sel_visual >= overlay.list.scroll_offset);
        assert!(sel_visual < overlay.list.scroll_offset + 2);
    }

    #[test]
    fn test_update_help_scroll_offset_viewport_larger_than_content() {
        let mut overlay = make_help_overlay(5, 1);
        overlay.list.selected = Some(4);
        update_help_scroll_offset(&mut overlay, 100);
        assert_eq!(
            overlay.list.scroll_offset, 0,
            "Offset should be 0 when viewport is larger than content"
        );
    }

    #[test]
    fn test_update_help_scroll_offset_sequential_moves_down_then_up() {
        let mut overlay = make_help_overlay(40, 4);
        let viewport = 10;
        // Move all the way down
        for _ in 0..39 {
            overlay.list.move_selection(1);
            update_help_scroll_offset(&mut overlay, viewport);
        }
        let offset_at_bottom = overlay.list.scroll_offset;
        // Move up one — offset should not change
        overlay.list.move_selection(-1);
        update_help_scroll_offset(&mut overlay, viewport);
        assert_eq!(
            overlay.list.scroll_offset, offset_at_bottom,
            "First up from bottom should not change offset"
        );
    }

    // ── Help mode movement action tests ──

    #[test]
    fn test_help_page_down_moves_selection() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(10);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        let initial_selected = state
            .help_overlay
            .as_ref()
            .and_then(|o| o.list.selected)
            .unwrap_or(0);

        process_action(Action::PageDown, &mut state, &ctx);

        let after = state
            .help_overlay
            .as_ref()
            .and_then(|o| o.list.selected)
            .unwrap_or(0);
        assert!(
            after > initial_selected,
            "PageDown should advance selection in help mode"
        );
        assert!(after >= 10, "PageDown should move by at least page_rows");
    }

    #[test]
    fn test_help_page_up_moves_selection() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(10);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        // Move down first
        for _ in 0..20 {
            process_action(Action::MoveSelection(1), &mut state, &ctx);
        }
        let before = state
            .help_overlay
            .as_ref()
            .and_then(|o| o.list.selected)
            .unwrap_or(0);

        process_action(Action::PageUp, &mut state, &ctx);

        let after = state
            .help_overlay
            .as_ref()
            .and_then(|o| o.list.selected)
            .unwrap_or(0);
        assert!(
            after < before,
            "PageUp should move selection backwards in help mode"
        );
    }

    #[test]
    fn test_help_half_page_down_moves_selection() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(20);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        process_action(Action::HalfPageDown, &mut state, &ctx);

        let after = state
            .help_overlay
            .as_ref()
            .and_then(|o| o.list.selected)
            .unwrap_or(0);
        assert_eq!(after, 10, "HalfPageDown should move by half the viewport");
    }

    #[test]
    fn test_help_move_top_and_bottom() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(10);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        let total = state
            .help_overlay
            .as_ref()
            .map_or(0, |o| o.list.filtered.len());
        assert!(total > 1, "Help overlay should have multiple rows");

        process_action(Action::MoveBottom, &mut state, &ctx);
        let after_bottom = state
            .help_overlay
            .as_ref()
            .and_then(|o| o.list.selected)
            .unwrap_or(0);
        assert_eq!(after_bottom, total - 1, "MoveBottom should go to last item");

        process_action(Action::MoveTop, &mut state, &ctx);
        let after_top = state
            .help_overlay
            .as_ref()
            .and_then(|o| o.list.selected)
            .unwrap_or(usize::MAX);
        assert_eq!(after_top, 0, "MoveTop should go to first item");
        assert_eq!(
            state
                .help_overlay
                .as_ref()
                .map_or(usize::MAX, |o| o.list.scroll_offset),
            0,
            "MoveTop should reset scroll offset to 0"
        );
    }

    // ── Help toggle + parent mode round-trip tests ──

    #[test]
    fn test_help_toggle_from_branch_select_restores_mode() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);
        assert!(matches!(state.mode, Mode::Help { .. }));
        assert!(state.help_overlay.is_some());

        process_action(Action::ShowHelp, &mut state, &ctx);
        assert_eq!(state.mode, Mode::BranchSelect);
        assert!(
            state.help_overlay.is_none(),
            "Toggle off should clear help_overlay"
        );
    }

    #[test]
    fn test_help_toggle_from_select_base_branch_restores_mode() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::SelectBaseBranch;
        state.base_branch_selection = Some(kiosk_core::state::BaseBranchSelection {
            new_name: "feat".into(),
            bases: vec!["main".into()],
            list: SearchableList::new(1),
        });

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);
        assert!(matches!(state.mode, Mode::Help { .. }));

        process_action(Action::ShowHelp, &mut state, &ctx);
        assert_eq!(state.mode, Mode::SelectBaseBranch);
        assert!(state.help_overlay.is_none());
        assert!(
            state.base_branch_selection.is_some(),
            "Base branch selection should survive help round-trip"
        );
    }

    #[test]
    fn test_help_toggle_from_confirm_worktree_delete_restores_mode() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::ConfirmWorktreeDelete {
            branch_name: "dev".to_string(),
            has_session: true,
        };

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);
        assert!(matches!(state.mode, Mode::Help { .. }));

        process_action(Action::ShowHelp, &mut state, &ctx);
        assert_eq!(
            state.mode,
            Mode::ConfirmWorktreeDelete {
                branch_name: "dev".to_string(),
                has_session: true,
            }
        );
        assert!(state.help_overlay.is_none());
    }

    // ── Help search filtering tests ──

    #[test]
    fn test_help_search_no_matches_empties_filtered() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        // Type nonsense that won't match any keybinding
        for c in "zzzzxxx".chars() {
            process_action(Action::SearchPush(c), &mut state, &ctx);
        }

        let overlay = state.help_overlay.as_ref().expect("overlay");
        assert!(
            overlay.list.filtered.is_empty(),
            "Nonsense search should yield zero results"
        );
        assert_eq!(overlay.list.selected, None);
        assert_eq!(overlay.list.scroll_offset, 0);
    }

    // ── Search filtering + scroll offset interaction ──

    #[test]
    fn test_help_search_resets_scroll_offset_after_scrolling() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.set_active_list_page_rows(10);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        // Scroll down significantly
        for _ in 0..30 {
            process_action(Action::MoveSelection(1), &mut state, &ctx);
        }
        let offset_before_search = state
            .help_overlay
            .as_ref()
            .map_or(0, |o| o.list.scroll_offset);
        assert!(offset_before_search > 0, "Should have scrolled down");

        // Type a search query — scroll_offset and selection should reset
        process_action(Action::SearchPush('q'), &mut state, &ctx);
        let overlay = state.help_overlay.as_ref().expect("overlay");
        assert_eq!(
            overlay.list.scroll_offset, 0,
            "Search should reset scroll offset"
        );
        assert_eq!(
            overlay.list.selected,
            Some(0),
            "Search should reset selection to first match"
        );

        // Clear search — should restore full list with offset reset
        process_action(Action::SearchPop, &mut state, &ctx);
        let overlay = state.help_overlay.as_ref().expect("overlay");
        assert_eq!(
            overlay.list.scroll_offset, 0,
            "Clearing search should keep offset at 0"
        );
        assert_eq!(overlay.list.selected, Some(0));
        let initial_count = state.help_overlay.as_ref().map_or(0, |o| o.rows.len());
        assert_eq!(
            overlay.list.filtered.len(),
            initial_count,
            "Clearing search should restore full list"
        );
    }

    // ── Help cursor movement tests ──

    #[test]
    fn test_help_cursor_movement_in_search_bar() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        // Type "hello world"
        for c in "hello world".chars() {
            process_action(Action::SearchPush(c), &mut state, &ctx);
        }

        let help_cursor = |s: &AppState| s.help_overlay.as_ref().map_or(0, |o| o.list.input.cursor);
        assert_eq!(help_cursor(&state), 11); // at end

        // Cursor left
        process_action(Action::CursorLeft, &mut state, &ctx);
        assert_eq!(help_cursor(&state), 10);

        // Cursor word left: should jump past "world" to before "hello"
        process_action(Action::CursorWordLeft, &mut state, &ctx);
        assert_eq!(help_cursor(&state), 6); // before "world"

        process_action(Action::CursorWordLeft, &mut state, &ctx);
        assert_eq!(help_cursor(&state), 0); // before "hello"

        // Cursor word right
        process_action(Action::CursorWordRight, &mut state, &ctx);
        assert_eq!(help_cursor(&state), 5); // after "hello"

        // Cursor end
        process_action(Action::CursorEnd, &mut state, &ctx);
        assert_eq!(help_cursor(&state), 11);

        // Cursor start
        process_action(Action::CursorStart, &mut state, &ctx);
        assert_eq!(help_cursor(&state), 0);

        // Cursor right
        process_action(Action::CursorRight, &mut state, &ctx);
        assert_eq!(help_cursor(&state), 1);
    }

    // ── Multibyte character handling in help search ──

    #[test]
    fn test_help_search_multibyte_characters() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let keys = KeysConfig::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();
        let ctx = default_ctx(&git, &tmux, &keys, &matcher, &sender);

        process_action(Action::ShowHelp, &mut state, &ctx);

        // Type multibyte characters
        for c in "café".chars() {
            process_action(Action::SearchPush(c), &mut state, &ctx);
        }

        let overlay = state.help_overlay.as_ref().expect("overlay");
        assert_eq!(overlay.list.input.text, "café");
        assert_eq!(overlay.list.input.cursor, "café".len()); // 5 bytes

        // Backspace should remove 'é' (2 bytes)
        process_action(Action::SearchPop, &mut state, &ctx);
        let overlay = state.help_overlay.as_ref().expect("overlay");
        assert_eq!(overlay.list.input.text, "caf");
        assert_eq!(overlay.list.input.cursor, 3);

        // Cursor left and right should handle multibyte correctly
        process_action(Action::SearchPush('ñ'), &mut state, &ctx);
        let overlay = state.help_overlay.as_ref().expect("overlay");
        assert_eq!(overlay.list.input.text, "cafñ");

        process_action(Action::CursorLeft, &mut state, &ctx);
        let cursor = state
            .help_overlay
            .as_ref()
            .map_or(0, |o| o.list.input.cursor);
        assert_eq!(cursor, 3, "Cursor should be before 'ñ'");

        process_action(Action::CursorRight, &mut state, &ctx);
        let cursor = state
            .help_overlay
            .as_ref()
            .map_or(0, |o| o.list.input.cursor);
        assert_eq!(cursor, "cafñ".len(), "Cursor should be after 'ñ'");
    }

    #[test]
    fn test_repos_discovered_preserves_search_state() {
        let repos = vec![make_repo("alpha"), make_repo("beta"), make_repo("gamma")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::RepoSelect;

        // Simulate user typing "al" in search
        state.repo_list.input.text = "al".to_string();
        state.repo_list.input.cursor = 2;
        let matcher = SkimMatcherV2::default();
        state.repo_list.filtered = state
            .repos
            .iter()
            .enumerate()
            .filter_map(|(i, r)| matcher.fuzzy_match(&r.name, "al").map(|score| (i, score)))
            .collect();
        state.repo_list.selected = if state.repo_list.filtered.is_empty() {
            None
        } else {
            Some(0)
        };

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        // New repos arrive (simulating background discovery)
        let new_repos = vec![
            make_repo("alpha"),
            make_repo("beta"),
            make_repo("gamma"),
            make_repo("delta"),
        ];

        process_app_event(
            AppEvent::ReposDiscovered {
                repos: new_repos,
                session_activity: std::collections::HashMap::new(),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // Search text and cursor must be preserved
        assert_eq!(state.repo_list.input.text, "al");
        assert_eq!(state.repo_list.input.cursor, 2);
        // "alpha" and "delta" match "al"; the others don't
        assert!(
            !state.repo_list.filtered.is_empty(),
            "filtered should contain matches for 'al'"
        );
        for &(idx, _) in &state.repo_list.filtered {
            assert!(
                state.repos[idx].name.contains('a') || state.repos[idx].name.contains('l'),
                "filtered repos should fuzzy-match 'al'"
            );
        }
        assert_eq!(state.repos.len(), 4);
        // Mode should stay RepoSelect, not get reset to Loading
        assert_eq!(state.mode, Mode::RepoSelect);
    }

    #[test]
    fn test_repos_discovered_does_not_kick_from_branch_select() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.selected_repo_idx = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::ReposDiscovered {
                repos: vec![make_repo("alpha"), make_repo("beta")],
                session_activity: std::collections::HashMap::new(),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // Should stay in BranchSelect
        assert_eq!(state.mode, Mode::BranchSelect);
        assert_eq!(state.repos.len(), 2);
    }

    // -- loading dialog sizing tests (via Dialog) --

    #[test]
    fn test_loading_dialog_short_message_single_line() {
        let dialog = build_loading_dialog("⠋ ".to_string(), "Fetching...", Color::Magenta);
        let (width, height) = dialog.size(100);
        assert_eq!(width, 80);
        assert_eq!(height, 3);
    }

    #[test]
    fn test_loading_dialog_long_message_wraps() {
        let msg = "Creating branch my-very-long-feature-branch-name from origin/main-development-branch...";
        let dialog = build_loading_dialog("⠋ ".to_string(), msg, Color::Magenta);
        let (width, height) = dialog.size(60);
        assert_eq!(width, 48);
        assert!(
            height > 3,
            "long message should wrap to more than 1 line, got height {height}"
        );
    }

    #[test]
    fn test_loading_dialog_narrow_terminal() {
        let dialog = build_loading_dialog(
            "⠋ ".to_string(),
            "Creating branch foo from bar...",
            Color::Magenta,
        );
        let (width, _height) = dialog.size(30);
        assert_eq!(width, 24);
    }

    #[test]
    fn test_loading_dialog_uses_dialog_width() {
        let dialog = build_loading_dialog("⠋ ".to_string(), "test", Color::Magenta);
        let (w1, _) = dialog.size(80);
        let (w2, _) = dialog.size(40);
        assert!(w1 > w2, "wider terminal should produce wider dialog");
    }

    // -- confirm_delete_dialog sizing tests (via Dialog) --

    fn confirm_dialog_size(
        branch_name: &str,
        has_session: bool,
        terminal_width: u16,
    ) -> (u16, u16) {
        build_confirm_delete_dialog(
            branch_name,
            has_session,
            "enter",
            "esc",
            Color::Magenta,
            Color::Blue,
        )
        .size(terminal_width)
    }

    #[test]
    fn test_confirm_delete_layout_short_branch() {
        let (w, h) = confirm_dialog_size("main", false, 120);
        assert_eq!(w, 80, "width should be capped at 80");
        assert_eq!(h, 7, "no wrapping needed for short branch");
    }

    #[test]
    fn test_confirm_delete_layout_long_branch() {
        let long_name = "a".repeat(100);
        let (w, h) = confirm_dialog_size(&long_name, false, 120);
        assert_eq!(w, 80, "width should be capped at 80");
        assert!(h > 7, "long branch should cause wrapping, height={h}");
    }

    #[test]
    fn test_confirm_delete_layout_very_long_branch() {
        let long_name = "a".repeat(200);
        let (w, h) = confirm_dialog_size(&long_name, false, 80);
        assert_eq!(w, 64, "width should be 80% of terminal");
        assert!(
            h > 8,
            "very long branch on narrow terminal needs more wrapping, height={h}",
        );
    }

    #[test]
    fn test_confirm_delete_layout_narrow_terminal() {
        let (w, _h) = confirm_dialog_size("main", false, 50);
        assert!(w <= 50, "dialog width {w} must fit in terminal");
    }

    #[test]
    fn test_confirm_delete_layout_session_same_width() {
        let (w_without, _) = confirm_dialog_size("feature-branch", false, 120);
        let (w_with, _) = confirm_dialog_size("feature-branch", true, 120);
        assert_eq!(
            w_with, w_without,
            "width is content-independent: with session ({w_with}) should equal without ({w_without})",
        );
    }

    #[test]
    fn test_confirm_delete_layout_exact_fit_no_wrap() {
        let (_w, h) = confirm_dialog_size("exactly-fits", false, 120);
        assert_eq!(h, 7, "exact fit should not wrap");
    }

    // -- rendering tests --

    fn buf_to_string(buf: &ratatui::buffer::Buffer) -> String {
        let area = buf.area();
        let mut s = String::new();
        for y in area.y..area.y + area.height {
            if y > area.y {
                s.push('\n');
            }
            for x in area.x..area.x + area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
        }
        s
    }

    fn render_confirm_dialog_to_buffer(
        branch_name: &str,
        has_session: bool,
        terminal_width: u16,
    ) -> (ratatui::buffer::Buffer, u16, u16) {
        use ratatui::widgets::{Block, Borders, Wrap};

        let dialog = build_confirm_delete_dialog(
            branch_name,
            has_session,
            "enter",
            "esc",
            Color::Magenta,
            Color::Blue,
        );
        let (w, h) = dialog.size(terminal_width);

        let action_text = if has_session {
            "Delete worktree and kill tmux session for branch "
        } else {
            "Delete worktree for branch "
        };
        let lines = vec![
            Line::from(vec![
                Span::raw(action_text),
                Span::styled(
                    format!("\"{branch_name}\""),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("?"),
            ]),
            Line::raw(""),
            Line::from(vec![
                Span::raw("confirm ("),
                Span::styled(
                    "enter",
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(")"),
                Span::raw(" / "),
                Span::raw("cancel ("),
                Span::styled(
                    "esc",
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(")"),
            ]),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Confirm delete ")
            .border_style(Style::default().fg(Color::Magenta))
            .padding(Padding::uniform(1));
        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Center);
        let area = Rect::new(0, 0, w, h);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::Widget::render(paragraph, area, &mut buf);
        (buf, w, h)
    }

    #[test]
    fn test_confirm_delete_render_full_text_visible() {
        let (buf, _w, _h) = render_confirm_dialog_to_buffer("main", false, 120);
        let rendered = buf_to_string(&buf);
        assert!(
            rendered.contains("main"),
            "branch name missing:\n{rendered}"
        );
        assert!(
            rendered.contains("Delete worktree"),
            "action text missing:\n{rendered}"
        );
        assert!(
            rendered.contains("confirm"),
            "confirm hint missing:\n{rendered}"
        );
        assert!(
            rendered.contains("cancel"),
            "cancel hint missing:\n{rendered}"
        );
    }

    #[test]
    fn test_confirm_delete_render_wrapping() {
        let long_name = "x".repeat(100);
        let (buf, _w, _h) = render_confirm_dialog_to_buffer(&long_name, false, 120);
        let rendered = buf_to_string(&buf);
        let x_count = rendered.chars().filter(|c| *c == 'x').count();
        assert_eq!(
            x_count, 100,
            "all branch chars should be rendered:\n{rendered}"
        );
    }

    #[test]
    fn test_confirm_delete_render_narrow_terminal_hints_visible() {
        // On a narrow terminal the hints line wraps; both "confirm" and "cancel"
        // must still be fully rendered (this was the bug that motivated the
        // word_wrapped_line_count fix).
        let (buf, _w, _h) = render_confirm_dialog_to_buffer("feat/headless-cli", true, 28);
        let rendered = buf_to_string(&buf);
        assert!(
            rendered.contains("confirm"),
            "confirm hint missing:\n{rendered}"
        );
        assert!(
            rendered.contains("cancel"),
            "cancel hint missing:\n{rendered}"
        );
        assert!(
            rendered.contains("feat/headless-cli"),
            "branch name missing:\n{rendered}",
        );
    }

    #[test]
    fn test_confirm_delete_render_border_positions() {
        let (buf, w, h) = render_confirm_dialog_to_buffer("main", false, 120);
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "┌");
        assert_eq!(buf.cell((w - 1, 0)).unwrap().symbol(), "┐");
        assert_eq!(buf.cell((0, h - 1)).unwrap().symbol(), "└");
        assert_eq!(buf.cell((w - 1, h - 1)).unwrap().symbol(), "┘");
    }

    // ── Setup wizard unit tests ──

    fn make_setup_state() -> AppState {
        let mut state = AppState::new_setup();
        state.mode = Mode::Setup(kiosk_core::state::SetupStep::SearchDirs);
        state
    }

    /// Create a temp directory with the given subdirectory names and return
    /// (`tempdir_handle`, `base_path_with_trailing_slash`).
    fn setup_temp_dirs(names: &[&str]) -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        for name in names {
            std::fs::create_dir(tmp.path().join(name)).unwrap();
        }
        let base = format!("{}/", tmp.path().display());
        (tmp, base)
    }

    // ── Tab completion ──

    #[test]
    fn setup_tab_generates_completions_without_selection() {
        let (_tmp, base) = setup_temp_dirs(&["alpha", "beta"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = base;
        setup.input.cursor = setup.input.text.len();

        handle_setup_tab_complete(&mut state);

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.completions.len(), 2);
        assert_eq!(setup.selected_completion, None);
    }

    #[test]
    fn setup_tab_single_completion_fills_in_with_slash() {
        let (_tmp, base) = setup_temp_dirs(&["only_dir"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}on");
        setup.input.cursor = setup.input.text.len();

        // First Tab generates completions
        handle_setup_tab_complete(&mut state);
        assert_eq!(state.setup.as_ref().unwrap().completions.len(), 1);

        // Second Tab fills in the single completion
        handle_setup_tab_complete(&mut state);

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.input.text, format!("{base}only_dir/"));
    }

    #[test]
    fn setup_tab_fills_common_prefix() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop", "Development"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}D");
        setup.input.cursor = setup.input.text.len();

        // First Tab generates completions
        handle_setup_tab_complete(&mut state);
        assert_eq!(state.setup.as_ref().unwrap().completions.len(), 2);

        // Second Tab fills to common prefix
        handle_setup_tab_complete(&mut state);

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.input.text, format!("{base}De"));
        assert_eq!(setup.completions.len(), 2);
        assert_eq!(setup.selected_completion, None);
    }

    #[test]
    fn setup_tab_selects_highlighted_when_no_more_common_prefix() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop", "Development"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}De");
        setup.input.cursor = setup.input.text.len();
        setup.completions = vec![format!("{base}Desktop"), format!("{base}Development")];
        setup.selected_completion = Some(1);

        handle_setup_tab_complete(&mut state);

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.input.text, format!("{base}Development/"));
    }

    #[test]
    fn setup_tab_selects_first_when_none_highlighted() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop", "Development"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}De");
        setup.input.cursor = setup.input.text.len();
        setup.completions = vec![format!("{base}Desktop"), format!("{base}Development")];
        setup.selected_completion = None;

        handle_setup_tab_complete(&mut state);

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.input.text, format!("{base}Desktop/"));
    }

    // ── Move selection ──

    #[test]
    fn setup_move_down_from_none_selects_first() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into()];
        setup.selected_completion = None;

        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(0));
    }

    #[test]
    fn setup_move_up_from_none_selects_last() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into(), "c".into()];
        setup.selected_completion = None;

        handle_setup_move_selection(&mut state, -1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(2));
    }

    #[test]
    fn setup_move_down_increments_selection() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into(), "c".into()];
        setup.selected_completion = Some(0);

        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(1));
    }

    #[test]
    fn setup_move_down_past_last_deselects() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into()];
        setup.selected_completion = Some(1);

        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, None);
    }

    #[test]
    fn setup_move_up_from_first_deselects() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into()];
        setup.selected_completion = Some(0);

        handle_setup_move_selection(&mut state, -1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, None);
    }

    #[test]
    fn setup_move_on_empty_completions_is_noop() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = Vec::new();
        setup.selected_completion = None;

        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, None);
    }

    // ── Enter / add directory ──

    #[test]
    fn setup_enter_no_selection_adds_typed_text() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = "~/my-projects".into();
        setup.input.cursor = setup.input.text.len();
        setup.completions = vec!["~/my-projects-extra".into()];
        setup.selected_completion = None;

        let result = handle_setup_add_dir(&mut state);
        assert!(result.is_none());

        let setup = state.setup.as_ref().unwrap();
        assert!(setup.dirs.contains(&"~/my-projects".to_string()));
        assert!(setup.input.text.is_empty());
    }

    #[test]
    fn setup_enter_with_selection_navigates_into_completion() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop", "Development"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}De");
        setup.input.cursor = setup.input.text.len();
        setup.completions = vec![format!("{base}Desktop"), format!("{base}Development")];
        setup.selected_completion = Some(1);

        let result = handle_setup_add_dir(&mut state);
        assert!(result.is_none());

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.input.text, format!("{base}Development/"));
        assert!(setup.dirs.is_empty());
    }

    #[test]
    fn setup_enter_empty_with_dirs_completes_setup() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = String::new();
        setup.dirs = vec!["~/Projects".into()];

        let result = handle_setup_add_dir(&mut state);
        assert!(matches!(result, Some(OpenAction::SetupComplete)));
    }

    #[test]
    fn setup_enter_empty_without_dirs_shows_error() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = String::new();
        setup.dirs = Vec::new();

        let result = handle_setup_add_dir(&mut state);
        assert!(result.is_none());
        assert!(state.error.is_some());
        assert!(state.error.as_ref().unwrap().contains("at least one"));
    }

    #[test]
    fn setup_enter_does_not_add_duplicate_dirs() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.dirs = vec!["~/Projects".into()];
        setup.input.text = "~/Projects".into();
        setup.input.cursor = setup.input.text.len();

        handle_setup_add_dir(&mut state);

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.dirs.len(), 1);
    }

    // ── Typing clears selection ──

    #[test]
    fn setup_typing_clears_selection() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}D");
        setup.input.cursor = setup.input.text.len();
        setup.completions = vec![format!("{base}Desktop")];
        setup.selected_completion = Some(0);

        let matcher = SkimMatcherV2::default();
        handle_search_push(&mut state, &matcher, 'e');

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.selected_completion, None);
        assert!(setup.input.text.ends_with("De"));
    }

    #[test]
    fn setup_typing_updates_completions() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop", "Development", "Documents"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}D");
        setup.input.cursor = setup.input.text.len();

        let matcher = SkimMatcherV2::default();
        handle_search_push(&mut state, &matcher, 'e');

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.completions.len(), 2);
        let names: Vec<&str> = setup
            .completions
            .iter()
            .map(std::string::String::as_str)
            .collect();
        assert!(names.iter().any(|n| n.contains("Desktop")));
        assert!(names.iter().any(|n| n.contains("Development")));
    }

    #[test]
    fn setup_backspace_updates_completions() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop", "Development", "Documents"]);

        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.input.text = format!("{base}De");
        setup.input.cursor = setup.input.text.len();

        let matcher = SkimMatcherV2::default();
        handle_search_pop(&mut state, &matcher);

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.completions.len(), 3);
        assert_eq!(setup.selected_completion, None);
    }

    // ── Welcome / continue ──

    #[test]
    fn setup_continue_transitions_to_search_dirs() {
        let mut state = AppState::new_setup();
        assert_eq!(
            state.mode,
            Mode::Setup(kiosk_core::state::SetupStep::Welcome)
        );

        handle_setup_continue(&mut state);
        assert_eq!(
            state.mode,
            Mode::Setup(kiosk_core::state::SetupStep::SearchDirs)
        );
        assert!(state.setup.is_some());
    }

    #[test]
    fn setup_continue_preserves_existing_state() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.dirs.push("~/existing".to_string());

        handle_setup_continue(&mut state);
        assert_eq!(
            state.mode,
            Mode::Setup(kiosk_core::state::SetupStep::SearchDirs)
        );
        assert_eq!(state.setup.as_ref().unwrap().dirs.len(), 1);
    }

    // ── Full flow integration ──

    #[test]
    fn setup_full_flow_type_navigate_enter_finish() {
        let (_tmp, base) = setup_temp_dirs(&["Desktop", "Development"]);

        let mut state = make_setup_state();
        let matcher = SkimMatcherV2::default();

        // Type the base path + "De"
        for c in format!("{base}De").chars() {
            handle_search_push(&mut state, &matcher, c);
        }

        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.completions.len(), 2);
        assert_eq!(setup.selected_completion, None);

        // Navigate down to highlight first completion
        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(0));

        // Navigate down again to highlight second completion (Development)
        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(1));

        // Press Enter to navigate into Development/
        handle_setup_add_dir(&mut state);
        let setup = state.setup.as_ref().unwrap();
        assert!(setup.input.text.ends_with("Development/"));
        assert!(setup.dirs.is_empty());

        // Clear input and type the path directly, then add it
        let setup = state.setup.as_mut().unwrap();
        setup.input.clear();
        setup.completions.clear();
        setup.selected_completion = None;
        setup.input.text = format!("{base}Development");
        setup.input.cursor = setup.input.text.len();

        handle_setup_add_dir(&mut state);
        let setup = state.setup.as_ref().unwrap();
        assert_eq!(setup.dirs.len(), 1);
        assert!(setup.dirs[0].ends_with("Development"));

        // Empty Enter to finish
        let result = handle_setup_add_dir(&mut state);
        assert!(matches!(result, Some(OpenAction::SetupComplete)));
    }

    // ── Cancel / Escape ──

    #[test]
    fn setup_cancel_deselects_when_selected() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into()];
        setup.selected_completion = Some(1);

        let result = handle_setup_cancel(&mut state);
        assert!(result.is_none());
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, None);
    }

    #[test]
    fn setup_cancel_quits_when_no_selection() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into()];
        setup.selected_completion = None;

        let result = handle_setup_cancel(&mut state);
        assert!(matches!(result, Some(OpenAction::Quit)));
    }

    #[test]
    fn setup_cancel_quits_when_no_completions() {
        let mut state = make_setup_state();

        let result = handle_setup_cancel(&mut state);
        assert!(matches!(result, Some(OpenAction::Quit)));
    }

    // ── Down/Up wrap-around deselection ──

    #[test]
    fn setup_move_down_up_cycle_through_and_back() {
        let mut state = make_setup_state();
        let setup = state.setup.as_mut().unwrap();
        setup.completions = vec!["a".into(), "b".into()];
        setup.selected_completion = None;

        // Down from None → first
        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(0));

        // Down → second
        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(1));

        // Down past last → None (back to text)
        handle_setup_move_selection(&mut state, 1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, None);

        // Up from None → last
        handle_setup_move_selection(&mut state, -1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(1));

        // Up → first
        handle_setup_move_selection(&mut state, -1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, Some(0));

        // Up past first → None (back to text)
        handle_setup_move_selection(&mut state, -1);
        assert_eq!(state.setup.as_ref().unwrap().selected_completion, None);
    }

    // ── GitFetchCompleted tests ──

    fn make_branch(name: &str, is_remote: bool) -> BranchEntry {
        BranchEntry {
            name: name.to_string(),
            worktree_path: None,
            has_session: false,
            is_current: false,
            is_default: false,
            is_remote,
            session_activity_ts: None,
        }
    }

    #[test]
    fn test_git_fetch_completed_adds_new_remote_branches() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![make_branch("main", false)];
        state.branch_list.reset(1);
        state.fetching_remotes = true;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::GitFetchCompleted {
                branches: vec![make_branch("feature-new", true)],
                repo_path: PathBuf::from("/tmp/alpha"),
                error: None,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert!(!state.fetching_remotes);
        assert_eq!(state.branches.len(), 2);
        assert_eq!(state.branches[1].name, "feature-new");
        assert!(state.branches[1].is_remote);
        assert_eq!(state.branch_list.filtered.len(), 2);
    }

    #[test]
    fn test_git_fetch_completed_deduplicates() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![
            make_branch("main", false),
            make_branch("existing-remote", true),
        ];
        state.branch_list.reset(2);
        state.fetching_remotes = true;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::GitFetchCompleted {
                branches: vec![
                    make_branch("existing-remote", true),
                    make_branch("brand-new", true),
                ],
                repo_path: PathBuf::from("/tmp/alpha"),
                error: None,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert_eq!(state.branches.len(), 3);
        let names: Vec<&str> = state.branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "existing-remote", "brand-new"]);
    }

    #[test]
    fn test_git_fetch_completed_preserves_search() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![make_branch("main", false)];
        state.branch_list.reset(1);
        state.branch_list.input.text = "feat".to_string();
        state.branch_list.input.cursor = 4;
        state.branch_list.filtered = vec![];
        state.branch_list.selected = None;
        state.fetching_remotes = true;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::GitFetchCompleted {
                branches: vec![make_branch("feature-x", true)],
                repo_path: PathBuf::from("/tmp/alpha"),
                error: None,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert_eq!(state.branch_list.input.text, "feat");
        assert_eq!(state.branches.len(), 2);
        // "feat" should match "feature-x" but not "main"
        assert_eq!(state.branch_list.filtered.len(), 1);
        let matched_idx = state.branch_list.filtered[0].0;
        assert_eq!(state.branches[matched_idx].name, "feature-x");
    }

    #[test]
    fn test_git_fetch_completed_ignored_wrong_mode() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::RepoSelect;
        state.branches = vec![make_branch("main", false)];
        state.fetching_remotes = true;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::GitFetchCompleted {
                branches: vec![make_branch("feature-x", true)],
                repo_path: PathBuf::from("/tmp/alpha"),
                error: None,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // Should be ignored since we're not in BranchSelect
        assert_eq!(state.branches.len(), 1);
        // fetching_remotes stays true because the event was ignored (wrong mode)
        assert!(state.fetching_remotes);
    }

    // ── Streaming discovery event tests ──

    #[test]
    fn test_repos_found_deduplicates_cwd_repo() {
        // CWD repo is pre-loaded; scan finds it again — should not duplicate
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::RepoSelect;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("alpha"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );
        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("beta"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert_eq!(state.repos.len(), 2, "alpha should not be duplicated");
        assert!(state.repos.iter().any(|r| r.name == "alpha"));
        assert!(state.repos.iter().any(|r| r.name == "beta"));
    }

    #[test]
    fn test_git_fetch_completed_ignored_wrong_repo() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.branches = vec![make_branch("main", false)];
        state.branch_list.reset(1);
        state.fetching_remotes = true;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::GitFetchCompleted {
                branches: vec![make_branch("feature-x", true)],
                repo_path: PathBuf::from("/tmp/wrong-repo"),
                error: None,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // Stale event should leave state untouched
        assert!(state.fetching_remotes);
        assert_eq!(state.branches.len(), 1);
    }

    #[test]
    fn test_git_fetch_completed_shows_error() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.selected_repo_idx = Some(0);
        state.mode = Mode::BranchSelect;
        state.fetching_remotes = true;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::GitFetchCompleted {
                branches: vec![],
                repo_path: PathBuf::from("/tmp/alpha"),
                error: Some("network unreachable".to_string()),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert!(!state.fetching_remotes);
        assert!(state.error.is_some());
        assert!(
            state
                .error
                .as_ref()
                .unwrap()
                .contains("network unreachable")
        );
    }

    #[test]
    fn test_repos_found_does_not_kick_from_branch_select() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.selected_repo_idx = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("beta"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert_eq!(state.mode, Mode::BranchSelect);
        assert_eq!(state.repos.len(), 2);
    }

    #[test]
    fn test_fetching_remotes_flag_lifecycle() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.repo_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".into()],
            ..Default::default()
        });
        let tmux: Arc<dyn TmuxProvider> = Arc::new(MockTmuxProvider::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = EventSender {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
        };

        // Enter branch select
        enter_branch_select(&mut state, 0, &git, &tmux, &sender);
        assert!(!state.fetching_remotes);

        // Process BranchesLoaded — should set fetching_remotes
        let event = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        process_app_event(event, &mut state, &git, &tmux, &sender);
        assert!(
            state.fetching_remotes,
            "fetching_remotes should be true after BranchesLoaded"
        );

        // Process GitFetchCompleted — should clear it
        process_app_event(
            AppEvent::GitFetchCompleted {
                branches: vec![],
                repo_path: PathBuf::from("/tmp/alpha"),
                error: None,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );
        assert!(
            !state.fetching_remotes,
            "fetching_remotes should be false after GitFetchCompleted"
        );
    }

    #[test]
    fn test_repos_found_preserves_search_state() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::RepoSelect;
        state.repo_list.input.text = "al".to_string();
        state.repo_list.input.cursor = 2;

        let matcher = SkimMatcherV2::default();
        state.repo_list.filtered = state
            .repos
            .iter()
            .enumerate()
            .filter_map(|(i, r)| matcher.fuzzy_match(&r.name, "al").map(|score| (i, score)))
            .collect();
        state.repo_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("delta"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );
        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("beta"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );
        assert_eq!(state.repo_list.input.text, "al");
        assert_eq!(state.repo_list.input.cursor, 2);
        assert_eq!(state.repos.len(), 3);
        // "alpha" and "delta" match "al"
        assert!(!state.repo_list.filtered.is_empty());
    }

    #[test]
    fn test_repos_found_multiple_batches_build_list() {
        let mut state = AppState::new(vec![], None);
        state.mode = Mode::Loading("Discovering repos...".into());

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        // First repo
        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("alpha"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );
        assert_eq!(state.repos.len(), 1);
        assert_eq!(state.mode, Mode::RepoSelect);

        // Second repo
        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("beta"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );
        assert_eq!(state.repos.len(), 2);

        // Third repo
        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("gamma"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );
        assert_eq!(state.repos.len(), 3);
    }

    #[test]
    fn test_scan_complete_collision_resolution() {
        let mut state = AppState::new(vec![], None);
        state.mode = Mode::RepoSelect;

        // Two repos with same name from different search dirs
        let mut repo1 = make_repo("api");
        repo1.path = PathBuf::from("/home/user/work/api");
        let mut repo2 = make_repo("api");
        repo2.path = PathBuf::from("/home/user/personal/api");
        state.repos = vec![repo1, repo2];
        state.repo_list.reset(2);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::ScanComplete {
                search_dirs: vec![
                    (PathBuf::from("/home/user/work"), 1),
                    (PathBuf::from("/home/user/personal"), 1),
                ],
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        let session_names: Vec<&str> = state
            .repos
            .iter()
            .map(|r| r.session_name.as_str())
            .collect();
        assert_eq!(session_names.len(), 2);
        // Both should be disambiguated with different search dir names
        let unique: std::collections::HashSet<&str> = session_names.iter().copied().collect();
        assert_eq!(
            unique.len(),
            2,
            "session names should be unique: {session_names:?}"
        );
        assert!(
            session_names.iter().any(|n| n.contains("work")),
            "should contain search dir name 'work': {session_names:?}"
        );
        assert!(
            session_names.iter().any(|n| n.contains("personal")),
            "should contain search dir name 'personal': {session_names:?}"
        );
    }

    #[test]
    fn test_scan_complete_no_collisions_preserves_names() {
        let mut state = AppState::new(vec![make_repo("alpha"), make_repo("beta")], None);
        state.mode = Mode::RepoSelect;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::ScanComplete {
                search_dirs: vec![(PathBuf::from("/home/user/dev"), 1)],
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert_eq!(state.repos[0].session_name, "alpha");
        assert_eq!(state.repos[1].session_name, "beta");
    }

    #[test]
    fn test_repo_enriched_unknown_repo_is_noop() {
        let mut state = AppState::new(vec![make_repo("alpha")], None);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        // Enrichment for a repo not in state
        process_app_event(
            AppEvent::RepoEnriched {
                repo_path: PathBuf::from("/tmp/nonexistent"),
                worktrees: vec![Worktree {
                    path: PathBuf::from("/tmp/nonexistent"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // Should not panic or add anything
        assert_eq!(state.repos.len(), 1);
        assert_eq!(state.repos[0].name, "alpha");
    }

    #[test]
    fn test_repo_enriched_updates_worktrees_without_resorting() {
        let mut state = AppState::new(vec![make_repo("beta"), make_repo("alpha")], None);
        // Manually set order so we can verify it doesn't change
        let original_order: Vec<String> = state.repos.iter().map(|r| r.name.clone()).collect();

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::RepoEnriched {
                repo_path: PathBuf::from("/tmp/alpha"),
                worktrees: vec![
                    Worktree {
                        path: PathBuf::from("/tmp/alpha"),
                        branch: Some("main".to_string()),
                        is_main: true,
                    },
                    Worktree {
                        path: PathBuf::from("/tmp/alpha-dev"),
                        branch: Some("dev".to_string()),
                        is_main: false,
                    },
                ],
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // Worktrees updated
        let alpha = state.repos.iter().find(|r| r.name == "alpha").unwrap();
        assert_eq!(alpha.worktrees.len(), 2);

        // Order preserved (no re-sort)
        let current_order: Vec<String> = state.repos.iter().map(|r| r.name.clone()).collect();
        assert_eq!(current_order, original_order);
    }

    #[test]
    fn test_session_activity_loaded_resorts_and_preserves_selection() {
        let repos = vec![make_repo("alpha"), make_repo("beta"), make_repo("gamma")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::RepoSelect;
        state.repo_list.selected = Some(1); // beta selected
        state.selected_repo_idx = Some(1);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        let mut activity = std::collections::HashMap::new();
        activity.insert("gamma".to_string(), 9999);

        process_app_event(
            AppEvent::SessionActivityLoaded {
                session_activity: activity,
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        // Activity should be stored
        assert_eq!(state.session_activity.get("gamma"), Some(&9999));

        // Selection should still point to beta
        let selected_name = state
            .selected_repo_idx
            .and_then(|idx| state.repos.get(idx))
            .map(|r| r.name.as_str());
        assert_eq!(selected_name, Some("beta"));
    }

    #[test]
    fn test_session_activity_loaded_empty_state_no_panic() {
        let mut state = AppState::new(vec![], None);
        state.mode = Mode::RepoSelect;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        // Should not panic with empty repos
        process_app_event(
            AppEvent::SessionActivityLoaded {
                session_activity: std::collections::HashMap::new(),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert!(state.repos.is_empty());
    }

    #[test]
    fn test_repos_found_switches_from_loading_to_repo_select() {
        let mut state = AppState::new(vec![], None);
        state.mode = Mode::Loading("Discovering repos...".into());

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = Arc::new(MockTmuxProvider::default());
        let sender = make_sender();

        process_app_event(
            AppEvent::ReposFound {
                repo: make_repo("alpha"),
            },
            &mut state,
            &git,
            &tmux,
            &sender,
        );

        assert_eq!(state.mode, Mode::RepoSelect);
    }
}
