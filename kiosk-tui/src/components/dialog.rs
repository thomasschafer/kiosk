use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

use super::{centered_fixed_rect, dialog_width};

/// A reusable centered dialog popup.
///
/// Handles width computation, word-wrap height estimation, centering, background
/// clearing, and rendering — the shared logic previously duplicated across the
/// loading popup, confirm-delete dialog, and error toast.
pub struct Dialog<'a> {
    lines: Vec<Line<'a>>,
    border_color: Color,
    title: Option<&'a str>,
    padding: Padding,
    alignment: Alignment,
}

impl<'a> Dialog<'a> {
    #[must_use]
    pub fn new(lines: Vec<Line<'a>>) -> Self {
        Self {
            lines,
            border_color: Color::White,
            title: None,
            padding: Padding::ZERO,
            alignment: Alignment::Left,
        }
    }

    #[must_use]
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    #[must_use]
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    #[must_use]
    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    #[must_use]
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Horizontal chrome: 2 (borders) + left padding + right padding.
    fn h_chrome(&self) -> u16 {
        2 + self.padding.left + self.padding.right
    }

    /// Vertical chrome: 2 (borders) + top padding + bottom padding.
    fn v_chrome(&self) -> u16 {
        2 + self.padding.top + self.padding.bottom
    }

    /// Compute `(width, height)` for this dialog given the terminal width.
    pub fn size(&self, terminal_width: u16) -> (u16, u16) {
        let width = dialog_width(terminal_width);
        let text_width = width.saturating_sub(self.h_chrome()).max(1);

        let content_height: u16 = self
            .lines
            .iter()
            .map(|line| word_wrapped_line_count(line, text_width))
            .sum();

        (width, content_height + self.v_chrome())
    }

    /// Render this dialog centered on `area`, clearing the background first.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let (width, height) = self.size(area.width);
        let centered = centered_fixed_rect(width, height, area);

        f.render_widget(Clear, centered);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.border_color));

        if let Some(title) = self.title {
            block = block.title(title);
        }

        block = block.padding(self.padding);

        let paragraph = Paragraph::new(self.lines.clone())
            .block(block)
            .wrap(Wrap { trim: false })
            .alignment(self.alignment);

        f.render_widget(paragraph, centered);
    }
}

