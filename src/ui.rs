use crate::{
    git::{self, Repo, Worktree},
    tmux,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

/// A flattened entry for display
#[derive(Debug, Clone)]
pub struct Entry {
    pub repo_name: String,
    pub repo_path: std::path::PathBuf,
    pub worktree: Worktree,
    pub has_session: bool,
}

impl Entry {
    pub fn display_name(&self) -> String {
        let branch = self
            .worktree
            .branch
            .as_deref()
            .unwrap_or("(detached)");

        if self.worktree.is_main {
            format!("{} [{}]", self.repo_name, branch)
        } else {
            let wt_name = self
                .worktree
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            format!("  {} → {} [{}]", self.repo_name, wt_name, branch)
        }
    }

    pub fn search_text(&self) -> String {
        let branch = self.worktree.branch.as_deref().unwrap_or("");
        format!("{} {}", self.repo_name, branch)
    }
}

pub struct App {
    entries: Vec<Entry>,
    filtered: Vec<(usize, i64)>, // (index into entries, score)
    list_state: ListState,
    search: String,
    matcher: SkimMatcherV2,
    split_command: Option<String>,
    mode: Mode,
    // For new worktree flow
    new_wt_repo_idx: Option<usize>,
    branches: Vec<String>,
    branch_filtered: Vec<(usize, i64)>,
    branch_search: String,
    branch_list_state: ListState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Browse,
    NewWorktree,
}

impl App {
    pub fn new(repos: Vec<Repo>, split_command: Option<String>) -> Self {
        let sessions = tmux::list_sessions();
        let mut entries = Vec::new();

        for repo in &repos {
            for wt in &repo.worktrees {
                let session_name = tmux::session_name_for(&wt.path);
                entries.push(Entry {
                    repo_name: repo.name.clone(),
                    repo_path: repo.path.clone(),
                    worktree: wt.clone(),
                    has_session: sessions.contains(&session_name),
                });
            }
        }

        let filtered: Vec<(usize, i64)> = entries.iter().enumerate().map(|(i, _)| (i, 0)).collect();
        let mut list_state = ListState::default();
        if !filtered.is_empty() {
            list_state.select(Some(0));
        }

        Self {
            entries,
            filtered,
            list_state,
            search: String::new(),
            matcher: SkimMatcherV2::default(),
            split_command,
            mode: Mode::Browse,
            new_wt_repo_idx: None,
            branches: Vec::new(),
            branch_filtered: Vec::new(),
            branch_search: String::new(),
            branch_list_state: ListState::default(),
        }
    }

    fn update_filter(&mut self) {
        if self.search.is_empty() {
            self.filtered = self.entries.iter().enumerate().map(|(i, _)| (i, 0)).collect();
        } else {
            let mut scored: Vec<(usize, i64)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    self.matcher
                        .fuzzy_match(&e.search_text(), &self.search)
                        .map(|score| (i, score))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered = scored;
        }

        if self.filtered.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    fn update_branch_filter(&mut self) {
        if self.branch_search.is_empty() {
            self.branch_filtered = self.branches.iter().enumerate().map(|(i, _)| (i, 0)).collect();
        } else {
            let mut scored: Vec<(usize, i64)> = self
                .branches
                .iter()
                .enumerate()
                .filter_map(|(i, b)| {
                    self.matcher
                        .fuzzy_match(b, &self.branch_search)
                        .map(|score| (i, score))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.branch_filtered = scored;
        }

        if self.branch_filtered.is_empty() {
            self.branch_list_state.select(None);
        } else {
            self.branch_list_state.select(Some(0));
        }
    }

    fn selected_entry(&self) -> Option<&Entry> {
        let sel = self.list_state.selected()?;
        let (idx, _) = self.filtered.get(sel)?;
        self.entries.get(*idx)
    }

    fn selected_branch(&self) -> Option<&str> {
        let sel = self.branch_list_state.selected()?;
        let (idx, _) = self.branch_filtered.get(sel)?;
        self.branches.get(*idx).map(String::as_str)
    }

    /// Returns Some(path) to open, or None to quit
    pub fn run(mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<Option<OpenAction>> {
        loop {
            terminal.draw(|f| self.draw(f))?;

            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match self.mode {
                    Mode::Browse => match key.code {
                        KeyCode::Esc => return Ok(None),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(None);
                        }
                        KeyCode::Enter => {
                            if let Some(entry) = self.selected_entry().cloned() {
                                return Ok(Some(OpenAction::Open {
                                    path: entry.worktree.path,
                                    split_command: self.split_command.clone(),
                                }));
                            }
                        }
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // New worktree for selected repo
                            if let Some(entry) = self.selected_entry().cloned() {
                                self.branches = git::list_branches(&entry.repo_path);
                                self.branch_search.clear();
                                self.update_branch_filter();
                                self.new_wt_repo_idx = self
                                    .list_state
                                    .selected()
                                    .and_then(|s| self.filtered.get(s))
                                    .map(|(i, _)| *i);
                                self.mode = Mode::NewWorktree;
                            }
                        }
                        KeyCode::Up => {
                            self.move_selection(-1);
                        }
                        KeyCode::Down => {
                            self.move_selection(1);
                        }
                        KeyCode::Char(c) => {
                            self.search.push(c);
                            self.update_filter();
                        }
                        KeyCode::Backspace => {
                            self.search.pop();
                            self.update_filter();
                        }
                        _ => {}
                    },
                    Mode::NewWorktree => match key.code {
                        KeyCode::Esc => {
                            self.mode = Mode::Browse;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.mode = Mode::Browse;
                        }
                        KeyCode::Enter => {
                            if let (Some(entry_idx), Some(branch)) =
                                (self.new_wt_repo_idx, self.selected_branch().map(String::from))
                            {
                                let entry = &self.entries[entry_idx];
                                let wt_dir = entry
                                    .repo_path
                                    .parent()
                                    .unwrap_or(&entry.repo_path)
                                    .join(format!("{}-{}", entry.repo_name, branch));

                                if let Err(e) =
                                    git::add_worktree(&entry.repo_path, &branch, &wt_dir)
                                {
                                    // TODO: show error in UI
                                    eprintln!("Error: {e}");
                                } else {
                                    return Ok(Some(OpenAction::Open {
                                        path: wt_dir,
                                        split_command: self.split_command.clone(),
                                    }));
                                }
                            }
                        }
                        KeyCode::Up => {
                            self.move_branch_selection(-1);
                        }
                        KeyCode::Down => {
                            self.move_branch_selection(1);
                        }
                        KeyCode::Char(c) => {
                            self.branch_search.push(c);
                            self.update_branch_filter();
                        }
                        KeyCode::Backspace => {
                            self.branch_search.pop();
                            self.update_branch_filter();
                        }
                        _ => {}
                    },
                }
            }
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, self.filtered.len() as i32 - 1) as usize;
        self.list_state.select(Some(next));
    }

    fn move_branch_selection(&mut self, delta: i32) {
        if self.branch_filtered.is_empty() {
            return;
        }
        let current = self.branch_list_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, self.branch_filtered.len() as i32 - 1) as usize;
        self.branch_list_state.select(Some(next));
    }

    fn draw(&mut self, f: &mut Frame) {
        let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(f.area());

        self.draw_search_bar(f, chunks[0]);
        self.draw_list(f, chunks[1]);

        if self.mode == Mode::NewWorktree {
            self.draw_branch_popup(f);
        }
    }

    fn draw_search_bar(&self, f: &mut Frame, area: Rect) {
        let search_display = if self.search.is_empty() {
            Line::from(Span::styled(
                "Type to search...",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(&*self.search)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" wts ")
            .border_style(Style::default().fg(Color::Magenta));

        let paragraph = Paragraph::new(search_display).block(block);
        f.render_widget(paragraph, area);
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .map(|(idx, _)| {
                let entry = &self.entries[*idx];
                let name = entry.display_name();
                let session_indicator = if entry.has_session { " ●" } else { "" };

                let style = if entry.has_session {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };

                ListItem::new(Line::from(vec![
                    Span::styled(name, style),
                    Span::styled(session_indicator, Style::default().fg(Color::Green)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} entries ", self.filtered.len()))
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Magenta)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn draw_branch_popup(&mut self, f: &mut Frame) {
        let area = centered_rect(60, 70, f.area());
        f.render_widget(Clear, area);

        let chunks =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

        // Branch search bar
        let search_display = if self.branch_search.is_empty() {
            Line::from(Span::styled(
                "Select branch for new worktree...",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(&*self.branch_search)
        };

        let search_block = Block::default()
            .borders(Borders::ALL)
            .title(" New Worktree (Ctrl+W) ")
            .border_style(Style::default().fg(Color::Yellow));
        f.render_widget(Paragraph::new(search_display).block(search_block), chunks[0]);

        // Branch list
        let items: Vec<ListItem> = self
            .branch_filtered
            .iter()
            .map(|(idx, _)| ListItem::new(&*self.branches[*idx]))
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Yellow)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        f.render_stateful_widget(list, chunks[1], &mut self.branch_list_state);
    }
}

pub enum OpenAction {
    Open {
        path: std::path::PathBuf,
        split_command: Option<String>,
    },
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
