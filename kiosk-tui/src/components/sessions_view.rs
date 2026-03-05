use crate::theme::Theme;
use kiosk_core::config::KeysConfig;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme, _keys: &KeysConfig) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    super::search_bar::draw(
        f,
        chunks[0],
        &super::search_bar::SearchBarStyle {
            title: "kiosk — sessions",
            placeholder: "Type to search sessions...",
            border_color: theme.accent,
            muted_color: theme.muted,
        },
        &state.sessions_list.input.text,
        state.sessions_list.input.cursor,
    );

    // Row width available for content (list area minus borders minus highlight symbol)
    let row_width = (chunks[1].width as usize).saturating_sub(4);

    // Session list
    let mut items: Vec<ListItem> = state
        .sessions_list
        .filtered
        .iter()
        .map(|(idx, _)| {
            let session = &state.sessions[*idx];
            let mut left_spans: Vec<Span<'_>> = Vec::new();

            // repo/branch
            left_spans.push(Span::raw(&session.repo_name));
            if let Some(branch) = &session.branch {
                left_spans.push(Span::styled(
                    format!("/{branch}"),
                    Style::default().fg(theme.secondary),
                ));
            }

            if session.attached {
                left_spans.push(Span::styled(
                    " (attached)",
                    Style::default().fg(theme.success),
                ));
            }

            ListItem::new(Line::from(
                super::list_row::render_with_optional_agent_badges(
                    &left_spans,
                    &session.agent_statuses,
                    &state.agent_labels,
                    theme,
                    row_width,
                ),
            ))
        })
        .collect();

    if state.loading_sessions && state.sessions_list.filtered.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "Loading sessions...",
            Style::default().fg(theme.muted),
        ))));
    } else if state.sessions.is_empty() && !state.loading_sessions {
        items.push(ListItem::new(Line::from(Span::styled(
            "No active sessions",
            Style::default().fg(theme.muted),
        ))));
    }

    let count = state.sessions_list.filtered.len();
    let loading_suffix = if state.loading_sessions {
        " | loading..."
    } else {
        ""
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {count} sessions{loading_suffix} "))
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.accent)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(state.sessions_list.selected);
    *list_state.offset_mut() = state.sessions_list.scroll_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
