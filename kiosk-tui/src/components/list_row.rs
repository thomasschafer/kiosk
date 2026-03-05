use crate::theme::Theme;
use kiosk_core::{agent::AgentStatus, config::AgentLabelsConfig};
use ratatui::{style::Style, text::Span};
use unicode_width::UnicodeWidthChar;

/// Truncate a span list to `max_width` display columns with ellipsis.
pub fn truncate_spans<'a>(spans: &[Span<'a>], max_width: usize) -> Vec<Span<'a>> {
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
pub fn right_align_suffix<'a>(
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

/// Render color-coded status badges for all detected agents.
pub fn render_agent_badges(
    statuses: &[AgentStatus],
    labels: &AgentLabelsConfig,
    theme: &Theme,
) -> Vec<Span<'static>> {
    statuses
        .iter()
        .enumerate()
        .flat_map(|(i, status)| {
            let (label, color) = match status.state {
                kiosk_core::AgentState::Running => (&labels.running, theme.accent),
                kiosk_core::AgentState::Waiting => (&labels.waiting, theme.warning),
                kiosk_core::AgentState::Idle => (&labels.idle, theme.muted),
                kiosk_core::AgentState::Unknown => (&labels.unknown, theme.hint),
            };
            let mut spans = Vec::new();
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(label.clone(), Style::default().fg(color)));
            spans
        })
        .collect()
}

/// Render a list row with optional right-aligned agent badges.
pub fn render_with_optional_agent_badges<'a>(
    left: &[Span<'a>],
    statuses: &[AgentStatus],
    labels: &AgentLabelsConfig,
    theme: &Theme,
    row_width: usize,
) -> Vec<Span<'a>> {
    if statuses.is_empty() {
        left.to_vec()
    } else {
        let right = render_agent_badges(statuses, labels, theme);
        right_align_suffix(left, &right, row_width)
    }
}

#[cfg(test)]
mod tests {
    use super::{right_align_suffix, truncate_spans};
    use ratatui::style::{Color, Style};
    use ratatui::text::Span;

    fn total_width(spans: &[Span]) -> usize {
        spans.iter().map(ratatui::prelude::Span::width).sum()
    }

    fn concat_text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

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
        assert_eq!(result[0].style, Style::default().fg(Color::Red));
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
        let result = right_align_suffix(&left, &right, 9);
        assert_eq!(concat_text(&result), "main");
    }

    #[test]
    fn right_align_label_just_fits() {
        let left = vec![Span::raw("m")];
        let right = vec![Span::styled("[RUNNING]", Style::default().fg(Color::Red))];
        let result = right_align_suffix(&left, &right, 11);
        assert_eq!(concat_text(&result), "m [RUNNING]");
    }

    #[test]
    fn right_align_label_boundary_drop() {
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("[UNKNOWN]", Style::default().fg(Color::Gray))];
        let result = right_align_suffix(&left, &right, 9);
        assert!(!concat_text(&result).contains("[UNKNOWN]"));
    }

    #[test]
    fn right_align_label_boundary_keep() {
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

    #[test]
    fn right_align_custom_no_brackets() {
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("RUNNING", Style::default().fg(Color::Red))];
        let result = right_align_suffix(&left, &right, 20);
        assert_eq!(concat_text(&result), "main         RUNNING");
    }

    #[test]
    fn right_align_custom_short_label() {
        let left = vec![Span::raw("main")];
        let right = vec![Span::styled("W", Style::default().fg(Color::Yellow))];
        let result = right_align_suffix(&left, &right, 8);
        assert_eq!(concat_text(&result), "main   W");
    }
}
