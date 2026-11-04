use std::time::Instant;

use kiosk_core::{
    config::keys::KeysConfig,
    state::DeleteFlowState,
    view::{self, Dialog},
};
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
    let Dialog::ConfirmDelete {
        title,
        warning,
        lines: message,
        busy,
        hints,
        ..
    } = view::delete_dialog(flow, backend_name, keys)
    else {
        unreachable!("the delete flow is a delete dialog");
    };
    let mut lines = warning
        .map(|warning| {
            Line::from(Span::styled(
                warning,
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ))
        })
        .into_iter()
        .chain(message.into_iter().map(Line::raw))
        .collect::<Vec<_>>();
    lines.push(Line::raw(""));
    if let Some(busy) = busy {
        let spinner = components::repo_list::spinner(spinner_start);
        lines.push(Line::from(Span::styled(
            format!("{spinner} {busy}"),
            Style::default().fg(theme.secondary),
        )));
    } else if let Some(hints) = components::dialog::dialog_hint_line(&hints, theme) {
        lines.push(hints);
    }
    let title = format!(" {title} ");
    components::dialog::Dialog::new(&title, lines, theme.secondary).render(frame, area);
}
