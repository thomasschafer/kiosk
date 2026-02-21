mod actions;
mod spawn;

use crate::{components, keymap};
use actions::{
    enter_branch_select, handle_confirm_delete, handle_delete_worktree, handle_go_back,
    handle_open_branch, handle_search_delete_word, handle_search_pop, handle_search_push,
    handle_show_help, handle_start_new_branch,
};
use crossterm::event::{self, Event, KeyEventKind};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use kiosk_core::{
    action::Action,
    config::{KeysConfig, keys::Command},
    event::AppEvent,
    git::GitProvider,
    state::{AppState, Mode},
    tmux::TmuxProvider,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use spawn::spawn_repo_discovery;
use std::{
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
    git: &Arc<dyn GitProvider>,
    tmux: &dyn TmuxProvider,
    sender: &EventSender,
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
                enter_branch_select(state, repo_idx, git, tmux, sender);
            } else {
                state.mode = Mode::BranchSelect;
            }
        }
        AppEvent::BranchesLoaded {
            branches,
            local_names,
        } => {
            state.branches = branches;
            state.branch_list.reset(state.branches.len());
            state.mode = Mode::BranchSelect;

            // Kick off remote branch loading
            if let Some(repo_idx) = state.selected_repo_idx {
                let repo_path = state.repos[repo_idx].path.clone();
                spawn::spawn_remote_branch_loading(git, sender, repo_path, local_names);
            }
        }
        AppEvent::RemoteBranchesLoaded { branches } => {
            if state.mode == Mode::BranchSelect {
                // Preserve current search/selection state
                let prev_search = state.branch_list.search.clone();
                let prev_cursor = state.branch_list.cursor;

                state.branches.extend(branches);
                // Re-apply filter with the expanded branch list
                if prev_search.is_empty() {
                    state.branch_list.filtered = state
                        .branches
                        .iter()
                        .enumerate()
                        .map(|(i, _)| (i, 0))
                        .collect();
                } else {
                    // Re-run fuzzy filter
                    let names: Vec<String> =
                        state.branches.iter().map(|b| b.name.clone()).collect();
                    let matcher = SkimMatcherV2::default();
                    let mut scored: Vec<(usize, i64)> = names
                        .iter()
                        .enumerate()
                        .filter_map(|(i, item)| {
                            matcher
                                .fuzzy_match(item, &prev_search)
                                .map(|score| (i, score))
                        })
                        .collect();
                    scored.sort_by(|a, b| b.1.cmp(&a.1));
                    state.branch_list.filtered = scored;
                }
                state.branch_list.cursor = prev_cursor;
                // Keep selection if valid, otherwise reset
                if let Some(sel) = state.branch_list.selected
                    && sel >= state.branch_list.filtered.len()
                {
                    state.branch_list.selected = if state.branch_list.filtered.is_empty() {
                        None
                    } else {
                        Some(0)
                    };
                }
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
                enter_branch_select(state, idx, git, tmux, sender);
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

        // All other actions are handled by helper functions above or are not applicable
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
            // These actions are already handled by helper functions before this match
            unreachable!("These actions should have been handled by helper functions above")
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::git::mock::MockGitProvider;
    use kiosk_core::git::{Repo, Worktree};
    use kiosk_core::state::{AppState, BranchEntry, Mode, SearchableList};
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
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = EventSender {
            tx,
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        let result = process_action(
            Action::EnterRepo,
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert!(result.is_none());
        assert!(matches!(state.mode, Mode::Loading(_)));

        // Wait for the background thread to send the event
        let event = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &git, &tmux, &sender);
        assert_eq!(state.mode, Mode::BranchSelect);
        assert_eq!(state.branches.len(), 2);
    }

    #[test]
    fn test_remote_branches_appended() {
        use kiosk_core::state::BranchEntry;

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
        }];
        state.branch_list.reset(1);

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let sender = make_sender();

        // Simulate remote branches arriving
        let remote_branches = vec![
            BranchEntry {
                name: "feature-x".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_remote: true,
            },
            BranchEntry {
                name: "feature-y".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_remote: true,
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
        use kiosk_core::state::BranchEntry;

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
        }];
        state.branch_list.reset(1);
        state.branch_list.search = "feat".to_string();
        state.branch_list.cursor = 4;
        // With search "feat", main shouldn't match
        state.branch_list.filtered = vec![];
        state.branch_list.selected = None;

        let git: Arc<dyn GitProvider> = Arc::new(MockGitProvider::default());
        let tmux = MockTmuxProvider::default();
        let sender = make_sender();

        process_app_event(
            AppEvent::RemoteBranchesLoaded {
                branches: vec![BranchEntry {
                    name: "feature-x".to_string(),
                    worktree_path: None,
                    has_session: false,
                    is_current: false,
                    is_remote: true,
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
            is_remote: false,
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
            is_remote: false,
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
            is_remote: false,
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
            is_remote: false,
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
            is_remote: false,
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
            is_remote: false,
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
            is_remote: false,
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
            is_remote: false,
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
