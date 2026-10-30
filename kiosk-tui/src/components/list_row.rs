use kiosk_core::backend::Label;
use ratatui::{style::Style, text::Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

pub fn truncate_spans<'a>(spans: &[Span<'a>], max_width: usize) -> Vec<Span<'a>> {
    if max_width == 0 {
        return Vec::new();
    }
    if spans.iter().map(Span::width).sum::<usize>() <= max_width {
        return spans.to_vec();
    }

    let mut result = Vec::new();
    let mut used = 0;
    for span in spans {
        let mut partial = String::new();
        for grapheme in span.content.graphemes(true) {
            let width = grapheme.width();
            if used + width >= max_width {
                break;
            }
            partial.push_str(grapheme);
            used += width;
        }
        if !partial.is_empty() {
            result.push(Span::styled(partial, span.style));
        }
        if used + 1 >= max_width {
            break;
        }
    }
    result.push(Span::raw("…"));
    result
}

pub fn right_align_suffix<'a>(
    left: &[Span<'a>],
    right: &[Span<'a>],
    row_width: usize,
) -> Vec<Span<'a>> {
    let right_width = right.iter().map(Span::width).sum::<usize>();
    if row_width < right_width + 1 {
        return truncate_spans(left, row_width);
    }
    let available = row_width - right_width - 1;
    let mut result = truncate_spans(left, available);
    let left_width = result.iter().map(Span::width).sum::<usize>();
    result.push(Span::raw(" ".repeat(row_width - left_width - right_width)));
    result.extend(right.iter().cloned());
    result
}

/// A row's left content followed by the backend's labels and, for an open row, the open
/// marker, right-aligned.
pub fn with_status<'a>(
    left: &[Span<'a>],
    labels: &[Label],
    is_open: bool,
    theme: &Theme,
    row_width: usize,
) -> Vec<Span<'a>> {
    let mut right = Vec::new();
    for label in labels {
        if !right.is_empty() {
            right.push(Span::raw(" "));
        }
        right.push(Span::styled(
            kiosk_core::display::sanitize(&label.text).into_owned(),
            Style::default().fg(theme.tone(label.tone)),
        ));
    }
    if is_open {
        if !right.is_empty() {
            right.push(Span::raw("  "));
        }
        right.push(Span::styled("● open", Style::default().fg(theme.open)));
    }
    if right.is_empty() {
        truncate_spans(left, row_width)
    } else {
        right_align_suffix(left, &right, row_width)
    }
}

#[cfg(test)]
mod tests {
    use kiosk_core::{backend::Tone, config::ThemeConfig};

    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn suffix_is_right_aligned_and_left_text_is_truncated() {
        let spans = right_align_suffix(
            &[Span::raw("a very long repository")],
            &[Span::raw("● open")],
            15,
        );
        assert_eq!(spans.iter().map(Span::width).sum::<usize>(), 15);
        assert!(text(&spans).ends_with("● open"));
        assert!(text(&spans).contains('…'));
    }

    #[test]
    fn labels_come_before_the_open_marker_and_rows_without_either_are_unchanged() {
        let theme = Theme::from_config(&ThemeConfig::default());
        let labels = [
            Label {
                text: "claude waiting".into(),
                tone: Tone::Attention,
            },
            Label {
                text: "codex working".into(),
                tone: Tone::Progress,
            },
        ];
        let spans = with_status(&[Span::raw("main")], &labels, true, &theme, 50);
        assert!(text(&spans).ends_with("claude waiting codex working  ● open"));
        assert_eq!(spans.iter().map(Span::width).sum::<usize>(), 50);
        assert_eq!(
            text(&with_status(&[Span::raw("main")], &[], false, &theme, 50)),
            "main"
        );
        assert_eq!(
            text(&with_status(
                &[Span::raw("main")],
                &labels[..1],
                false,
                &theme,
                30
            )),
            format!("main{}claude waiting", " ".repeat(12))
        );
    }

    #[test]
    fn truncation_keeps_multi_scalar_graphemes_intact() {
        let spans = truncate_spans(&[Span::raw("a👩‍💻bc")], 4);

        assert_eq!(text(&spans), "a👩‍💻…");
    }
}
