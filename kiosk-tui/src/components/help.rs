use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph},
};

use crate::theme::Theme;
use kiosk_core::{
    state::HelpOverlayState,
    view::{self, HelpEntry, Tone},
};

fn items(entries: &[HelpEntry], theme: &Theme) -> Vec<ListItem<'static>> {
    if entries.is_empty() {
        return vec![ListItem::new(Span::styled(
            view::NO_MATCHING_BINDINGS,
            theme.style(Tone::Muted).add_modifier(Modifier::ITALIC),
        ))];
    }
    let keys = |keys: &[String]| keys.join(", ");
    // Descriptions line up after the longest keys, and after 14 columns at least.
    let keys_width = entries
        .iter()
        .filter_map(|entry| match entry {
            HelpEntry::Binding { keys: bound, .. } => Some(keys(bound).chars().count()),
            HelpEntry::Section { .. } | HelpEntry::Blank => None,
        })
        .max()
        .unwrap_or_default()
        .max(14);
    entries
        .iter()
        .map(|entry| match entry {
            HelpEntry::Section { name } => ListItem::new(Span::styled(
                format!("{name}:"),
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )),
            HelpEntry::Blank => ListItem::new(""),
            HelpEntry::Binding {
                keys: bound,
                description,
                command,
                ..
            } => ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<keys_width$} ", keys(bound)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(*description),
                Span::styled(format!("  ({command})"), theme.style(Tone::Muted)),
            ])),
        })
        .collect()
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut HelpOverlayState,
    toast_visible: bool,
    theme: &Theme,
) {
    let width = area.width.saturating_mul(4) / 5;
    let height = area.height.saturating_mul(9) / 10;
    let [horizontal] = Layout::horizontal([Constraint::Length(width.max(1))])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height.max(1))])
        .flex(Flex::Center)
        .areas(horizontal);
    frame.render_widget(Clear, popup);

    let (search_area, notice_area, list_area) = if toast_visible {
        let [search, notice, list] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .areas(popup);
        (search, Some(notice), list)
    } else {
        let [search, list] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(popup);
        (search, None, list)
    };
    super::search_bar::draw(
        frame,
        search_area,
        &super::search_bar::SearchBarStyle {
            title: view::HELP_TITLE,
            placeholder: view::HELP_PLACEHOLDER,
            border_color: theme.tertiary,
            muted_color: theme.muted,
        },
        &overlay.list.input.text,
        overlay.list.input.cursor,
    );
    if let Some(notice_area) = notice_area {
        frame.render_widget(
            Paragraph::new(view::HELP_TOAST_NOTICE)
                .style(theme.style(Tone::Muted))
                .alignment(Alignment::Center),
            notice_area,
        );
    }

    let entries = view::help_entries(overlay);
    let viewport_rows = usize::from(list_area.height.saturating_sub(2)).max(1);
    let selected = view::scroll_help(overlay, &entries, viewport_rows);
    let list = List::new(items(&entries, theme))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " {} ",
                    view::help_summary(overlay.list.filtered.len())
                ))
                .border_style(Style::default().fg(theme.tertiary)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.tertiary)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always);
    let mut list_state = ListState::default();
    list_state.select(selected);
    *list_state.offset_mut() = overlay.list.scroll_offset;
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::config::keys::{BindingMode, KeysConfig};

    #[test]
    fn overlay_uses_effective_lowercase_bindings_and_search_metadata() {
        let keys = toml::from_str::<KeysConfig>(
            "[branch_select]\n\"C-b\" = \"new_branch\"\n\"C-o\" = \"noop\"",
        )
        .unwrap();
        let overlay = HelpOverlayState::for_mode(&keys, BindingMode::Branch);
        let row = overlay
            .rows
            .iter()
            .find(|row| row.command_name == "new_branch")
            .unwrap();
        assert_eq!(row.keys, ["ctrl+b"]);
        assert!(row.search_text().contains("Create a new branch"));
        assert!(
            !overlay
                .rows
                .iter()
                .any(|row| row.keys.iter().any(|key| key == "ctrl+o"))
        );
    }

    #[test]
    fn filtered_layout_keeps_section_headers_and_separators() {
        let keys = KeysConfig::default();
        let mut overlay = HelpOverlayState::for_mode(&keys, BindingMode::Branch);
        let branch = overlay
            .rows
            .iter()
            .position(|row| row.section_name == "branches")
            .unwrap();
        let general = overlay
            .rows
            .iter()
            .position(|row| row.section_name == "general")
            .unwrap();
        overlay.list.filtered = vec![(branch, 1), (general, 1)];
        let entries = view::help_entries(&overlay);
        assert!(matches!(
            entries[0],
            HelpEntry::Section { name: "branches" }
        ));
        assert!(matches!(entries[2], HelpEntry::Blank));
        assert!(matches!(entries[3], HelpEntry::Section { name: "general" }));
    }
}
