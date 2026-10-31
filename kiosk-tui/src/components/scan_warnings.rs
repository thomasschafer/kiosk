use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;
use kiosk_core::{
    config::keys::KeysConfig,
    git::ScanWarning,
    state::{ScanWarningsOverlayState, scan_summary},
};

const REASON_INDENT: &str = "  ";

/// Splits `text` into rows of at most `width` columns, breaking anywhere, so paths with no
/// spaces still wrap and the scroll range is exactly the number of rows drawn.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if used > 0 && used + grapheme_width > width {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut()
            .expect("rows is never empty")
            .push_str(grapheme);
        used += grapheme_width;
    }
    rows
}

fn content_lines(warnings: &[ScanWarning], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let reason_width = width.saturating_sub(REASON_INDENT.len());
    let mut lines = Vec::new();
    for (index, warning) in warnings.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let path = warning.path.display().to_string();
        lines.extend(
            hard_wrap(&kiosk_core::display::sanitize(&path), width)
                .into_iter()
                .map(|row| Line::styled(row, Style::default().add_modifier(Modifier::BOLD))),
        );
        lines.extend(
            hard_wrap(
                &kiosk_core::display::sanitize(&warning.message),
                reason_width,
            )
            .into_iter()
            .map(|row| Line::styled(format!("{REASON_INDENT}{row}"), theme.muted)),
        );
    }
    lines
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut ScanWarningsOverlayState,
    warnings: &[ScanWarning],
    theme: &Theme,
    keys: &KeysConfig,
) {
    let width = area.width.saturating_mul(4) / 5;
    let height = area.height.saturating_mul(9) / 10;
    let [horizontal] = Layout::horizontal([Constraint::Length(width.max(1))])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height.max(1))])
        .flex(Flex::Center)
        .areas(horizontal);
    frame.render_widget(Clear, popup);

    let mut hints = super::dialog::move_hints(keys, crate::keymap::SCAN_WARNINGS_BINDING_MODE)
        .into_iter()
        .map(|(key, label)| {
            let label = match label {
                "move up" => "scroll up",
                "move down" => "scroll down",
                _ => "scroll",
            };
            (key, label)
        })
        .collect::<Vec<_>>();
    hints.push(("esc".into(), "close"));
    let mut hint_line = super::dialog::hint_line(&hints, theme).expect("esc always closes");
    hint_line.spans.insert(0, Span::raw(" "));
    hint_line.spans.push(Span::raw(" "));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning))
        .title(format!(" {} ", scan_summary(warnings.len())))
        .title_bottom(hint_line)
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);

    let lines = content_lines(warnings, usize::from(inner.width), theme);
    let max_offset = lines.len().saturating_sub(usize::from(inner.height));
    overlay.scroll_offset = overlay.scroll_offset.min(max_offset);
    let scroll = u16::try_from(overlay.scroll_offset).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(lines).block(block).scroll((scroll, 0)),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::config::ThemeConfig;
    use ratatui::{Terminal, backend::TestBackend};

    fn warning(path: &str, message: &str) -> ScanWarning {
        ScanWarning {
            path: path.into(),
            message: message.into(),
        }
    }

    fn render(
        overlay: &mut ScanWarningsOverlayState,
        warnings: &[ScanWarning],
        size: (u16, u16),
    ) -> Vec<String> {
        let theme = Theme::from_config(&ThemeConfig::default());
        let keys = KeysConfig::default();
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                draw(frame, area, overlay, warnings, &theme, &keys);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn hard_wrap_breaks_long_words_by_display_width() {
        assert_eq!(hard_wrap("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(hard_wrap("", 4), [""]);
        assert_eq!(hard_wrap("界界界", 4), ["界界", "界"]);
        assert_eq!(hard_wrap("abc", 0), ["a", "b", "c"]);
    }

    #[test]
    fn lists_each_path_with_its_reason_and_the_close_hint() {
        let warnings = [
            warning("/src/locked", "failed to read nested directory: denied"),
            warning("/src/broken\nlink", "failed to inspect directory entry"),
        ];
        let rendered = render(
            &mut ScanWarningsOverlayState::default(),
            &warnings,
            (100, 20),
        )
        .join("\n");
        assert!(
            rendered.contains("2 directories could not be scanned"),
            "{rendered}"
        );
        assert!(rendered.contains("/src/locked"), "{rendered}");
        assert!(
            rendered.contains("  failed to read nested directory: denied"),
            "{rendered}"
        );
        assert!(rendered.contains("/src/broken\\nlink"), "{rendered}");
        assert!(rendered.contains("↑/↓ scroll / esc close"), "{rendered}");
    }

    #[test]
    fn scrolling_is_clamped_to_the_last_page() {
        let warnings = (0..20)
            .map(|index| warning(&format!("/src/repo-{index:02}"), "denied"))
            .collect::<Vec<_>>();
        let mut overlay = ScanWarningsOverlayState {
            scroll_offset: usize::MAX,
        };
        let rows = render(&mut overlay, &warnings, (60, 12));
        let inner_height = 12 * 9 / 10 - 2;
        let total_lines = warnings.len() * 3 - 1;
        assert_eq!(overlay.scroll_offset, total_lines - inner_height);
        let rendered = rows.join("\n");
        assert!(rendered.contains("/src/repo-19"), "{rendered}");
        assert!(!rendered.contains("/src/repo-00"), "{rendered}");
    }
}
