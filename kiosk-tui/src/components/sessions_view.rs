use crate::theme::Theme;
use kiosk_core::agent::AgentState;
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
                    Style::default().fg(theme.muted),
                ));
            }

            if session.attached {
                left_spans.push(Span::styled(
                    " (attached)",
                    Style::default().fg(theme.success),
                ));
            }

            // Agent badges on the right
            if session.agent_statuses.is_empty() {
                ListItem::new(Line::from(left_spans))
            } else {
                let right: Vec<Span<'_>> = session
                    .agent_statuses
                    .iter()
                    .enumerate()
                    .flat_map(|(i, status)| {
                        let (label, color) = match status.state {
                            AgentState::Running => (&state.agent_labels.running, theme.accent),
                            AgentState::Waiting => (&state.agent_labels.waiting, theme.warning),
                            AgentState::Idle => (&state.agent_labels.idle, theme.muted),
                            AgentState::Unknown => (&state.agent_labels.unknown, theme.hint),
                        };
                        let mut spans = Vec::new();
                        if i > 0 {
                            spans.push(Span::raw(" "));
                        }
                        spans.push(Span::styled(label.clone(), Style::default().fg(color)));
                        spans
                    })
                    .collect();
                ListItem::new(Line::from(right_align_suffix(
                    &left_spans,
                    &right,
                    row_width,
                )))
            }
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

/// Combine left content with a right-aligned label, padding or truncating as needed.
fn right_align_suffix<'a>(
    left: &[Span<'a>],
    right: &[Span<'a>],
    row_width: usize,
) -> Vec<Span<'a>> {
    let right_width: usize = right.iter().map(Span::width).sum();

    if row_width < 1 + right_width {
        return truncate_spans(left, row_width);
    }

    let left_width: usize = left.iter().map(Span::width).sum();
    let available_for_left = row_width - 1 - right_width;

    let mut result;
    if left_width <= available_for_left {
        result = left.to_vec();
        let padding = row_width - left_width - right_width;
        result.push(Span::raw(" ".repeat(padding)));
    } else {
        result = truncate_spans(left, available_for_left);
        result.push(Span::raw(" "));
    }

    result.extend(right.iter().cloned());
    result
}

/// Truncate a span list to `max_width` display columns with ellipsis.
fn truncate_spans<'a>(spans: &[Span<'a>], max_width: usize) -> Vec<Span<'a>> {
    if max_width == 0 {
        return Vec::new();
    }

    let total_width: usize = spans.iter().map(Span::width).sum();
    if total_width <= max_width {
        return spans.to_vec();
    }

    let content_width = max_width - 1;
    let mut result = Vec::new();
    let mut used = 0;

    for span in spans {
        let span_w = span.width();
        if used + span_w <= content_width {
            result.push(span.clone());
            used += span_w;
        } else {
            let mut partial = String::new();
            for ch in span.content.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + cw > content_width {
                    break;
                }
                partial.push(ch);
                used += cw;
            }
            if !partial.is_empty() {
                result.push(Span::styled(partial, span.style));
            }
            break;
        }
    }

    result.push(Span::raw("…"));
    result
}
