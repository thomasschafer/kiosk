use crate::theme::Theme;
use kiosk_core::config::{Command, KeysConfig};
use kiosk_core::state::{AppState, Mode};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

#[allow(clippy::too_many_lines)]
pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme, keys: &KeysConfig) {
    let repo_name = state
        .selected_repo_idx
        .map_or("??", |i| state.repos[i].name.as_str());
    let selected_repo_path = state.selected_repo_idx.map(|i| state.repos[i].path.clone());

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    super::search_bar::draw(
        f,
        chunks[0],
        &format!("{repo_name} — select branch"),
        &state.branch_list.search,
        state.branch_list.cursor,
        "Type to search branches (or type new branch name)...",
        theme.secondary,
    );

    // Branch list
    let mut items: Vec<ListItem> = state
        .branch_list
        .filtered
        .iter()
        .map(|(idx, _)| {
            let branch = &state.branches[*idx];

            if branch.is_remote {
                // Remote branches rendered with muted style
                let mut spans = vec![Span::styled(
                    &branch.name,
                    Style::default().fg(Color::DarkGray),
                )];
                spans.push(Span::styled(
                    " (remote)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
                return ListItem::new(Line::from(spans));
            }

            let mut spans = vec![Span::raw(&branch.name)];
            let is_deleting = selected_repo_path
                .as_ref()
                .is_some_and(|repo_path| state.is_branch_pending_delete(repo_path, &branch.name));

            if is_deleting {
                spans.push(Span::styled(
                    " (deleting...)",
                    Style::default().fg(theme.accent),
                ));
            } else if branch.has_session {
                spans.push(Span::styled(
                    " (session)",
                    Style::default().fg(theme.success),
                ));
            } else if branch.worktree_path.is_some() {
                spans.push(Span::styled(
                    " (worktree)",
                    Style::default().fg(Color::Yellow),
                ));
            }
            if branch.is_current {
                spans.push(Span::styled(" *", Style::default().fg(theme.accent)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    // If search doesn't match anything, show "create new branch" option
    if state.loading_branches && state.branch_list.filtered.is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "Loading branches...",
            Style::default().fg(Color::DarkGray),
        )])));
    } else if state.branch_list.filtered.is_empty() && !state.branch_list.search.is_empty() {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("+ Create branch ", Style::default().fg(theme.success)),
            Span::styled(
                format!("\"{}\"", state.branch_list.search),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " (Enter to pick base)",
                Style::default().fg(Color::DarkGray),
            ),
        ])));
    }

    let count = state.branch_list.filtered.len();
    let hints = build_branch_hints(keys);
    let loading_suffix = if state.loading_branches {
        " | loading..."
    } else {
        ""
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {count} branches ({hints}{loading_suffix}) "))
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.secondary)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(state.branch_list.selected);
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}

fn build_branch_hints(keys: &KeysConfig) -> String {
    let keymap = keys.keymap_for_mode(&Mode::BranchSelect);
    let mut hints = Vec::new();

    if let Some(key) = KeysConfig::find_key(&keymap, &Command::GoBack) {
        hints.push(format!("{key}: go back"));
    }
    if let Some(key) = KeysConfig::find_key(&keymap, &Command::NewBranch) {
        hints.push(format!("{key}: new branch"));
    }
    if let Some(key) = KeysConfig::find_key(&keymap, &Command::DeleteWorktree) {
        hints.push(format!("{key}: delete worktree"));
    }

    hints.join(", ")
}
