use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

use crate::theme::Theme;
use kiosk_core::view::{Hint, Tone};

/// Renders hints as one line, `key label` pairs split by `separator`. Returns `None` when
/// there are no hints.
pub(crate) fn hint_line<'a>(hints: &[Hint], separator: &'a str, theme: &Theme) -> Option<Line<'a>> {
    let mut spans = Vec::new();
    for hint in hints {
        if !spans.is_empty() {
            spans.push(Span::raw(separator));
        }
        spans.push(Span::styled(hint.key.clone(), theme.style(Tone::Hint)));
        spans.push(Span::raw(format!(" {}", hint.label)));
    }
    (!spans.is_empty()).then(|| Line::from(spans))
}

/// [`hint_line`] with the separator dialogs use.
pub(crate) fn dialog_hint_line<'a>(hints: &[Hint], theme: &Theme) -> Option<Line<'a>> {
    hint_line(hints, " / ", theme)
}

pub struct Dialog<'a> {
    lines: Vec<Line<'a>>,
    border_color: Color,
    title: &'a str,
}

impl<'a> Dialog<'a> {
    pub fn new(title: &'a str, lines: Vec<Line<'a>>, border_color: Color) -> Self {
        Self {
            lines,
            border_color,
            title,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(4).clamp(20, 100);
        let text_width = width.saturating_sub(6).max(1);
        let content_height = super::wrapped_content_height(&self.lines, text_width);
        let height = content_height.saturating_add(4).min(area.height.max(1));
        let [horizontal] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        let [dialog_area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(horizontal);
        frame.render_widget(Clear, dialog_area);
        frame.render_widget(
            Paragraph::new(self.lines.clone())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(self.border_color))
                        .title(self.title)
                        .padding(Padding::uniform(1)),
                )
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            dialog_area,
        );
    }
}