/// Estimate visual line count when a `Line` is word-wrapped to `max_width` columns.
/// Uses byte length as a width proxy, which is exact for ASCII and a safe overestimate
/// for multi-byte UTF-8 (produces a taller dialog rather than clipping content).
pub fn word_wrapped_line_count(line: &Line, max_width: u16) -> u16 {
    let max_w = usize::from(max_width);
    if max_w == 0 {
        return 1;
    }

    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    if text.is_empty() {
        return 1;
    }

    let mut lines: u16 = 1;
    let mut col: usize = 0;

    for (i, word) in text.split(' ').enumerate() {
        let w = word.len();
        let needed = if i == 0 || col == 0 { w } else { w + 1 };

        if col + needed <= max_w {
            col += needed;
        } else if w <= max_w {
            lines += 1;
            col = w;
        } else {
            if col > 0 {
                lines += 1;
            }
            col = w;
            while col > max_w {
                lines += 1;
                col -= max_w;
            }
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;
    use ratatui::text::Span;

    // -- word_wrapped_line_count tests --

    #[test]
    fn test_word_wrap_single_line_no_wrap() {
        let line = Line::raw("hello world");
        assert_eq!(word_wrapped_line_count(&line, 20), 1);
    }

    #[test]
    fn test_word_wrap_exact_fit() {
        let line = Line::raw("hello world");
        assert_eq!(word_wrapped_line_count(&line, 11), 1);
    }

    #[test]
    fn test_word_wrap_breaks_at_word_boundary() {
        let line = Line::raw("hello world");
        assert_eq!(word_wrapped_line_count(&line, 10), 2);
    }

    #[test]
    fn test_word_wrap_multiple_wraps() {
        let line = Line::raw("one two three four");
        assert_eq!(word_wrapped_line_count(&line, 5), 4);
    }

    #[test]
    fn test_word_wrap_oversized_word() {
        let line = Line::raw("abcdefghij");
        assert_eq!(word_wrapped_line_count(&line, 4), 3);
    }

    #[test]
    fn test_word_wrap_oversized_word_exact_multiple() {
        let line = Line::raw("abcdefgh");
        assert_eq!(word_wrapped_line_count(&line, 4), 2);
    }

    #[test]
    fn test_word_wrap_oversized_after_short_word() {
        let line = Line::raw("hi abcdefghij");
        assert_eq!(word_wrapped_line_count(&line, 6), 3);
    }

    #[test]
    fn test_word_wrap_empty_line() {
        let line = Line::raw("");
        assert_eq!(word_wrapped_line_count(&line, 20), 1);
    }

    #[test]
    fn test_word_wrap_zero_width() {
        let line = Line::raw("hello");
        assert_eq!(word_wrapped_line_count(&line, 0), 1);
    }

    #[test]
    fn test_word_wrap_multi_span_line() {
        let line = Line::from(vec![
            Span::raw("hello "),
            Span::styled("world", Style::default().fg(Color::Red)),
        ]);
        assert_eq!(word_wrapped_line_count(&line, 20), 1);
        assert_eq!(word_wrapped_line_count(&line, 8), 2);
    }

    // -- Dialog size tests --

    #[test]
    fn test_dialog_size_no_padding() {
        let dialog = Dialog::new(vec![Line::raw("hello")]);
        let (w, h) = dialog.size(100);
        assert_eq!(w, 80);
        assert_eq!(h, 3); // 1 content + 2 border
    }

    #[test]
    fn test_dialog_size_with_uniform_padding() {
        let dialog = Dialog::new(vec![Line::raw("hello")]).padding(Padding::uniform(1));
        let (w, h) = dialog.size(100);
        assert_eq!(w, 80);
        assert_eq!(h, 5); // 1 content + 4 chrome
    }

    #[test]
    fn test_dialog_size_with_horizontal_padding() {
        let dialog = Dialog::new(vec![Line::raw("hello")]).padding(Padding::horizontal(1));
        let (w, h) = dialog.size(100);
        assert_eq!(w, 80);
        assert_eq!(h, 3); // 1 content + 2 v_chrome (no vertical padding)
    }

    #[test]
    fn test_dialog_size_multiple_lines() {
        let dialog = Dialog::new(vec![
            Line::raw("line one"),
            Line::raw(""),
            Line::raw("line three"),
        ])
        .padding(Padding::uniform(1));
        let (_w, h) = dialog.size(100);
        assert_eq!(h, 7); // 3 content + 4 chrome
    }

    #[test]
    fn test_dialog_size_wrapping() {
        let long_text = "a ".repeat(50);
        let dialog = Dialog::new(vec![Line::raw(long_text.trim())]).padding(Padding::uniform(1));
        let (_w, h) = dialog.size(100);
        assert!(h > 5, "should wrap, height={h}");
    }

    #[test]
    fn test_dialog_builder_defaults() {
        let dialog = Dialog::new(vec![Line::raw("test")]);
        assert_eq!(dialog.border_color, Color::White);
        assert!(dialog.title.is_none());
        assert_eq!(dialog.alignment, Alignment::Left);
    }

    #[test]
    fn test_dialog_builder_methods() {
        let dialog = Dialog::new(vec![Line::raw("test")])
            .border_color(Color::Red)
            .title("My Title")
            .alignment(Alignment::Center)
            .padding(Padding::uniform(2));
        assert_eq!(dialog.border_color, Color::Red);
        assert_eq!(dialog.title, Some("My Title"));
        assert_eq!(dialog.alignment, Alignment::Center);
        assert_eq!(dialog.padding, Padding::uniform(2));
    }

    #[test]
    fn test_dialog_chrome_computed_from_padding() {
        let d1 = Dialog::new(vec![]).padding(Padding::ZERO);
        assert_eq!(d1.h_chrome(), 2);
        assert_eq!(d1.v_chrome(), 2);

        let d2 = Dialog::new(vec![]).padding(Padding::uniform(1));
        assert_eq!(d2.h_chrome(), 4);
        assert_eq!(d2.v_chrome(), 4);

        let d3 = Dialog::new(vec![]).padding(Padding::horizontal(1));
        assert_eq!(d3.h_chrome(), 4);
        assert_eq!(d3.v_chrome(), 2);

        let d4 = Dialog::new(vec![]).padding(Padding::new(1, 2, 3, 4));
        assert_eq!(d4.h_chrome(), 2 + 1 + 2);
        assert_eq!(d4.v_chrome(), 2 + 3 + 4);
    }

    // -- Error toast via Dialog --

    #[test]
    fn test_error_toast_size_short_message() {
        let dialog = Dialog::new(vec![Line::from(vec![
            Span::raw("Error: "),
            Span::raw("Something failed"),
        ])])
        .padding(Padding::uniform(1));
        let (w, h) = dialog.size(100);
        assert_eq!(w, 80);
        assert_eq!(h, 5); // 1 line + 4 chrome
    }

    #[test]
    fn test_error_toast_size_long_message() {
        let long_msg = "a]".repeat(100);
        let dialog = Dialog::new(vec![Line::from(vec![
            Span::raw("Error: "),
            Span::raw(long_msg),
        ])])
        .padding(Padding::uniform(1));
        let (_w, h) = dialog.size(100);
        assert!(h > 5);
    }

    #[test]
    fn test_error_toast_size_narrow_terminal() {
        let dialog = Dialog::new(vec![Line::from(vec![
            Span::raw("Error: "),
            Span::raw("fail"),
        ])])
        .padding(Padding::uniform(1));
        let (w, h) = dialog.size(20);
        assert_eq!(w, 16);
        assert!(h >= 5);
    }

    // -- Loading dialog via Dialog --

    #[test]
    fn test_loading_dialog_size_short_message() {
        let dialog = Dialog::new(vec![Line::from(vec![
            Span::raw("⠋ "),
            Span::raw("Fetching..."),
        ])])
        .padding(Padding::horizontal(1));
        let (w, h) = dialog.size(100);
        assert_eq!(w, 80);
        assert_eq!(h, 3); // 1 line + 2 v_chrome (horizontal padding only)
    }

    #[test]
    fn test_loading_dialog_size_long_message() {
        let msg = "a".repeat(100);
        let dialog = Dialog::new(vec![Line::from(vec![Span::raw("⠋ "), Span::raw(msg)])])
            .padding(Padding::horizontal(1));
        let (_w, h) = dialog.size(60);
        assert!(h > 3, "long message should wrap, height={h}");
    }

    #[test]
    fn test_loading_dialog_size_narrow_terminal() {
        let dialog = Dialog::new(vec![Line::from(vec![
            Span::raw("⠋ "),
            Span::raw("Creating branch foo from bar..."),
        ])])
        .padding(Padding::horizontal(1));
        let (w, _h) = dialog.size(30);
        assert!(w <= 30);
    }

    #[test]
    fn test_loading_dialog_width_scales_with_terminal() {
        let make = || {
            Dialog::new(vec![Line::from(vec![Span::raw("⠋ "), Span::raw("test")])])
                .padding(Padding::horizontal(1))
        };
        let (w1, _) = make().size(80);
        let (w2, _) = make().size(40);
        assert!(w1 > w2);
    }
}
