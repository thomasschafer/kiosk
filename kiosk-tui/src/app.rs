use crate::{components, keymap};
use crossterm::event::{self, Event, KeyEventKind};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use kiosk_core::{
    action::Action,
    config::{KeysConfig, keys::Command},
    event::AppEvent,
    git::GitProvider,
    state::{AppState, BranchEntry, Mode, NewBranchFlow, SearchableList, worktree_dir},
    tmux::TmuxProvider,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// What to do after the TUI exits
pub enum OpenAction {
    Open {
        path: PathBuf,
        session_name: String,
        split_command: Option<String>,
    },
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
    tmux: &dyn TmuxProvider,
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

    // Start repo discovery in background if repos are empty
    if state.repos.is_empty() {
        spawn_repo_discovery(git, &event_sender, search_dirs);
    }

    loop {
        terminal.draw(|f| draw(f, state, theme, keys, &spinner_start))?;

        // Check background channel (non-blocking)
        if let Ok(app_event) = rx.try_recv() {
            if let Some(result) = process_app_event(app_event, state, git.as_ref(), tmux) {
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
            state.error = None;

            if let Some(action) = keymap::resolve_action(key, state, keys)
                && let Some(result) =
                    process_action(action, state, git, tmux, &matcher, &event_sender)
            {
                return Ok(Some(result));
            }
        }
    }
}

fn draw(
    f: &mut Frame,
    state: &AppState,
    theme: &crate::theme::Theme,
    keys: &kiosk_core::config::KeysConfig,
    spinner_start: &Instant,
) {
    // Loading mode: full-screen spinner
    if let Mode::Loading(ref msg) = state.mode {
        draw_loading(f, f.area(), msg, theme, spinner_start);
        return;
    }

    let (main_area, error_area) = if state.error.is_some() {
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(f.area());
        (chunks[0], Some(chunks[1]))
    } else {
        (f.area(), None)
    };

    match &state.mode {
        Mode::RepoSelect => components::repo_list::draw(f, main_area, state, theme, keys),
        Mode::BranchSelect => components::branch_picker::draw(f, main_area, state, theme, keys),
        Mode::NewBranchBase => {
            components::branch_picker::draw(f, main_area, state, theme, keys);
            components::new_branch::draw(f, state, theme);
        }
        Mode::ConfirmDelete { .. } => {
            components::branch_picker::draw(f, main_area, state, theme, keys);
            draw_confirm_delete_dialog(f, main_area, state, theme, keys);
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
                Mode::NewBranchBase => {
                    components::branch_picker::draw(f, main_area, state, theme, keys);
                    components::new_branch::draw(f, state, theme);
                }
                Mode::ConfirmDelete { .. } => {
                    components::branch_picker::draw(f, main_area, state, theme, keys);
                    draw_confirm_delete_dialog(f, main_area, state, theme, keys);
                }
                _ => {}
            }
            // Draw help overlay on top
            components::help::draw(f, state, theme, keys);
        }
        Mode::Loading(_) => unreachable!(),
    }

    if let Some(area) = error_area {
        components::error_bar::draw(f, area, state);
    }
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

    let text = Line::from(vec![
        Span::styled(
            format!("{spinner} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(message),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    // Centre vertically
    let vertical = Layout::vertical([
        Constraint::Percentage(45),
        Constraint::Length(3),
        Constraint::Percentage(45),
    ])
    .split(area);

    let horizontal = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(50),
        Constraint::Percentage(25),
    ])
    .split(vertical[1]);

    let paragraph = Paragraph::new(text)
        .block(block)
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(paragraph, horizontal[1]);
}

fn draw_confirm_delete_dialog(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &crate::theme::Theme,
    keys: &kiosk_core::config::KeysConfig,
) {
    if let Mode::ConfirmDelete {
        branch_name,
        has_session,
    } = &state.mode
    {
        let action_text = if *has_session {
            "Delete worktree and kill tmux session for branch "
        } else {
            "Delete worktree for branch "
        };

        let confirm_key = KeysConfig::find_key(&keys.confirmation, &Command::Confirm)
            .map_or("y".to_string(), |k| k.to_string());
        let cancel_key = KeysConfig::find_key(&keys.confirmation, &Command::Cancel)
            .map_or("Esc".to_string(), |k| k.to_string());

        let text = vec![
            Line::from(vec![
                Span::raw(action_text),
                Span::styled(
                    format!("\"{branch_name}\""),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("?"),
            ]),
            Line::raw(""),
            Line::from(vec![
                Span::styled(&confirm_key, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" / "),
                Span::styled(&cancel_key, Style::default().add_modifier(Modifier::BOLD)),
            ]),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Confirm Delete ")
            .border_style(Style::default().fg(theme.accent));

        // Centre the dialog
        let vertical = Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Length(5),
            Constraint::Percentage(40),
        ])
        .split(area);

        let horizontal = Layout::horizontal([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(vertical[1]);

        let paragraph = Paragraph::new(text)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(paragraph, horizontal[1]);
    }
}

/// Handle events from background tasks
fn process_app_event(
    event: AppEvent,
    state: &mut AppState,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
) -> Option<OpenAction> {
    match event {
        AppEvent::ReposDiscovered { repos } => {
            state.repo_list.reset(repos.len());
            state.repos = repos;
            state.mode = Mode::RepoSelect;
        }
        AppEvent::WorktreeCreated { path, session_name } => {
            return Some(OpenAction::Open {
                path,
                session_name,
                split_command: state.split_command.clone(),
            });
        }
        AppEvent::WorktreeRemoved { branch_name: _ } => {
            // Return to branch select and refresh the branch list
            if let Some(repo_idx) = state.selected_repo_idx {
                enter_branch_select(state, repo_idx, git, tmux);
            } else {
                state.mode = Mode::BranchSelect;
            }
        }
        AppEvent::GitError(msg) => {
            // Return to the appropriate mode
            if state.new_branch_base.is_some() {
                state.new_branch_base = None;
                state.mode = Mode::BranchSelect;
            } else {
                state.mode = Mode::BranchSelect;
            }
            state.error = Some(msg);
        }
    }
    None
}

fn spawn_repo_discovery(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    search_dirs: Vec<(std::path::PathBuf, u16)>,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        // Check if cancelled before starting
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        let repos = git.discover_repos(&search_dirs);
        sender.send(AppEvent::ReposDiscovered { repos });
    });
}

fn spawn_worktree_creation(
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
        // Check if cancelled before starting
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

fn spawn_worktree_removal(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    worktree_path: PathBuf,
    branch_name: String,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        // Check if cancelled before starting
        if sender.cancel.load(Ordering::Relaxed) {
            return;
        }
        match git.remove_worktree(&worktree_path) {
            Ok(()) => sender.send(AppEvent::WorktreeRemoved { branch_name }),
            Err(e) => sender.send(AppEvent::GitError(format!("{e}"))),
        }
    });
}

fn spawn_branch_and_worktree_creation(
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
        // Check if cancelled before starting
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

/// Handle movement-related actions via `SearchableList` methods
fn handle_movement_actions(action: &Action, state: &mut AppState) -> bool {
    let Some(list) = state.active_list_mut() else {
        return false;
    };
    let list_len: i32 = list.filtered.len().try_into().unwrap_or(i32::MAX);
    match action {
        Action::HalfPageUp => list.move_selection(-list_len.min(10)),
        Action::HalfPageDown => list.move_selection(list_len.min(10)),
        Action::PageUp => list.move_selection(-list_len.min(25)),
        Action::PageDown => list.move_selection(list_len.min(25)),
        Action::MoveTop => list.move_to_top(),
        Action::MoveBottom => list.move_to_bottom(),
        _ => return false,
    }
    true
}

/// Handle simple cursor and error actions
fn handle_simple_actions(action: &Action, state: &mut AppState) -> bool {
    match action {
        Action::CursorLeft => {
            if let Some(list) = state.active_list_mut() {
                list.cursor_left();
            }
            true
        }
        Action::CursorRight => {
            if let Some(list) = state.active_list_mut() {
                list.cursor_right();
            }
            true
        }
        Action::CursorStart => {
            if let Some(list) = state.active_list_mut() {
                list.cursor_start();
            }
            true
        }
        Action::CursorEnd => {
            if let Some(list) = state.active_list_mut() {
                list.cursor_end();
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

#[allow(clippy::needless_pass_by_value)]
fn process_action(
    action: Action,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    tmux: &dyn TmuxProvider,
    matcher: &SkimMatcherV2,
    sender: &EventSender,
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
                enter_branch_select(state, idx, git.as_ref(), tmux);
            }
        }

        Action::GoBack => handle_go_back(state),

        Action::OpenBranch => {
            if let Some(result) = handle_open_branch(state, git, sender) {
                return Some(result);
            }
        }

        Action::StartNewBranchFlow => {
            handle_start_new_branch(state, git);
        }

        Action::MoveSelection(delta) => {
            if let Some(list) = state.active_list_mut() {
                list.move_selection(delta);
            }
        }

        Action::SearchPush(c) => handle_search_push(state, matcher, c),
        Action::SearchPop => handle_search_pop(state, matcher),

        Action::DeleteWorktree => handle_delete_worktree(state),
        Action::ConfirmDeleteWorktree => handle_confirm_delete(state, git, tmux, sender),

        Action::SearchDeleteWord => handle_search_delete_word(state, matcher),

        Action::ShowHelp => handle_show_help(state),

        // Actions handled before the match statement
        Action::HalfPageUp
        | Action::HalfPageDown
        | Action::PageUp
        | Action::PageDown
        | Action::MoveTop
        | Action::MoveBottom
        | Action::CursorLeft
        | Action::CursorRight
        | Action::CursorStart
        | Action::CursorEnd
        | Action::CancelDeleteWorktree => {
            // These are handled by helper functions before this match
        }
    }

    None
}

fn handle_go_back(state: &mut AppState) {
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

fn handle_show_help(state: &mut AppState) {
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

fn handle_start_new_branch(state: &mut AppState, git: &Arc<dyn GitProvider>) {
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

fn handle_delete_worktree(state: &mut AppState) {
    if let Some(sel) = state.branch_list.selected
        && let Some(&(idx, _)) = state.branch_list.filtered.get(sel)
    {
        let branch = &state.branches[idx];
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

fn handle_confirm_delete(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    tmux: &dyn TmuxProvider,
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
            state.mode = Mode::Loading(format!("Removing worktree for {branch_name}..."));
            spawn_worktree_removal(git, sender, worktree_path, branch_name);
        }
    }
}

fn handle_open_branch(
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
                match worktree_dir(repo, &branch.name) {
                    Ok(wt_path) => {
                        let branch_name = branch.name.clone();
                        let session_name = repo.tmux_session_name(&wt_path);
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

fn enter_branch_select(
    state: &mut AppState,
    repo_idx: usize,
    git: &dyn GitProvider,
    tmux: &dyn TmuxProvider,
) {
    state.selected_repo_idx = Some(repo_idx);
    state.mode = Mode::BranchSelect;

    let repo = &state.repos[repo_idx];
    let sessions = tmux.list_sessions();
    let all_branches = git.list_branches(&repo.path);

    let wt_by_branch: std::collections::HashMap<&str, &kiosk_core::git::Worktree> = repo
        .worktrees
        .iter()
        .filter_map(|wt| wt.branch.as_deref().map(|b| (b, wt)))
        .collect();

    state.branches = all_branches
        .iter()
        .map(|branch_name| {
            let worktree_path = wt_by_branch
                .get(branch_name.as_str())
                .map(|wt| wt.path.clone());
            let has_session = worktree_path
                .as_ref()
                .is_some_and(|p| sessions.contains(&repo.tmux_session_name(p)));
            let is_current = repo.worktrees.first().and_then(|wt| wt.branch.as_deref())
                == Some(branch_name.as_str());

            BranchEntry {
                name: branch_name.clone(),
                worktree_path,
                has_session,
                is_current,
            }
        })
        .collect();

    // Sort: branches with sessions first, then with worktrees, then alphabetical
    state.branches.sort_by(|a, b| {
        b.has_session
            .cmp(&a.has_session)
            .then(b.worktree_path.is_some().cmp(&a.worktree_path.is_some()))
            .then(a.name.cmp(&b.name))
    });

    state.branch_list.reset(state.branches.len());
}

/// Handle search character push action
fn handle_search_push(state: &mut AppState, matcher: &SkimMatcherV2, c: char) {
    if let Some(list) = state.active_list_mut() {
        list.insert_char(c);
    }
    update_active_filter(state, matcher);
}

/// Handle search character pop action
fn handle_search_pop(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.backspace();
    }
    update_active_filter(state, matcher);
}

/// Handle search delete word action
fn handle_search_delete_word(state: &mut AppState, matcher: &SkimMatcherV2) {
    if let Some(list) = state.active_list_mut() {
        list.delete_word();
    }
    update_active_filter(state, matcher);
}

/// Update the fuzzy filter for the active mode's list
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

/// Apply fuzzy filter to a `SearchableList` against a set of item names
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

/// Handle move selection action for different modes

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::git::mock::MockGitProvider;
    use kiosk_core::git::{Repo, Worktree};
    use kiosk_core::state::{AppState, BranchEntry, Mode};
    use kiosk_core::tmux::mock::MockTmuxProvider;

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

    #[test]
    fn test_enter_repo_populates_branches() {
        let repos = vec![make_repo("alpha"), make_repo("beta")];
        let mut state = AppState::new(repos, None);
        state.repo_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".into(), "dev".into()],
            ..Default::default()
        });
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        let result = process_action(
            Action::EnterRepo,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert!(result.is_none());
        assert_eq!(state.mode, Mode::BranchSelect);
        assert_eq!(state.branches.len(), 2);
    }

    #[test]
    fn test_go_back_from_branch_to_repo() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(Action::GoBack, &mut state, &git, &tmux, &matcher, &sender);
        assert_eq!(state.mode, Mode::RepoSelect);
    }

    #[test]
    fn test_go_back_from_new_branch_to_branch() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::NewBranchBase;
        state.new_branch_base = Some(kiosk_core::state::NewBranchFlow {
            new_name: "feat".into(),
            bases: vec!["main".into()],
            list: SearchableList::new(1),
        });

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(Action::GoBack, &mut state, &git, &tmux, &matcher, &sender);
        assert_eq!(state.mode, Mode::BranchSelect);
        assert!(state.new_branch_base.is_none());
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
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        let result = process_action(
            Action::OpenBranch,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert!(result.is_some());
        match result.unwrap() {
            OpenAction::Open {
                path, session_name, ..
            } => {
                assert_eq!(path, PathBuf::from("/tmp/alpha"));
                assert_eq!(session_name, "alpha");
            }
            _ => panic!("Expected OpenAction::Open"),
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
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        let result = process_action(
            Action::OpenBranch,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert!(result.is_none());
        assert!(matches!(state.mode, Mode::Loading(_)));
    }

    #[test]
    fn test_search_push_filters() {
        let repos = vec![make_repo("alpha"), make_repo("beta")];
        let mut state = AppState::new(repos, None);
        assert_eq!(state.repo_list.filtered.len(), 2);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::SearchPush('a'),
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.search, "a");
        // "alpha" matches "a", "beta" also matches "a" — but both should be present
        assert!(!state.repo_list.filtered.is_empty());
    }

    #[test]
    fn test_move_selection() {
        let repos = vec![make_repo("alpha"), make_repo("beta"), make_repo("gamma")];
        let mut state = AppState::new(repos, None);
        assert_eq!(state.repo_list.selected, Some(0));

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::MoveSelection(1),
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.selected, Some(1));

        process_action(
            Action::MoveSelection(1),
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.selected, Some(2));

        // Should clamp at max
        process_action(
            Action::MoveSelection(1),
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.selected, Some(2));
    }

    #[test]
    fn test_open_repo_returns_repo_path() {
        let repos = vec![make_repo("alpha"), make_repo("beta")];
        let mut state = AppState::new(repos, Some("hx".into()));
        state.repo_list.selected = Some(1);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        let result = process_action(Action::OpenRepo, &mut state, &git, &tmux, &matcher, &sender);
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
            _ => panic!("Expected OpenAction::Open"),
        }
    }

    #[test]
    fn test_new_branch_empty_name_shows_error() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.mode = Mode::BranchSelect;
        state.selected_repo_idx = Some(0);
        state.branch_list.search = String::new(); // empty

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".into()],
            ..Default::default()
        });
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::StartNewBranchFlow,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

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
        state.branch_list.search = "feat/new".to_string();

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider {
            branches: vec!["main".into()],
            ..Default::default()
        });
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::StartNewBranchFlow,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

        assert_eq!(state.mode, Mode::NewBranchBase);
        assert!(state.new_branch_base.is_some());
        assert_eq!(state.new_branch_base.unwrap().new_name, "feat/new");
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
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::DeleteWorktree,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

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
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::DeleteWorktree,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

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
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::DeleteWorktree,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

        assert_eq!(
            state.mode,
            Mode::ConfirmDelete {
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
        }];
        state.branch_list.filtered = vec![(0, 0)];
        state.branch_list.selected = Some(0);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::DeleteWorktree,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

        assert_eq!(
            state.mode,
            Mode::ConfirmDelete {
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
        state.mode = Mode::ConfirmDelete {
            branch_name: "dev".to_string(),
            has_session: true,
        };
        state.branches = vec![BranchEntry {
            name: "dev".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/alpha-dev")),
            has_session: true,
            is_current: false,
        }];

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::ConfirmDeleteWorktree,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

        let killed = tmux.killed_sessions.borrow();
        assert_eq!(killed.as_slice(), &["alpha-dev"]);
        assert!(matches!(state.mode, Mode::Loading(_)));
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
        state.mode = Mode::ConfirmDelete {
            branch_name: "dev".to_string(),
            has_session: false,
        };
        state.branches = vec![BranchEntry {
            name: "dev".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/alpha-dev")),
            has_session: false,
            is_current: false,
        }];

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        process_action(
            Action::ConfirmDeleteWorktree,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );

        let killed = tmux.killed_sessions.borrow();
        assert!(killed.is_empty());
        assert!(matches!(state.mode, Mode::Loading(_)));
    }

    #[test]
    fn test_cursor_movement_multibyte() {
        // "café" = 5 bytes: c(1) a(1) f(1) é(2)
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.repo_list.search = "café".to_string();
        state.repo_list.cursor = state.repo_list.search.len(); // 5 (byte len)

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        // Move left from end should skip over the 2-byte 'é'
        process_action(
            Action::CursorLeft,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 3); // before 'é' (byte offset of 'é')

        // Move left again should land before 'f'
        process_action(
            Action::CursorLeft,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 2);

        // Move right should skip over 'f' (1 byte)
        process_action(
            Action::CursorRight,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 3);

        // Move right should skip over 'é' (2 bytes)
        process_action(
            Action::CursorRight,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 5);
    }

    #[test]
    fn test_backspace_multibyte() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.repo_list.search = "café".to_string();
        state.repo_list.cursor = state.repo_list.search.len(); // 5

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        // Backspace should remove 'é' (2 bytes)
        process_action(
            Action::SearchPop,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.search, "caf");
        assert_eq!(state.repo_list.cursor, 3);
    }

    #[test]
    fn test_cursor_movement_in_search() {
        let repos = vec![make_repo("alpha")];
        let mut state = AppState::new(repos, None);
        state.repo_list.search = "hello".to_string();
        state.repo_list.cursor = 5; // at end

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let matcher = SkimMatcherV2::default();
        let sender = make_sender();

        // Move cursor left
        process_action(
            Action::CursorLeft,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 4);

        // Move cursor to start
        process_action(
            Action::CursorStart,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 0);

        // Move cursor to end
        process_action(
            Action::CursorEnd,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 5);

        // Move cursor right at end stays at end
        process_action(
            Action::CursorRight,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_list.cursor, 5);
    }
}
