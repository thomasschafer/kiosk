use ratatui::layout::{Constraint, Layout, Rect};

pub mod branch_picker;
pub mod error_bar;
pub mod help;
pub mod layout;
pub mod new_branch;
pub mod repo_list;
pub mod search_bar;

/// Helper function to center a rect within another rect
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Percentage(percent_y.min(100)),
        Constraint::Fill(1),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Percentage(percent_x.min(100)),
        Constraint::Fill(1),
    ])
    .split(popup_layout[1])[1]
}
