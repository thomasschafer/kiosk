use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

/// Render a search bar with visual cursor indicator
pub fn draw(
    f: &mut Frame,
    area: Rect,
    title: &str,
    search_text: &str,
    cursor_pos: usize,
    placeholder: &str,
    border_color: ratatui::style::Color,
) {
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(border_color));

    if search_text.is_empty() {
        let content = Line::from(Span::styled(
            placeholder,
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(Paragraph::new(content).block(search_block), area);
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
    }
}
