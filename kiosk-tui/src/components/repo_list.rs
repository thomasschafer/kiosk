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
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    let search_text = if state.repo_search.is_empty() {
        Line::from(Span::styled(
            "Type to search repos...",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(state.repo_search.as_str())
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(" kiosk — select repo ")
        .border_style(Style::default().fg(theme.accent));
    f.render_widget(Paragraph::new(search_text).block(search_block), chunks[0]);

    // Repo list
    let items: Vec<ListItem> = state
        .filtered_repos
        .iter()
        .map(|(idx, _)| {
            let repo = &state.repos[*idx];
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
                .title(format!(
                    " {} repos (Enter: open, Tab: branches) ",
                    state.filtered_repos.len()
                ))
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(state.repo_selected);
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
