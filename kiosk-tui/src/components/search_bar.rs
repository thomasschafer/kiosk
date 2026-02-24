use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct SearchBarStyle<'a> {
    pub title: &'a str,
    pub placeholder: &'a str,
    pub border_color: Color,
    pub muted_color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisibleSlice {
    start: usize,
    end: usize,
    cursor_col: u16,
}

fn visible_slice(text: &str, cursor_pos: usize, max_width: u16) -> VisibleSlice {
    if max_width == 0 || text.is_empty() {
        return VisibleSlice {
            start: 0,
            end: 0,
            cursor_col: 0,
        };
    }

    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let mut boundaries: Vec<usize> = graphemes.iter().map(|(i, _)| *i).collect();
    boundaries.push(text.len());

    let cursor = cursor_pos.min(text.len());
    let boundary_idx = match boundaries.binary_search(&cursor) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    };

    let mut prefix_widths = Vec::with_capacity(boundaries.len());
    let mut width = 0;
    prefix_widths.push(0);
    for (_, grapheme) in &graphemes {
        width += grapheme.width();
        prefix_widths.push(width);
    }

    let cursor_col = prefix_widths[boundary_idx];
    let max_width = max_width as usize;
    let max_cursor_col = max_width.saturating_sub(1);
    let scroll_col = cursor_col.saturating_sub(max_cursor_col);

    let mut start_index = 0;
    for (idx, &col) in prefix_widths.iter().enumerate() {
        if col > scroll_col {
            break;
        }
        start_index = idx;
    }
    if start_index >= graphemes.len() {
        start_index = graphemes.len().saturating_sub(1);
    }

    let start_byte = boundaries[start_index];
    let mut end_index = start_index;
    let mut visible_width = 0;
    while end_index < graphemes.len() {
        let g_width = graphemes[end_index].1.width();
        if visible_width + g_width > max_width {
            break;
        }
        visible_width += g_width;
        end_index += 1;
    }
    let end_byte = boundaries[end_index];

    let cursor_col = cursor_col
        .saturating_sub(prefix_widths[start_index])
        .min(max_cursor_col);
    let cursor_col = u16::try_from(cursor_col).unwrap_or(u16::MAX);

    VisibleSlice {
        start: start_byte,
        end: end_byte,
        cursor_col,
    }
}

/// Render a search bar with a terminal cursor indicator
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
        let content = Line::from(vec![Span::styled(
            style.placeholder,
            Style::default().fg(style.muted_color),
        )]);
        f.render_widget(Paragraph::new(content).block(search_block), area);
        if inner.width > 0 && inner.height > 0 {
            f.set_cursor_position((inner.x, inner.y));
        }
    } else {
        let slice = visible_slice(search_text, cursor_pos, inner.width);
        let content = Line::from(Span::raw(&search_text[slice.start..slice.end]));
        f.render_widget(Paragraph::new(content).block(search_block), area);

        if inner.width > 0 && inner.height > 0 {
            let cursor_x = inner.x.saturating_add(slice.cursor_col);
            f.set_cursor_position((cursor_x, inner.y));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::visible_slice;

    #[test]
    fn test_visible_slice_no_scroll() {
        let result = visible_slice("hello", 2, 10);
        assert_eq!(result.start, 0);
        assert_eq!(result.end, 5);
        assert_eq!(result.cursor_col, 2);
    }

    #[test]
    fn test_visible_slice_scrolls_to_cursor() {
        let result = visible_slice("hello world", 11, 5);
        assert_eq!(&"hello world"[result.start..result.end], "orld");
        assert_eq!(result.cursor_col, 4);
    }

    #[test]
    fn test_visible_slice_handles_emoji_width() {
        let text = "A👩‍💻B";
        let cursor_pos = "A👩‍💻".len();
        let result = visible_slice(text, cursor_pos, 3);
        assert_eq!(&text[result.start..result.end], "👩‍💻B");
        assert_eq!(result.cursor_col, 2);
    }

    #[test]
    fn test_visible_slice_combining_mark() {
        let text = "e\u{0301}x";
        let cursor_pos = "e\u{0301}".len();
        let result = visible_slice(text, cursor_pos, 2);
        assert_eq!(&text[result.start..result.end], text);
        assert_eq!(result.cursor_col, 1);
    }
}
