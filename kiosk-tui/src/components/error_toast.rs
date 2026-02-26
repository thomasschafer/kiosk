use super::dialog::Dialog;
use crate::theme::Theme;
use kiosk_core::{
    config::{KeysConfig, keys::Command},
    state::AppState,
};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Padding,
};

fn build_error_dialog<'a>(error: &'a str, dismiss_key: &str, theme: &Theme) -> Dialog<'a> {
    let text = Line::from(vec![
        Span::styled(
            "Error: ",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(error),
    ]);

    let hint = Line::from(vec![
        Span::styled(
            dismiss_key.to_string(),
            Style::default().fg(theme.hint).add_modifier(Modifier::BOLD),
        ),
        Span::raw(": close"),
    ]);

    Dialog::new(vec![text, Line::raw(""), hint])
        .border_color(theme.error)
        .title(" Error ")
        .padding(Padding::uniform(1))
        .alignment(Alignment::Center)
}

fn cancel_key_label(keys: &KeysConfig) -> String {
    KeysConfig::find_key(&keys.modal, &Command::Cancel).map_or("esc".to_string(), |k| k.to_string())
}

/// Compute the width and height for an error toast dialog.
pub fn error_toast_size(
    error: &str,
    keys: &KeysConfig,
    theme: &Theme,
    terminal_width: u16,
) -> (u16, u16) {
    build_error_dialog(error, &cancel_key_label(keys), theme).size(terminal_width)
}

/// Draw an error toast popup centered on the screen.
pub fn draw(f: &mut Frame, area: Rect, state: &AppState, keys: &KeysConfig, theme: &Theme) {
    if let Some(error) = &state.error {
        build_error_dialog(error, &cancel_key_label(keys), theme).render(f, area);
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

    fn test_keys() -> KeysConfig {
        KeysConfig::default()
    }

    #[test]
    fn test_error_toast_size_short_message() {
        let theme = test_theme();
        let keys = test_keys();
        let (w, h) = error_toast_size("Something failed", &keys, &theme, 100);
        assert_eq!(w, 80);
        assert_eq!(h, 3 + 4); // 3 lines (error + blank + hint) + 4 chrome
    }

    #[test]
    fn test_error_toast_size_long_message() {
        let theme = test_theme();
        let keys = test_keys();
        let long_msg = "a]".repeat(100);
        let (w, h) = error_toast_size(&long_msg, &keys, &theme, 100);
        assert_eq!(w, 80);
        assert!(h > 5);
    }

    #[test]
    fn test_error_toast_size_narrow_terminal() {
        let theme = test_theme();
        let keys = test_keys();
        let (w, h) = error_toast_size("fail", &keys, &theme, 20);
        assert_eq!(w, 16);
        assert!(h >= 5);
    }
}
