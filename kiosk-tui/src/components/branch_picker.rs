use crate::theme::Theme;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let repo_name = state
        .selected_repo_idx
        .map_or("??", |i| state.repos[i].name.as_str());

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    let search_text = if state.branch_search.is_empty() {
        Line::from(Span::styled(
            "Type to search branches (or type new branch name)...",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(state.branch_search.as_str())
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {repo_name} — select branch "))
        .border_style(Style::default().fg(theme.secondary));
    f.render_widget(Paragraph::new(search_text).block(search_block), chunks[0]);

    // Branch list
    let mut items: Vec<ListItem> = state
        .filtered_branches
        .iter()
        .map(|(idx, _)| {
            let branch = &state.branches[*idx];
            let mut spans = vec![Span::raw(&branch.name)];

            if branch.has_session {
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
    if state.filtered_branches.is_empty() && !state.branch_search.is_empty() {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("+ Create branch ", Style::default().fg(theme.success)),
            Span::styled(
                format!("\"{}\"", state.branch_search),
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

    let count = state.filtered_branches.len();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {count} branches (Esc to go back, Ctrl+N for new branch) "))
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
    list_state.select(state.branch_selected);
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
