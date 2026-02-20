use crate::{components, keymap};
use crossterm::event::{self, Event, KeyEventKind};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use kiosk_core::{
    action::Action,
    event::AppEvent,
    git::GitProvider,
    state::{AppState, BranchEntry, Mode, NewBranchFlow, worktree_dir},
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
        terminal.draw(|f| draw(f, state, theme, &spinner_start))?;

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

            if let Some(action) = keymap::resolve_action(key, state)
                && let Some(result) =
                    process_action(action, state, git, tmux, &matcher, &event_sender)
            {
                return Ok(Some(result));
            }
        }
    }
}

fn draw(f: &mut Frame, state: &AppState, theme: &crate::theme::Theme, spinner_start: &Instant) {
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

    match state.mode {
        Mode::RepoSelect => components::repo_list::draw(f, main_area, state, theme),
        Mode::BranchSelect => components::branch_picker::draw(f, main_area, state, theme),
        Mode::NewBranchBase => {
            components::branch_picker::draw(f, main_area, state, theme);
            components::new_branch::draw(f, state, theme);
        }
        Mode::ConfirmDelete(_) => {
            components::branch_picker::draw(f, main_area, state, theme);
            draw_confirm_delete_dialog(f, main_area, state, theme);
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
) {
    if let Mode::ConfirmDelete(branch_name) = &state.mode {
        let text = vec![
            Line::from(vec![
                Span::raw("Delete worktree for branch "),
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
                Span::styled("y", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("es / "),
                Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("o / "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
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
fn process_app_event(event: AppEvent, state: &mut AppState, git: &dyn GitProvider, tmux: &dyn TmuxProvider) -> Option<OpenAction> {
    match event {
        AppEvent::ReposDiscovered { repos } => {
            state.repos = repos;
            state.filtered_repos = state
                .repos
                .iter()
                .enumerate()
                .map(|(i, _)| (i, 0))
                .collect();
            state.repo_selected = if state.filtered_repos.is_empty() {
                None
            } else {
                Some(0)
            };
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
            Ok(()) => sender.send(AppEvent::WorktreeCreated { path: wt_path, session_name }),
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
            Ok(()) => sender.send(AppEvent::WorktreeCreated { path: wt_path, session_name }),
            Err(e) => sender.send(AppEvent::GitError(format!("{e}"))),
        }
    });
}

fn process_action(
    action: Action,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    tmux: &dyn TmuxProvider,
    matcher: &SkimMatcherV2,
    sender: &EventSender,
) -> Option<OpenAction> {
    match action {
        Action::Quit => return Some(OpenAction::Quit),

        Action::OpenRepo => {
            if let Some(sel) = state.repo_selected
                && let Some(&(idx, _)) = state.filtered_repos.get(sel)
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
            if let Some(sel) = state.repo_selected
                && let Some(&(idx, _)) = state.filtered_repos.get(sel)
            {
                enter_branch_select(state, idx, git.as_ref(), tmux);
            }
        }

        Action::GoBack => match state.mode {
            Mode::BranchSelect => {
                state.mode = Mode::RepoSelect;
                state.branch_search.clear();
            }
            Mode::NewBranchBase => {
                state.new_branch_base = None;
                state.mode = Mode::BranchSelect;
            }
            Mode::ConfirmDelete(_) => {
                state.mode = Mode::BranchSelect;
            }
            Mode::RepoSelect | Mode::Loading(_) => {}
        },

        Action::OpenBranch => {
            if let Some(result) = handle_open_branch(state, git, sender) {
                return Some(result);
            }
        }

        Action::StartNewBranchFlow => {
            let repo_idx = state.selected_repo_idx?;
            let repo = &state.repos[repo_idx];
            let bases = git.list_branches(&repo.path);
            let filtered: Vec<(usize, i64)> =
                bases.iter().enumerate().map(|(i, _)| (i, 0)).collect();
            let selected = if filtered.is_empty() { None } else { Some(0) };

            state.new_branch_base = Some(NewBranchFlow {
                new_name: state.branch_search.clone(),
                bases,
                filtered,
                selected,
                search: String::new(),
            });
            state.mode = Mode::NewBranchBase;
        }

        Action::MoveSelection(delta) => match state.mode {
            Mode::RepoSelect => {
                move_selection(&mut state.repo_selected, state.filtered_repos.len(), delta);
            }
            Mode::BranchSelect => {
                move_selection(
                    &mut state.branch_selected,
                    state.filtered_branches.len(),
                    delta,
                );
            }
            Mode::NewBranchBase => {
                if let Some(flow) = &mut state.new_branch_base {
                    move_selection(&mut flow.selected, flow.filtered.len(), delta);
                }
            }
            Mode::ConfirmDelete(_) | Mode::Loading(_) => {}
        },

        Action::SearchPush(c) => handle_search_update(state, matcher, |s| s.push(c)),
        Action::SearchPop => handle_search_update(state, matcher, |s| {
            s.pop();
        }),

        Action::DeleteWorktree => {
            // Only allow deletion on branches with worktrees that are not current
            if let Some(sel) = state.branch_selected
                && let Some(&(idx, _)) = state.filtered_branches.get(sel)
            {
                let branch = &state.branches[idx];
                if branch.worktree_path.is_some() && !branch.is_current {
                    state.mode = Mode::ConfirmDelete(branch.name.clone());
                }
            }
        }

        Action::ConfirmDeleteWorktree => {
            if let Mode::ConfirmDelete(branch_name) = &state.mode {
                let branch_name = branch_name.clone();
                // Find the branch and its worktree path
                if let Some(branch) = state.branches.iter().find(|b| b.name == branch_name)
                    && let Some(worktree_path) = &branch.worktree_path
                {
                    let worktree_path = worktree_path.clone();
                    state.mode = Mode::Loading(format!("Removing worktree for {branch_name}..."));
                    spawn_worktree_removal(git, sender, worktree_path, branch_name);
                }
            }
        }

        Action::CancelDeleteWorktree => {
            state.mode = Mode::BranchSelect;
        }

        Action::ShowError(msg) => {
            state.error = Some(msg);
        }
        Action::ClearError => {
            state.error = None;
        }
    }

    None
}

fn handle_search_update(
    state: &mut AppState,
    matcher: &SkimMatcherV2,
    mutate: impl FnOnce(&mut String),
) {
    match state.mode {
        Mode::RepoSelect => {
            mutate(&mut state.repo_search);
            update_repo_filter(state, matcher);
        }
        Mode::BranchSelect => {
            mutate(&mut state.branch_search);
            update_branch_filter(state, matcher);
        }
        Mode::NewBranchBase => {
            if let Some(flow) = &mut state.new_branch_base {
                mutate(&mut flow.search);
                update_flow_filter(flow, matcher);
            }
        }
        Mode::ConfirmDelete(_) | Mode::Loading(_) => {}
    }
}

fn handle_open_branch(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
) -> Option<OpenAction> {
    match state.mode {
        Mode::BranchSelect => {
            if let Some(sel) = state.branch_selected
                && let Some(&(idx, _)) = state.filtered_branches.get(sel)
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
                && let Some(sel) = flow.selected
                && let Some(&(idx, _)) = flow.filtered.get(sel)
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
        Mode::RepoSelect | Mode::ConfirmDelete(_) | Mode::Loading(_) => {}
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
    state.branch_search.clear();
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

    state.filtered_branches = state
        .branches
        .iter()
        .enumerate()
        .map(|(i, _)| (i, 0))
        .collect();
    state.branch_selected = if state.filtered_branches.is_empty() {
        None
    } else {
        Some(0)
    };
}

fn move_selection(selected: &mut Option<usize>, len: usize, delta: i32) {
    if len == 0 {
        return;
    }
    let current = selected.unwrap_or(0);
    if delta > 0 {
        *selected = Some(
            current
                .saturating_add(delta.unsigned_abs() as usize)
                .min(len - 1),
        );
    } else {
        *selected = Some(current.saturating_sub(delta.unsigned_abs() as usize));
    }
}

fn update_repo_filter(state: &mut AppState, matcher: &SkimMatcherV2) {
    let names: Vec<String> = state.repos.iter().map(|r| r.name.clone()).collect();
    update_fuzzy_filter(
        matcher,
        &names,
        &state.repo_search,
        &mut state.filtered_repos,
        &mut state.repo_selected,
    );
}

fn update_branch_filter(state: &mut AppState, matcher: &SkimMatcherV2) {
    let names: Vec<String> = state.branches.iter().map(|b| b.name.clone()).collect();
    update_fuzzy_filter(
        matcher,
        &names,
        &state.branch_search,
        &mut state.filtered_branches,
        &mut state.branch_selected,
    );
}

fn update_flow_filter(flow: &mut NewBranchFlow, matcher: &SkimMatcherV2) {
    update_fuzzy_filter(
        matcher,
        &flow.bases,
        &flow.search,
        &mut flow.filtered,
        &mut flow.selected,
    );
}

fn update_fuzzy_filter(
    matcher: &SkimMatcherV2,
    items: &[String],
    query: &str,
    filtered: &mut Vec<(usize, i64)>,
    selected: &mut Option<usize>,
) {
    if query.is_empty() {
        *filtered = items.iter().enumerate().map(|(i, _)| (i, 0)).collect();
    } else {
        let mut scored: Vec<(usize, i64)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| matcher.fuzzy_match(item, query).map(|score| (i, score)))
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        *filtered = scored;
    }

    if filtered.is_empty() {
        *selected = None;
    } else {
        *selected = Some(0);
    }
}

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
        state.repo_selected = Some(0);

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
            filtered: vec![(0, 0)],
            selected: Some(0),
            search: String::new(),
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
        state.filtered_branches = vec![(0, 0)];
        state.branch_selected = Some(0);

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
        state.filtered_branches = vec![(0, 0)];
        state.branch_selected = Some(0);

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
        assert_eq!(state.filtered_repos.len(), 2);

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
        assert_eq!(state.repo_search, "a");
        // "alpha" matches "a", "beta" also matches "a" — but both should be present
        assert!(!state.filtered_repos.is_empty());
    }

    #[test]
    fn test_move_selection() {
        let repos = vec![make_repo("alpha"), make_repo("beta"), make_repo("gamma")];
        let mut state = AppState::new(repos, None);
        assert_eq!(state.repo_selected, Some(0));

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
        assert_eq!(state.repo_selected, Some(1));

        process_action(
            Action::MoveSelection(1),
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_selected, Some(2));

        // Should clamp at max
        process_action(
            Action::MoveSelection(1),
            &mut state,
            &git,
            &tmux,
            &matcher,
            &sender,
        );
        assert_eq!(state.repo_selected, Some(2));
    }

    #[test]
    fn test_open_repo_returns_repo_path() {
        let repos = vec![make_repo("alpha"), make_repo("beta")];
        let mut state = AppState::new(repos, Some("hx".into()));
        state.repo_selected = Some(1);

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
}
