use crate::theme::Theme;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

pub fn draw(f: &mut Frame, state: &AppState, theme: &Theme) {
    let Some(flow) = &state.base_branch_selection else {
        return;
    };

    let area = super::centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    let search_text = if flow.list.search.is_empty() {
        Line::from(Span::styled(
            "Select base branch...",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(flow.list.search.as_str())
    };
    let title = format!(" New branch \"{}\" — pick base ", flow.new_name);
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.success));
    f.render_widget(Paragraph::new(search_text).block(search_block), chunks[0]);

    let items: Vec<ListItem> = flow
        .list
        .filtered
        .iter()
        .map(|(idx, _)| ListItem::new(flow.bases[*idx].as_str()))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.success)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(flow.list.selected);
    *list_state.offset_mut() = flow.list.scroll_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
