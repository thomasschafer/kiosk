use super::{centered_fixed_rect, dialog_width};
use crate::theme::Theme;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

/// Compute the width and height for an error toast dialog.
pub fn error_toast_size(error: &str, terminal_width: u16) -> (u16, u16) {
    let width = dialog_width(terminal_width);
    // 2 for borders + 2 for 1-cell horizontal padding on each side
    let h_chrome: u16 = 4;
    // 2 for borders + 2 for vertical padding (top/bottom)
    let v_chrome: u16 = 4;
    let text_width = width.saturating_sub(h_chrome).max(1);

    let line = Line::from(vec![Span::raw("Error: "), Span::raw(error)]);
    let content_height = crate::app::word_wrapped_line_count(&line, text_width);
    (width, content_height + v_chrome)
}

/// Draw an error toast popup centered on the screen.
pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if let Some(error) = &state.error {
        let text = Line::from(vec![
            Span::styled(
                "Error: ",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(error.as_str()),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Error ")
            .border_style(Style::default().fg(theme.error))
            .padding(Padding::uniform(1));

        let (width, height) = error_toast_size(error, area.width);
        let centered = centered_fixed_rect(width, height, area);
        f.render_widget(Clear, centered);

        let paragraph = Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Center);
        f.render_widget(paragraph, centered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_toast_size_short_message() {
        let (w, h) = error_toast_size("Something failed", 100);
        assert_eq!(w, 80); // dialog_width(100) = 80
        // "Error: Something failed" fits in one line (23 chars < 76 text_width)
        assert_eq!(h, 1 + 4); // 1 line + 4 chrome
    }

    #[test]
    fn test_error_toast_size_long_message() {
        let long_msg = "a]".repeat(100); // 200 chars
        let (w, h) = error_toast_size(&long_msg, 100);
        assert_eq!(w, 80);
        assert!(h > 5); // should wrap to multiple lines
    }

    #[test]
    fn test_error_toast_size_narrow_terminal() {
        let (w, h) = error_toast_size("fail", 20);
        assert_eq!(w, 16); // dialog_width(20) = 16
        assert!(h >= 5); // at least 1 line + chrome
    }
}
