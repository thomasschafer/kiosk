use ratatui::layout::{Constraint, Layout, Rect};

pub mod branch_picker;
pub mod dialog;
pub mod error_toast;
pub mod help;
pub mod new_branch;
pub mod path_input;
pub mod repo_list;
pub mod search_bar;
pub mod setup;

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

/// Standard dialog width: 80% of terminal width, capped at 80 columns.
pub fn dialog_width(terminal_width: u16) -> u16 {
    (u32::from(terminal_width) * 80 / 100).min(80) as u16
}

/// Center a rect with a fixed width and height, clamped to fit within `r`.
pub fn centered_fixed_rect(width: u16, height: u16, r: Rect) -> Rect {
    let clamped_width = width.min(r.width);
    let clamped_height = height.min(r.height);
    let offset_x = r.x + (r.width.saturating_sub(clamped_width)) / 2;
    let offset_y = r.y + (r.height.saturating_sub(clamped_height)) / 2;
    Rect::new(offset_x, offset_y, clamped_width, clamped_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dialog_width_normal_terminal() {
        assert_eq!(dialog_width(100), 80);
        assert_eq!(dialog_width(80), 64);
        assert_eq!(dialog_width(50), 40);
    }

    #[test]
    fn test_dialog_width_capped_at_80() {
        assert_eq!(dialog_width(200), 80);
    }

    #[test]
    fn test_dialog_width_wide_terminal_no_overflow() {
        // u16::MAX = 65535; previous implementation would overflow at width > 819
        assert_eq!(dialog_width(820), 80);
        assert_eq!(dialog_width(1000), 80);
        assert_eq!(dialog_width(u16::MAX), 80);
    }

    #[test]
    fn test_dialog_width_zero() {
        assert_eq!(dialog_width(0), 0);
    }
}
