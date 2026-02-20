use crossterm::event::KeyEvent;
use kiosk_core::action::Action;
use kiosk_core::state::AppState;
use ratatui::{Frame, layout::Rect};

pub trait Component {
    fn draw(&self, f: &mut Frame, area: Rect, state: &AppState);
    fn handle_key(&self, key: KeyEvent, state: &AppState) -> Option<Action>;
}
