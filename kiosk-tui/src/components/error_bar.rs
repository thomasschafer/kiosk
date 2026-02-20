use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::Paragraph,
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState) {
    if let Some(error) = &state.error {
        let error_line = Paragraph::new(Span::styled(
            format!(" Error: {error}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
        f.render_widget(error_line, area);
    }
}
