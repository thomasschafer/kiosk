use crate::theme::Theme;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};

pub fn draw(f: &mut Frame, state: &AppState, theme: &Theme) {
    let Some(flow) = &state.base_branch_selection else {
        return;
    };

    let area = super::centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    let title = format!("New branch \"{}\" — pick base", flow.new_name);
    super::search_bar::draw(
        f,
        chunks[0],
        &super::search_bar::SearchBarStyle {
            title: &title,
            placeholder: "Select base branch...",
            border_color: theme.tertiary,
            muted_color: theme.muted,
        },
        &flow.list.search,
        flow.list.cursor,
    );

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
                .border_style(Style::default().fg(theme.tertiary)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.tertiary)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(flow.list.selected);
    *list_state.offset_mut() = flow.list.scroll_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
