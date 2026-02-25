use super::dialog::Dialog;
use crate::theme::Theme;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Padding,
};

fn build_error_dialog<'a>(error: &'a str, theme: &Theme) -> Dialog<'a> {
    let text = Line::from(vec![
        Span::styled(
            "Error: ",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(error),
    ]);

    Dialog::new(vec![text])
        .border_color(theme.error)
        .title(" Error ")
        .padding(Padding::uniform(1))
        .alignment(Alignment::Center)
}

/// Compute the width and height for an error toast dialog.
pub fn error_toast_size(error: &str, theme: &Theme, terminal_width: u16) -> (u16, u16) {
    build_error_dialog(error, theme).size(terminal_width)
}

/// Draw an error toast popup centered on the screen.
pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if let Some(error) = &state.error {
        build_error_dialog(error, theme).render(f, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use kiosk_core::config::ThemeConfig;

    fn test_theme() -> Theme {
        Theme::from_config(&ThemeConfig::default())
    }

    #[test]
    fn test_error_toast_size_short_message() {
        let theme = test_theme();
        let (w, h) = error_toast_size("Something failed", &theme, 100);
        assert_eq!(w, 80);
        assert_eq!(h, 1 + 4); // 1 line + 4 chrome
    }

    #[test]
    fn test_error_toast_size_long_message() {
        let theme = test_theme();
        let long_msg = "a]".repeat(100);
        let (w, h) = error_toast_size(&long_msg, &theme, 100);
        assert_eq!(w, 80);
        assert!(h > 5);
    }

    #[test]
    fn test_error_toast_size_narrow_terminal() {
        let theme = test_theme();
        let (w, h) = error_toast_size("fail", &theme, 20);
        assert_eq!(w, 16);
        assert!(h >= 5);
    }
}
