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

fn build_error_dialog<'a>(error: &'a str, dismiss_key: &'a str, theme: &Theme) -> Dialog<'a> {
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
            dismiss_key,
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
    use kiosk_core::state::AppState;
    use ratatui::{Terminal, backend::TestBackend};

    fn test_theme() -> Theme {
        Theme::from_config(&ThemeConfig::default())
    }

    fn render_error_toast(error: &str, width: u16, height: u16) -> String {
        let theme = test_theme();
        let mut state = AppState::new(vec![], None);
        state.error = Some(error.to_string());

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw(f, f.area(), &state, &KeysConfig::default(), &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        let mut output = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                output.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            output.push('\n');
        }
        output
    }

    #[test]
    fn test_error_toast_renders_short_message() {
        let output = render_error_toast("Something failed", 100, 20);
        assert!(output.contains("Error:"), "should contain 'Error:' label");
        assert!(
            output.contains("Something failed"),
            "should contain the error message"
        );
        assert!(output.contains("Error"), "should contain border title");
    }

    #[test]
    fn test_error_toast_renders_long_message() {
        let long_msg = "a".repeat(200);
        let output = render_error_toast(&long_msg, 100, 30);
        assert!(output.contains("Error:"), "should contain 'Error:' label");
        // Long message should be word-wrapped, so multiple lines contain 'a's
        let content_lines: Vec<&str> = output
            .lines()
            .filter(|l| l.contains("aaa"))
            .collect();
        assert!(
            content_lines.len() > 1,
            "long message should wrap across multiple lines"
        );
    }

    #[test]
    fn test_error_toast_renders_in_narrow_terminal() {
        let output = render_error_toast("fail", 20, 15);
        assert!(output.contains("Error:"), "should contain 'Error:' label");
        assert!(output.contains("fail"), "should contain the error message");
    }

    #[test]
    fn test_no_error_renders_nothing() {
        let theme = test_theme();
        let keys = test_keys();
        let state = AppState::new(vec![], None); // no error set

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw(f, f.area(), &state, &keys, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        let mut output = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                output.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
        }
        assert!(!output.contains("Error"), "should not render anything when no error");
    }
}
