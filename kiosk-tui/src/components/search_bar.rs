use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use unicode_width::UnicodeWidthStr;

pub struct SearchBarStyle<'a> {
    pub title: &'a str,
    pub placeholder: &'a str,
    pub border_color: Color,
    pub muted_color: Color,
}

/// Render a search bar with visual cursor indicator
pub fn draw(
    f: &mut Frame,
    area: Rect,
    style: &SearchBarStyle<'_>,
    search_text: &str,
    cursor_pos: usize,
) {
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", style.title))
        .border_style(Style::default().fg(style.border_color));
    let inner = search_block.inner(area);

    if search_text.is_empty() {
        let content = Line::from(vec![
            Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
            Span::styled(style.placeholder, Style::default().fg(style.muted_color)),
        ]);
        f.render_widget(Paragraph::new(content).block(search_block), area);
        if inner.width > 0 && inner.height > 0 {
            f.set_cursor(inner.x, inner.y);
        }
    } else {
        // Split text at cursor position and render with a visible cursor
        let before = &search_text[..cursor_pos];
        let after = &search_text[cursor_pos..];

        let mut spans = vec![Span::raw(before.to_string())];

        if let Some(cursor_char) = after.chars().next() {
            // Highlight the character under the cursor
            spans.push(Span::styled(
                cursor_char.to_string(),
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            let rest_start = cursor_pos + cursor_char.len_utf8();
            if rest_start < search_text.len() {
                spans.push(Span::raw(search_text[rest_start..].to_string()));
            }
        } else {
            // Cursor is at the end — show a block cursor
            spans.push(Span::styled(
                " ",
                Style::default().add_modifier(Modifier::REVERSED),
            ));
        }

        let content = Line::from(spans);
        f.render_widget(Paragraph::new(content).block(search_block), area);

        if inner.width > 0 && inner.height > 0 {
            let mut cursor_x = inner.x.saturating_add(before.width() as u16);
            let max_x = inner.x.saturating_add(inner.width.saturating_sub(1));
            if cursor_x > max_x {
                cursor_x = max_x;
            }
            f.set_cursor(cursor_x, inner.y);
        }
    }
}
