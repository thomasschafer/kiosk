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
use std::path::PathBuf;

#[derive(Debug, Clone)]
struct BranchEntry {
    name: String,
    /// If a worktree already exists for this branch
    worktree_path: Option<PathBuf>,
    has_session: bool,
    is_current: bool,
}

pub struct App {
    repos: Vec<Repo>,
    filtered_repos: Vec<(usize, i64)>,
    repo_list_state: ListState,
    repo_search: String,

    // Branch picker (when a repo is selected)
    selected_repo_idx: Option<usize>,
    branches: Vec<BranchEntry>,
    filtered_branches: Vec<(usize, i64)>,
    branch_list_state: ListState,
    branch_search: String,

    // New branch flow
    new_branch_base: Option<NewBranchFlow>,

    matcher: SkimMatcherV2,
    split_command: Option<String>,
    mode: Mode,
}

#[derive(Debug, Clone)]
struct NewBranchFlow {
    /// The new branch name (what the user typed)
    new_name: String,
    /// Base branches to pick from
    bases: Vec<String>,
    filtered: Vec<(usize, i64)>,
    list_state: ListState,
    search: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    RepoSelect,
    BranchSelect,
    NewBranchBase,
}

impl App {
    pub fn new(repos: Vec<Repo>, split_command: Option<String>) -> Self {
        let filtered_repos: Vec<(usize, i64)> =
            repos.iter().enumerate().map(|(i, _)| (i, 0)).collect();
        let mut repo_list_state = ListState::default();
        if !filtered_repos.is_empty() {
            repo_list_state.select(Some(0));
        }

        Self {
            repos,
            filtered_repos,
            repo_list_state,
            repo_search: String::new(),
            selected_repo_idx: None,
            branches: Vec::new(),
            filtered_branches: Vec::new(),
            branch_list_state: ListState::default(),
            branch_search: String::new(),
            new_branch_base: None,
            matcher: SkimMatcherV2::default(),
            split_command,
            mode: Mode::RepoSelect,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<Option<OpenAction>> {
        loop {
            terminal.draw(|f| self.draw(f))?;

            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Global quit
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    return Ok(None);
                }

                match self.mode {
                    Mode::RepoSelect => {
                        if let Some(action) = self.handle_repo_select(key.code)? {
                            return Ok(Some(action));
                        }
                    }
                    Mode::BranchSelect => {
                        if let Some(action) = self.handle_branch_select(key.code)? {
                            return Ok(Some(action));
                        }
                    }
                    Mode::NewBranchBase => {
                        if let Some(action) = self.handle_new_branch_base(key.code)? {
                            return Ok(Some(action));
                        }
                    }
                }
            }
        }
    }

