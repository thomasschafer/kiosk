use crossterm::event::KeyEvent;
use kiosk_core::action::Action;
use kiosk_core::state::AppState;
use ratatui::{Frame, layout::Rect};

/// Trait for pluggable UI components. Not yet implemented by built-in components
/// (which use free functions), but defines the interface for future plugin components.
pub trait Component {
    fn draw(&self, f: &mut Frame, area: Rect, state: &AppState);
    fn handle_key(&self, key: KeyEvent, state: &AppState) -> Option<Action>;
}
