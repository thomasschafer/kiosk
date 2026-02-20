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
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

/// What to do after the TUI exits
pub enum OpenAction {
    Open {
        path: PathBuf,
        split_command: Option<String>,
    },
    Quit,
}

/// Handle for dispatching background work
#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<AppEvent>,
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
    git: Arc<dyn GitProvider>,
    tmux: &dyn TmuxProvider,
) -> anyhow::Result<Option<OpenAction>> {
    let matcher = SkimMatcherV2::default();
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let event_sender = EventSender { tx };
    let spinner_start = Instant::now();

    loop {
        terminal.draw(|f| draw(f, state, &spinner_start))?;

        // Check background channel (non-blocking)
        if let Ok(app_event) = rx.try_recv() {
            if let Some(result) = process_app_event(app_event, state) {
                return Ok(Some(result));
            }
            continue;
        }

        // Poll terminal events with a timeout so we can update spinner + check channel
        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
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
                        return Ok(Some(OpenAction::Quit));
                    }
                    continue;
                }

                // Clear error on any keypress
                state.error = None;

                if let Some(action) = keymap::resolve_action(key, state) {
                    if let Some(result) =
                        process_action(action, state, &git, tmux, &matcher, &event_sender)
                    {
                        return Ok(Some(result));
                    }
                }
            }
        }
    }
}

fn draw(f: &mut Frame, state: &AppState, spinner_start: &Instant) {
    // Loading mode: full-screen spinner
    if let Mode::Loading(ref msg) = state.mode {
        draw_loading(f, f.area(), msg, spinner_start);
        return;
    }

    let (main_area, error_area) = if state.error.is_some() {
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(f.area());
        (chunks[0], Some(chunks[1]))
    } else {
        (f.area(), None)
    };

    match state.mode {
        Mode::RepoSelect => components::repo_list::draw(f, main_area, state),
        Mode::BranchSelect => components::branch_picker::draw(f, main_area, state),
        Mode::NewBranchBase => {
            components::branch_picker::draw(f, main_area, state);
            components::new_branch::draw(f, state);
        }
        Mode::Loading(_) => unreachable!(),
    }

    if let Some(area) = error_area {
        components::error_bar::draw(f, area, state);
    }
}

fn draw_loading(f: &mut Frame, area: Rect, message: &str, start: &Instant) {
    let elapsed = start.elapsed().as_millis() as usize;
    let frame_idx = (elapsed / 80) % SPINNER_FRAMES.len();
    let spinner = SPINNER_FRAMES[frame_idx];

    let text = Line::from(vec![
        Span::styled(
            format!("{spinner} "),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(message),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

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

/// Handle events from background tasks
fn process_app_event(event: AppEvent, state: &mut AppState) -> Option<OpenAction> {
    match event {
        AppEvent::WorktreeCreated { path } => {
            return Some(OpenAction::Open {
                path,
                split_command: state.split_command.clone(),
            });
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
        AppEvent::ReposLoaded(repos) => {
            // Future: handle async repo discovery
            let _ = repos;
        }
    }
    None
}

fn spawn_worktree_creation(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch: String,
    wt_path: PathBuf,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(
        move || match git.add_worktree(&repo_path, &branch, &wt_path) {
            Ok(()) => sender.send(AppEvent::WorktreeCreated { path: wt_path }),
            Err(e) => sender.send(AppEvent::GitError(format!("{e}"))),
        },
    );
}

fn spawn_branch_and_worktree_creation(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    new_branch: String,
    base: String,
    wt_path: PathBuf,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        match git.create_branch_and_worktree(&repo_path, &new_branch, &base, &wt_path) {
            Ok(()) => sender.send(AppEvent::WorktreeCreated { path: wt_path }),
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

        Action::EnterRepo => {
            if let Some(sel) = state.repo_selected {
                if let Some(&(idx, _)) = state.filtered_repos.get(sel) {
                    enter_branch_select(state, idx, git.as_ref(), tmux);
                }
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
            Mode::RepoSelect | Mode::Loading(_) => {}
        },

        Action::OpenBranch => match state.mode {
            Mode::BranchSelect => {
                if let Some(sel) = state.branch_selected {
                    if let Some(&(idx, _)) = state.filtered_branches.get(sel) {
                        let branch = &state.branches[idx];
                        let repo = &state.repos[state.selected_repo_idx.unwrap()];

                        if let Some(wt_path) = &branch.worktree_path {
                            return Some(OpenAction::Open {
                                path: wt_path.clone(),
                                split_command: state.split_command.clone(),
                            });
                        }
                        let wt_path = worktree_dir(repo, &branch.name);
                        let branch_name = branch.name.clone();
                        state.mode =
                            Mode::Loading(format!("Creating worktree for {branch_name}..."));
                        spawn_worktree_creation(
                            git,
                            sender,
                            repo.path.clone(),
                            branch_name,
                            wt_path,
                        );
                    }
                }
            }
            Mode::NewBranchBase => {
                if let Some(flow) = &state.new_branch_base {
                    if let Some(sel) = flow.selected {
                        if let Some(&(idx, _)) = flow.filtered.get(sel) {
                            let base = flow.bases[idx].clone();
                            let new_name = flow.new_name.clone();
                            let repo = &state.repos[state.selected_repo_idx.unwrap()];
                            let wt_path = worktree_dir(repo, &new_name);
                            state.mode =
                                Mode::Loading(format!("Creating branch {new_name} from {base}..."));
                            spawn_branch_and_worktree_creation(
                                git,
                                sender,
                                repo.path.clone(),
                                new_name,
                                base,
                                wt_path,
                            );
                        }
                    }
                }
            }
            Mode::RepoSelect | Mode::Loading(_) => {}
        },

        Action::StartNewBranchFlow => {
            let repo = &state.repos[state.selected_repo_idx.unwrap()];
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
            Mode::Loading(_) => {}
        },

        Action::SearchPush(c) => match state.mode {
            Mode::RepoSelect => {
                state.repo_search.push(c);
                update_repo_filter(state, matcher);
            }
            Mode::BranchSelect => {
                state.branch_search.push(c);
                update_branch_filter(state, matcher);
            }
            Mode::NewBranchBase => {
                if let Some(flow) = &mut state.new_branch_base {
                    flow.search.push(c);
                    update_flow_filter(flow, matcher);
                }
            }
            Mode::Loading(_) => {}
        },

        Action::SearchPop => match state.mode {
            Mode::RepoSelect => {
                state.repo_search.pop();
                update_repo_filter(state, matcher);
            }
            Mode::BranchSelect => {
                state.branch_search.pop();
                update_branch_filter(state, matcher);
            }
            Mode::NewBranchBase => {
                if let Some(flow) = &mut state.new_branch_base {
                    flow.search.pop();
                    update_flow_filter(flow, matcher);
                }
            }
            Mode::Loading(_) => {}
        },

        Action::ShowError(msg) => {
            state.error = Some(msg);
        }
        Action::ClearError => {
            state.error = None;
        }

        // These are handled internally or not needed at this level
        Action::SelectRepo(_)
        | Action::SelectBranch(_)
        | Action::CreateWorktree { .. }
        | Action::CreateBranchAndWorktree { .. }
        | Action::OpenSession { .. } => {}
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
                .map(|p| sessions.contains(&tmux.session_name_for(p)))
                .unwrap_or(false);
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
    let current = selected.unwrap_or(0) as i32;
    let next = (current + delta).clamp(0, len as i32 - 1) as usize;
    *selected = Some(next);
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