    fn handle_repo_select(
        &mut self,
        key: KeyCode,
    ) -> anyhow::Result<Option<OpenAction>> {
        match key {
            KeyCode::Esc => return Ok(Some(OpenAction::Quit)),
            KeyCode::Enter => {
                if let Some(sel) = self.repo_list_state.selected() {
                    if let Some(&(idx, _)) = self.filtered_repos.get(sel) {
                        self.enter_branch_select(idx);
                    }
                }
            }
            KeyCode::Up => self.move_repo_selection(-1),
            KeyCode::Down => self.move_repo_selection(1),
            KeyCode::Backspace => {
                self.repo_search.pop();
                self.update_repo_filter();
            }
            KeyCode::Char(c) => {
                self.repo_search.push(c);
                self.update_repo_filter();
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_branch_select(
        &mut self,
        key: KeyCode,
    ) -> anyhow::Result<Option<OpenAction>> {
        match key {
            KeyCode::Esc => {
                self.mode = Mode::RepoSelect;
                self.branch_search.clear();
            }
            KeyCode::Enter => {
                // If we have a selected branch, open it
                if let Some(sel) = self.branch_list_state.selected() {
                    if let Some(&(idx, _)) = self.filtered_branches.get(sel) {
                        let branch = &self.branches[idx];
                        let repo = &self.repos[self.selected_repo_idx.unwrap()];

                        if let Some(wt_path) = &branch.worktree_path {
                            // Worktree exists — just open it
                            return Ok(Some(OpenAction::Open {
                                path: wt_path.clone(),
                                split_command: self.split_command.clone(),
                            }));
                        }
                        // No worktree — create one
                        let wt_path = worktree_dir(repo, &branch.name);
                        git::add_worktree(&repo.path, &branch.name, &wt_path)?;
                        return Ok(Some(OpenAction::Open {
                            path: wt_path,
                            split_command: self.split_command.clone(),
                        }));
                    }
                }

                // No match — if search is non-empty, offer to create new branch
                if !self.branch_search.is_empty() && self.filtered_branches.is_empty() {
                    self.start_new_branch_flow();
                }
            }
            KeyCode::Up => self.move_branch_selection(-1),
            KeyCode::Down => self.move_branch_selection(1),
            KeyCode::Backspace => {
                self.branch_search.pop();
                self.update_branch_filter();
            }
            KeyCode::Char(c) => {
                self.branch_search.push(c);
                self.update_branch_filter();
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_new_branch_base(
        &mut self,
        key: KeyCode,
    ) -> anyhow::Result<Option<OpenAction>> {
        let flow = self.new_branch_base.as_mut().unwrap();
        match key {
            KeyCode::Esc => {
                self.new_branch_base = None;
                self.mode = Mode::BranchSelect;
            }
            KeyCode::Enter => {
                if let Some(sel) = flow.list_state.selected() {
                    if let Some(&(idx, _)) = flow.filtered.get(sel) {
                        let base = flow.bases[idx].clone();
                        let new_name = flow.new_name.clone();
                        let repo = &self.repos[self.selected_repo_idx.unwrap()];

                        // Create new branch and worktree
                        let wt_path = worktree_dir(repo, &new_name);
                        git::create_branch_and_worktree(
                            &repo.path,
                            &new_name,
                            &base,
                            &wt_path,
                        )?;
                        return Ok(Some(OpenAction::Open {
                            path: wt_path,
                            split_command: self.split_command.clone(),
                        }));
                    }
                }
            }
            KeyCode::Up => {
                if let Some(flow) = &mut self.new_branch_base {
                    move_list_selection(&mut flow.list_state, flow.filtered.len(), -1);
                }
            }
            KeyCode::Down => {
                if let Some(flow) = &mut self.new_branch_base {
                    move_list_selection(&mut flow.list_state, flow.filtered.len(), 1);
                }
            }
            KeyCode::Backspace => {
                flow.search.pop();
                update_fuzzy_filter(
                    &self.matcher,
                    &flow.bases,
                    &flow.search,
                    &mut flow.filtered,
                    &mut flow.list_state,
                );
            }
            KeyCode::Char(c) => {
                flow.search.push(c);
                update_fuzzy_filter(
                    &self.matcher,
                    &flow.bases,
                    &flow.search,
                    &mut flow.filtered,
                    &mut flow.list_state,
                );
            }
            _ => {}
        }
        Ok(None)
    }

    fn enter_branch_select(&mut self, repo_idx: usize) {
        self.selected_repo_idx = Some(repo_idx);
        self.branch_search.clear();
        self.mode = Mode::BranchSelect;

        let repo = &self.repos[repo_idx];
        let sessions = tmux::list_sessions();
        let all_branches = git::list_branches(&repo.path);

        // Map worktrees by branch name for quick lookup
        let wt_by_branch: std::collections::HashMap<&str, &Worktree> = repo
            .worktrees
            .iter()
            .filter_map(|wt| wt.branch.as_deref().map(|b| (b, wt)))
            .collect();

        self.branches = all_branches
            .iter()
            .map(|branch_name| {
                let worktree_path = wt_by_branch
                    .get(branch_name.as_str())
                    .map(|wt| wt.path.clone());
                let has_session = worktree_path
                    .as_ref()
                    .map(|p| sessions.contains(&tmux::session_name_for(p)))
                    .unwrap_or(false);
                let is_current = repo
                    .worktrees
                    .first()
                    .and_then(|wt| wt.branch.as_deref())
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
        self.branches.sort_by(|a, b| {
            b.has_session
                .cmp(&a.has_session)
                .then(b.worktree_path.is_some().cmp(&a.worktree_path.is_some()))
                .then(a.name.cmp(&b.name))
        });

        self.filtered_branches = self
            .branches
            .iter()
            .enumerate()
            .map(|(i, _)| (i, 0))
            .collect();
        self.branch_list_state = ListState::default();
        if !self.filtered_branches.is_empty() {
            self.branch_list_state.select(Some(0));
        }
    }

    fn start_new_branch_flow(&mut self) {
        let repo = &self.repos[self.selected_repo_idx.unwrap()];
        let bases = git::list_branches(&repo.path);
        let filtered: Vec<(usize, i64)> = bases.iter().enumerate().map(|(i, _)| (i, 0)).collect();
        let mut list_state = ListState::default();
        if !filtered.is_empty() {
            list_state.select(Some(0));
        }

        self.new_branch_base = Some(NewBranchFlow {
            new_name: self.branch_search.clone(),
            bases,
            filtered,
            list_state,
            search: String::new(),
        });
        self.mode = Mode::NewBranchBase;
    }

    // -- Filter updates --

    fn update_repo_filter(&mut self) {
        let names: Vec<String> = self.repos.iter().map(|r| r.name.clone()).collect();
        update_fuzzy_filter(
            &self.matcher,
            &names,
            &self.repo_search,
            &mut self.filtered_repos,
            &mut self.repo_list_state,
        );
    }

    fn update_branch_filter(&mut self) {
        let names: Vec<String> = self.branches.iter().map(|b| b.name.clone()).collect();
        update_fuzzy_filter(
            &self.matcher,
            &names,
            &self.branch_search,
            &mut self.filtered_branches,
            &mut self.branch_list_state,
        );
    }

    // -- Selection movement --

    fn move_repo_selection(&mut self, delta: i32) {
        move_list_selection(&mut self.repo_list_state, self.filtered_repos.len(), delta);
    }

    fn move_branch_selection(&mut self, delta: i32) {
        move_list_selection(
            &mut self.branch_list_state,
            self.filtered_branches.len(),
            delta,
        );
    }

    // -- Drawing --

    fn draw(&mut self, f: &mut Frame) {
        match self.mode {
            Mode::RepoSelect => self.draw_repo_select(f),
            Mode::BranchSelect => self.draw_branch_select(f),
            Mode::NewBranchBase => {
                self.draw_branch_select(f);
                self.draw_new_branch_popup(f);
            }
        }
    }

    fn draw_repo_select(&mut self, f: &mut Frame) {
        let chunks =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(f.area());

        // Search bar
        let search_text = if self.repo_search.is_empty() {
            Line::from(Span::styled(
                "Type to search repos...",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(&*self.repo_search)
        };
        let search_block = Block::default()
            .borders(Borders::ALL)
            .title(" wts — select repo ")
            .border_style(Style::default().fg(Color::Magenta));
        f.render_widget(Paragraph::new(search_text).block(search_block), chunks[0]);

        // Repo list
        let items: Vec<ListItem> = self
            .filtered_repos
            .iter()
            .map(|(idx, _)| {
                let repo = &self.repos[*idx];
                let wt_count = repo.worktrees.len();
                let branch = repo
                    .worktrees
                    .first()
                    .and_then(|wt| wt.branch.as_deref())
                    .unwrap_or("??");

                let mut spans = vec![Span::raw(&repo.name)];
                spans.push(Span::styled(
                    format!(" [{branch}]"),
                    Style::default().fg(Color::DarkGray),
                ));
                if wt_count > 1 {
                    spans.push(Span::styled(
                        format!(" +{} worktrees", wt_count - 1),
                        Style::default().fg(Color::Yellow),
                    ));
                }

                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} repos ", self.filtered_repos.len()))
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Magenta)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        f.render_stateful_widget(list, chunks[1], &mut self.repo_list_state);
    }

    fn draw_branch_select(&mut self, f: &mut Frame) {
        let repo_name = self
            .selected_repo_idx
            .map(|i| self.repos[i].name.as_str())
            .unwrap_or("??");

        let chunks =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(f.area());

        // Search bar
        let search_text = if self.branch_search.is_empty() {
            Line::from(Span::styled(
                "Type to search branches (or type new branch name)...",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(&*self.branch_search)
        };
        let search_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {repo_name} — select branch "))
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Paragraph::new(search_text).block(search_block), chunks[0]);

        // Branch list
        let items: Vec<ListItem> = self
            .filtered_branches
            .iter()
            .map(|(idx, _)| {
                let branch = &self.branches[*idx];
                let mut spans = vec![Span::raw(&branch.name)];

                if branch.has_session {
                    spans.push(Span::styled(" ●", Style::default().fg(Color::Green)));
                } else if branch.worktree_path.is_some() {
                    spans.push(Span::styled(
                        " (worktree)",
                        Style::default().fg(Color::Yellow),
                    ));
                }
                if branch.is_current {
                    spans.push(Span::styled(" *", Style::default().fg(Color::Magenta)));
                }

                ListItem::new(Line::from(spans))
            })
            .collect();

        let mut list_items = items;

        // If search doesn't match anything, show "create new branch" option
        if self.filtered_branches.is_empty() && !self.branch_search.is_empty() {
            list_items.push(ListItem::new(Line::from(vec![
                Span::styled("+ Create branch ", Style::default().fg(Color::Green)),
                Span::styled(
                    format!("\"{}\"", self.branch_search),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " (Enter to pick base)",
                    Style::default().fg(Color::DarkGray),
                ),
            ])));
        }

        let count = self.filtered_branches.len();
        let list = List::new(list_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {count} branches (Esc to go back) "))
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        f.render_stateful_widget(list, chunks[1], &mut self.branch_list_state);
    }

    fn draw_new_branch_popup(&mut self, f: &mut Frame) {
        let Some(flow) = &mut self.new_branch_base else {
            return;
        };

        let area = centered_rect(60, 60, f.area());
        f.render_widget(Clear, area);

        let chunks =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

        let search_text = if flow.search.is_empty() {
            Line::from(Span::styled(
                "Select base branch...",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(&*flow.search)
        };
        let title = format!(" New branch \"{}\" — pick base ", flow.new_name);
        let search_block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Green));
        f.render_widget(Paragraph::new(search_text).block(search_block), chunks[0]);

        let items: Vec<ListItem> = flow
            .filtered
            .iter()
            .map(|(idx, _)| ListItem::new(&*flow.bases[*idx]))
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Green)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        f.render_stateful_widget(list, chunks[1], &mut flow.list_state);
    }
}

pub enum OpenAction {
    Open {
        path: PathBuf,
        split_command: Option<String>,
    },
    Quit,
}

/// Determine where to put a new worktree for a branch
fn worktree_dir(repo: &Repo, branch: &str) -> PathBuf {
    // Place worktrees as siblings: ../repo-name-branch
    let parent = repo.path.parent().unwrap_or(&repo.path);
    // Sanitize branch name for filesystem
    let safe_branch = branch.replace('/', "-");
    parent.join(format!("{}-{safe_branch}", repo.name))
}

fn update_fuzzy_filter(
    matcher: &SkimMatcherV2,
    items: &[String],
    query: &str,
    filtered: &mut Vec<(usize, i64)>,
    list_state: &mut ListState,
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
        list_state.select(None);
    } else {
        list_state.select(Some(0));
    }
}

fn move_list_selection(state: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        return;
    }
    let current = state.selected().unwrap_or(0) as i32;
    let next = (current + delta).clamp(0, len as i32 - 1) as usize;
    state.select(Some(next));
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
