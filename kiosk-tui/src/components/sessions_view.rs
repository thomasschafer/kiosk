use crate::theme::Theme;
use kiosk_core::config::KeysConfig;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState},
};

pub fn draw(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    _keys: &KeysConfig,
    show_selection: bool,
) {
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
        &state.sessions_view.list.input.text,
        state.sessions_view.list.input.cursor,
    );

    // Row width available for content (list area minus borders minus highlight symbol)
    let row_width = (chunks[1].width as usize).saturating_sub(4);

    // Session list
    let mut items: Vec<ListItem> = state
        .sessions_view
        .list
        .filtered
        .iter()
        .map(|(idx, _)| {
            let session = &state.sessions_view.sessions[*idx];
            let mut left_spans: Vec<Span<'_>> = Vec::new();

            let primary_label = session.repo_name().unwrap_or(session.session_name.as_str());
            left_spans.push(Span::raw(primary_label));
            if let Some(branch) = session.branch() {
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

    // Determine the right empty-state message.
    // Priority order matters: "No matching sessions" should show whenever
    // sessions exist but the filter produces no hits, regardless of whether
    // a background refresh is in progress — the title bar already shows the
    // "(loading...)" suffix so there's no need to hide the filter feedback.
    if !state.sessions_view.sessions.is_empty() && state.sessions_view.list.filtered.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "No matching sessions",
            Style::default().fg(theme.muted),
        ))));
    } else if state.sessions_view.sessions.is_empty() && state.sessions_view.is_loading() {
        items.push(ListItem::new(Line::from(Span::styled(
            "Loading sessions...",
            Style::default().fg(theme.muted),
        ))));
    } else if state.sessions_view.sessions.is_empty() && !state.sessions_view.is_loading() {
        items.push(ListItem::new(Line::from(Span::styled(
            "No active sessions",
            Style::default().fg(theme.muted),
        ))));
    }

    let count = state.sessions_view.list.filtered.len();
    let loading_suffix = if state.sessions_view.is_resolving() {
        " | loading..."
    } else {
        ""
    };
    let session_word = if count == 1 { "session" } else { "sessions" };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {count} {session_word}{loading_suffix} "))
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.accent)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always);

    let mut list_state = ListState::default();
    if show_selection {
        list_state.select(state.sessions_view.list.selected);
    }
    *list_state.offset_mut() = state.sessions_view.list.scroll_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
