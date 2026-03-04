use crate::theme::Theme;
use kiosk_core::agent::AgentState;
use kiosk_core::config::KeysConfig;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState},
};
use unicode_width::UnicodeWidthChar;

/// Truncate a span list to `max_width` display columns, replacing the last
/// character position with `…` if truncation occurs.
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
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
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

#[allow(clippy::too_many_lines)]
pub fn draw(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    _keys: &KeysConfig,
    show_selection: bool,
) {
    let repo_name = state
        .selected_repo_idx
        .map_or("??", |i| state.repos[i].name.as_str());
    let selected_repo_path = state.selected_repo_idx.map(|i| state.repos[i].path.clone());

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    let title = format!("{repo_name} — select branch");
    super::search_bar::draw(
        f,
        chunks[0],
        &super::search_bar::SearchBarStyle {
            title: &title,
            placeholder: "Type to search branches (or type new branch name)...",
            border_color: theme.secondary,
            muted_color: theme.muted,
        },
        &state.branch_list.input.text,
        state.branch_list.input.cursor,
    );

    // Row width available for content (list area minus borders minus highlight symbol "▸ ")
    let row_width = (chunks[1].width as usize).saturating_sub(4);

    // Branch list
    let mut items: Vec<ListItem> = state
        .branch_list
        .filtered
        .iter()
        .map(|(idx, _)| {
            let branch = &state.branches[*idx];
            let mut left_spans: Vec<Span<'_>> = Vec::new();

            if let Some(remote) = &branch.remote {
                left_spans.push(Span::styled(&branch.name, Style::default().fg(theme.muted)));
                left_spans.push(Span::styled(
                    format!(" ({remote})"),
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::ITALIC),
                ));
            } else {
                left_spans.push(Span::raw(&branch.name));
                let is_deleting = selected_repo_path.as_ref().is_some_and(|repo_path| {
                    state.is_branch_pending_delete(repo_path, &branch.name)
                });

                if is_deleting {
                    left_spans.push(Span::styled(
                        " (deleting...)",
                        Style::default().fg(theme.accent),
                    ));
                } else if branch.has_session {
                    left_spans.push(Span::styled(
                        " (session)",
                        Style::default().fg(theme.success),
                    ));
                } else if branch.worktree_path.is_some() {
                    left_spans.push(Span::styled(
                        " (worktree)",
                        Style::default().fg(theme.warning),
                    ));
                }

                if branch.is_current {
                    left_spans.push(Span::styled(" *", Style::default().fg(theme.accent)));
                }
            }

            if let Some(ref agent_status) = branch.agent_status {
                let (label, color) = match agent_status.state {
                    AgentState::Running => (&state.agent_labels.running, theme.accent),
                    AgentState::Waiting => (&state.agent_labels.waiting, theme.warning),
                    AgentState::Idle => (&state.agent_labels.idle, theme.muted),
                    AgentState::Unknown => (&state.agent_labels.unknown, theme.hint),
                };
                let right = vec![Span::styled(label.clone(), Style::default().fg(color))];
                ListItem::new(Line::from(right_align_suffix(
                    &left_spans,
                    &right,
                    row_width,
                )))
            } else {
                ListItem::new(Line::from(left_spans))
            }
        })
        .collect();

    // If search doesn't match anything, show "create new branch" option
    if state.loading_branches && state.branch_list.filtered.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "Loading branches...",
            Style::default().fg(theme.muted),
        ))));
    } else if state.branch_list.filtered.is_empty() && !state.branch_list.input.text.is_empty() {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("+ Create branch ", Style::default().fg(theme.success)),
            Span::styled(
                format!("\"{}\"", state.branch_list.input.text),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" (Enter to pick base)", Style::default().fg(theme.muted)),
        ])));
    }

    let count = state.branch_list.filtered.len();
    let loading_suffix = if state.loading_branches {
        " | loading..."
    } else if state.fetching_remotes {
        " | fetching..."
    } else {
        ""
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {count} branches{loading_suffix} "))
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.secondary)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always);

    let mut list_state = ListState::default();
    list_state.select(show_selection.then_some(state.branch_list.selected).flatten());
    *list_state.offset_mut() = state.branch_list.scroll_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};
    use ratatui::text::Span;

    fn total_width(spans: &[Span]) -> usize {
        spans.iter().map(ratatui::prelude::Span::width).sum()
    }

    fn concat_text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    // --- truncate_spans ---

    #[test]
    fn truncate_spans_no_truncation_needed() {
        let spans = vec![Span::raw("hello")];
        let result = truncate_spans(&spans, 10);
        assert_eq!(concat_text(&result), "hello");
        assert_eq!(total_width(&result), 5);
    }

    #[test]
    fn truncate_spans_exact_fit() {
        let spans = vec![Span::raw("hello")];
        let result = truncate_spans(&spans, 5);
        assert_eq!(concat_text(&result), "hello");
        assert_eq!(total_width(&result), 5);
    }

    #[test]
    fn truncate_spans_with_ellipsis() {
        let spans = vec![Span::raw("hello world")];
        let result = truncate_spans(&spans, 6);
        assert_eq!(concat_text(&result), "hello\u{2026}");
        assert_eq!(total_width(&result), 6);
    }

    #[test]
    fn truncate_spans_multi_span() {
        let spans = vec![
            Span::styled("abc", Style::default().fg(Color::Red)),
            Span::styled("def", Style::default().fg(Color::Blue)),
        ];
        let result = truncate_spans(&spans, 5);
        assert_eq!(concat_text(&result), "abcd\u{2026}");
        assert_eq!(total_width(&result), 5);
        // First span preserved with its style
        assert_eq!(result[0].style, Style::default().fg(Color::Red));
        // Partial second span keeps its style
        assert_eq!(result[1].style, Style::default().fg(Color::Blue));
    }

    #[test]
    fn truncate_spans_zero_width() {
        let spans = vec![Span::raw("hello")];
        let result = truncate_spans(&spans, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn truncate_spans_width_one() {
        let spans = vec![Span::raw("hello")];
        let result = truncate_spans(&spans, 1);
        assert_eq!(concat_text(&result), "\u{2026}");
        assert_eq!(total_width(&result), 1);
    }

    // --- right_align_suffix ---
    //
    // Labels are rendered verbatim from config (including any brackets),
    // so these tests use "[RUNNING]" for defaults and plain "RUNNING"
    // or short strings for custom configs.

    #[test]
    fn right_align_wide_terminal() {
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("[RUNNING]", Style::default().fg(Color::Red))];
        let result = right_align_suffix(&left, &right, 40);
        assert_eq!(total_width(&result), 40);
        let text = concat_text(&result);
        assert!(text.starts_with("main"), "got: {text}");
        assert!(text.ends_with("[RUNNING]"), "got: {text}");
    }

    #[test]
    fn right_align_exact_fit() {
        // "main" (4) + " " (1) + "[WAITING]" (9) = 14
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled(
            "[WAITING]",
            Style::default().fg(Color::Yellow),
        )];
        let result = right_align_suffix(&left, &right, 14);
        assert_eq!(total_width(&result), 14);
        assert_eq!(concat_text(&result), "main [WAITING]");
    }

    #[test]
    fn right_align_branch_truncated() {
        let left = vec![Span::raw("very-long-branch-name")];
        let right = vec![Span::styled(
            "[WAITING]",
            Style::default().fg(Color::Yellow),
        )];
        let result = right_align_suffix(&left, &right, 25);
        assert_eq!(total_width(&result), 25);
        let text = concat_text(&result);
        assert!(text.ends_with(" [WAITING]"), "label missing: {text}");
        assert!(text.contains('\u{2026}'), "no ellipsis: {text}");
    }

    #[test]
    fn right_align_multi_span_truncated() {
        let left = vec![
            Span::raw("long-branch"),
            Span::styled(" (session)", Style::default().fg(Color::Green)),
        ];
        let right = vec![Span::styled("[RUNNING]", Style::default().fg(Color::Red))];
        let result = right_align_suffix(&left, &right, 22);
        assert_eq!(total_width(&result), 22);
        let text = concat_text(&result);
        assert!(text.ends_with(" [RUNNING]"), "label missing: {text}");
        assert!(text.contains('\u{2026}'), "no ellipsis: {text}");
    }

    #[test]
    fn right_align_label_dropped_when_too_narrow() {
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled(
            "[WAITING]",
            Style::default().fg(Color::Yellow),
        )];
        // need 1 + 9 = 10; row=9 < 10, label dropped
        let result = right_align_suffix(&left, &right, 9);
        assert_eq!(concat_text(&result), "main");
    }

    #[test]
    fn right_align_label_just_fits() {
        let left = vec![Span::raw("m")];
        let right = vec![Span::styled("[RUNNING]", Style::default().fg(Color::Red))];
        // "m" (1) + " " (1) + "[RUNNING]" (9) = 11
        let result = right_align_suffix(&left, &right, 11);
        assert_eq!(concat_text(&result), "m [RUNNING]");
    }

    #[test]
    fn right_align_label_boundary_drop() {
        // row = right_width (9) < 1 + 9 = 10, so label dropped
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("[UNKNOWN]", Style::default().fg(Color::Gray))];
        let result = right_align_suffix(&left, &right, 9);
        assert!(!concat_text(&result).contains("[UNKNOWN]"));
    }

    #[test]
    fn right_align_label_boundary_keep() {
        // row = 10, 10 < 10 is false, label kept
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("[UNKNOWN]", Style::default().fg(Color::Gray))];
        let result = right_align_suffix(&left, &right, 10);
        assert!(concat_text(&result).contains("[UNKNOWN]"));
    }

    #[test]
    fn right_align_very_narrow() {
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("[IDLE]", Style::default().fg(Color::DarkGray))];
        let result = right_align_suffix(&left, &right, 3);
        let text = concat_text(&result);
        assert!(!text.contains("[IDLE]"));
        assert_eq!(total_width(&result), 3);
    }

    // --- Custom label configs (no brackets, short strings) ---

    #[test]
    fn right_align_custom_no_brackets() {
        // User configured labels without brackets: running = "RUNNING"
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("RUNNING", Style::default().fg(Color::Red))];
        let result = right_align_suffix(&left, &right, 20);
        assert_eq!(concat_text(&result), "main         RUNNING");
    }

    #[test]
    fn right_align_custom_short_label() {
        // User configured short labels: waiting = "W"
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("W", Style::default().fg(Color::Yellow))];
        let result = right_align_suffix(&left, &right, 8);
        assert_eq!(concat_text(&result), "main   W");
    }
}
