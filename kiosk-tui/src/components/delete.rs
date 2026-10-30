use std::time::Instant;

use kiosk_core::{config::keys::KeysConfig, state::DeleteFlowState};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{components, theme::Theme};

/// The confirmation for removing a checkout. `backend_name` names where an open checkout
/// is also closed.
pub(crate) fn draw_dialog(
    frame: &mut Frame,
    area: Rect,
    flow: &DeleteFlowState,
    backend_name: &str,
    theme: &Theme,
    keys: &KeysConfig,
    spinner_start: Instant,
) {
    let target = flow.target();
    let path = kiosk_core::display::sanitize(&kiosk_core::path::display(target.worktree_path()))
        .into_owned();
    let mut lines = if target.force() {
        vec![
            Line::from(Span::styled(
                "This checkout has uncommitted changes.",
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::raw(format!("Force-remove {path}?")),
        ]
    } else {
        vec![Line::raw(format!("Remove checkout {path}?"))]
    };
    if target.is_open() {
        lines.push(Line::raw(format!(
            "It will also be closed in {backend_name}."
        )));
    }
    lines.push(Line::raw(""));
    if target.in_progress() {
        let spinner =
            components::repo_list::SPINNER_FOR_LOADING[(spinner_start.elapsed().as_millis() / 80)
                as usize
                % components::repo_list::SPINNER_FOR_LOADING.len()];
        lines.push(Line::from(Span::styled(
            format!("{spinner} Removing checkout…"),
            Style::default().fg(theme.secondary),
        )));
    } else if let Some(hints) =
        components::dialog::hint_line(&components::dialog::confirm_cancel_hints(keys), theme)
    {
        lines.push(hints);
    }
    components::dialog::Dialog::new(" Confirm delete ", lines, theme.secondary).render(frame, area);
}
