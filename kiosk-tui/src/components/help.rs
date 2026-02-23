use kiosk_core::config::KeysConfig;
use kiosk_core::state::{AppState, HelpOverlayState};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};

enum HelpLayoutEntry {
    SectionHeader(&'static str),
    Blank,
    Row(usize),
}

fn compute_help_layout(overlay: &HelpOverlayState) -> Vec<HelpLayoutEntry> {
    let mut entries = Vec::new();
    let mut current_section: Option<&'static str> = None;

    for (row_idx, _) in overlay.list.filtered.iter().copied() {
        let Some(row) = overlay.rows.get(row_idx) else {
            continue;
        };

        if current_section != Some(row.section_name) {
            if current_section.is_some() {
                entries.push(HelpLayoutEntry::Blank);
            }
            current_section = Some(row.section_name);
            entries.push(HelpLayoutEntry::SectionHeader(row.section_name));
        }

        entries.push(HelpLayoutEntry::Row(row_idx));
    }

    entries
}

/// Help overlay showing keybindings.
pub fn draw(f: &mut Frame, state: &AppState, theme: &crate::theme::Theme, _keys: &KeysConfig) {
    let Some(overlay) = state.help_overlay.as_ref() else {
        return;
    };

    let popup_area = super::centered_rect(80, 85, f.area());
    f.render_widget(Clear, popup_area);

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(popup_area);
    super::search_bar::draw(
        f,
        chunks[0],
        &super::search_bar::SearchBarStyle {
            title: "help - key bindings",
            placeholder: "Type to filter by key, command, or description...",
            border_color: theme.accent,
            muted_color: theme.muted,
        },
        &overlay.list.search,
        overlay.list.cursor,
    );

    let (items, row_item_indices) = build_visible_items(overlay, theme.muted);
    let selected_item = overlay
        .list
        .selected
        .and_then(|selected| row_item_indices.get(selected))
        .copied();
    // For the help overlay, scroll_offset is already in visual-row space
    // (computed by update_help_scroll_offset), unlike other lists where it's
    // a logical item index. This is because the help list has non-selectable
    // visual rows (section headers, blank separators) that shift the mapping.
    let list_offset = overlay.list.scroll_offset;
    let title = format!(" {} bindings (esc: close) ", overlay.list.filtered.len());
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(theme.accent)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.secondary)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(selected_item);
    *list_state.offset_mut() = list_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}

fn build_visible_items(
    overlay: &HelpOverlayState,
    muted_color: Color,
) -> (Vec<ListItem<'static>>, Vec<usize>) {
    if overlay.list.filtered.is_empty() {
        let item = ListItem::new(Line::from(Span::styled(
            "No matching bindings",
            Style::default()
                .fg(muted_color)
                .add_modifier(Modifier::ITALIC),
        )));
        return (vec![item], Vec::new());
    }

    let layout = compute_help_layout(overlay);
    let mut items = Vec::with_capacity(layout.len());
    let mut row_item_indices = Vec::new();

    for entry in &layout {
        match entry {
            HelpLayoutEntry::Blank => {
                items.push(ListItem::new(Line::from("")));
            }
            HelpLayoutEntry::SectionHeader(name) => {
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("{}:", name.replace('_', " ")),
                    Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ))));
            }
            HelpLayoutEntry::Row(row_idx) => {
                let row = &overlay.rows[*row_idx];
                row_item_indices.push(items.len());
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<13}", row.key_display),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" {}", row.description)),
                    Span::styled(
                        format!("  ({})", row.command),
                        Style::default().fg(muted_color),
                    ),
                ])));
            }
        }
    }

    (items, row_item_indices)
}

pub(crate) fn help_visual_metrics(overlay: &HelpOverlayState) -> (Vec<usize>, usize) {
    let layout = compute_help_layout(overlay);
    let row_item_indices = layout
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| matches!(entry, HelpLayoutEntry::Row(_)).then_some(i))
        .collect();
    (row_item_indices, layout.len())
}

#[cfg(test)]
mod tests {
    use super::{build_visible_items, help_visual_metrics};
    use kiosk_core::{
        config::Command,
        config::keys::FlattenedKeybindingRow,
        state::{HelpOverlayState, SearchableList},
    };
    use ratatui::style::Color;

    #[test]
    fn test_build_visible_items_inserts_blank_line_between_sections() {
        let rows = vec![
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-h".to_string(),
                command: Command::ShowHelp,
                description: Command::ShowHelp.labels().description,
            },
            FlattenedKeybindingRow {
                section_index: 1,
                section_name: "text_edit",
                key_display: "backspace".to_string(),
                command: Command::DeleteBackwardChar,
                description: Command::DeleteBackwardChar.labels().description,
            },
        ];
        let mut list = SearchableList::new(rows.len());
        list.selected = Some(0);
        let overlay = HelpOverlayState { list, rows };

        let (items, _row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        assert!(
            items.len() >= 5,
            "Expected section header, row, blank, header, row"
        );
    }

    #[test]
    fn test_build_visible_items_uses_row_offset_mapping_for_scroll() {
        let rows = vec![
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-c".to_string(),
                command: Command::Quit,
                description: Command::Quit.labels().description,
            },
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-h".to_string(),
                command: Command::ShowHelp,
                description: Command::ShowHelp.labels().description,
            },
            FlattenedKeybindingRow {
                section_index: 1,
                section_name: "text_edit",
                key_display: "backspace".to_string(),
                command: Command::DeleteBackwardChar,
                description: Command::DeleteBackwardChar.labels().description,
            },
        ];
        let mut list = SearchableList::new(rows.len());
        list.selected = Some(2);
        list.scroll_offset = 2;
        let overlay = HelpOverlayState { list, rows };

        let (_items, row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        let selected_item = overlay
            .list
            .selected
            .and_then(|selected| row_item_indices.get(selected))
            .copied()
            .expect("Expected selected item");
        assert!(selected_item > 0);
    }

    #[test]
    fn test_help_scroll_near_bottom_up_then_down_does_not_jump_viewport() {
        let rows: Vec<FlattenedKeybindingRow> = (0..40)
            .map(|i| FlattenedKeybindingRow {
                section_index: i / 10,
                section_name: match i / 10 {
                    0 => "general",
                    1 => "text_edit",
                    2 => "list_navigation",
                    _ => "repo_select",
                },
                key_display: format!("K-{i:02}"),
                command: Command::MoveDown,
                description: Command::MoveDown.labels().description,
            })
            .collect();

        let mut list = SearchableList::new(rows.len());
        let viewport_rows = 10;
        for _ in 0..200 {
            list.move_selection(1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }

        let mut overlay = HelpOverlayState { list, rows };
        let (_items, row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        let selected_before = overlay
            .list
            .selected
            .and_then(|selected| row_item_indices.get(selected))
            .copied();
        let offset_before = Some(overlay.list.scroll_offset);

        overlay.list.move_selection(-1);
        overlay
            .list
            .update_scroll_offset_for_selection(viewport_rows);
        let (_items, row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        let selected_after_up = overlay
            .list
            .selected
            .and_then(|selected| row_item_indices.get(selected))
            .copied();
        let offset_after_up = Some(overlay.list.scroll_offset);

        overlay.list.move_selection(1);
        overlay
            .list
            .update_scroll_offset_for_selection(viewport_rows);
        let (_items, row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        let selected_after_down = overlay
            .list
            .selected
            .and_then(|selected| row_item_indices.get(selected))
            .copied();
        let offset_after_down = Some(overlay.list.scroll_offset);

        assert_eq!(offset_before, offset_after_up);
        assert_eq!(offset_after_up, offset_after_down);
        assert_eq!(
            selected_before.expect("selected before"),
            selected_after_up.expect("selected after up") + 1
        );
        assert_eq!(
            selected_before.expect("selected before"),
            selected_after_down.expect("selected after down")
        );
    }

    #[test]
    fn test_help_mapping_keeps_selection_off_visual_bottom_edge_before_true_end() {
        let rows: Vec<FlattenedKeybindingRow> = (0..30)
            .map(|i| FlattenedKeybindingRow {
                section_index: i / 10,
                section_name: match i / 10 {
                    0 => "general",
                    1 => "text_edit",
                    _ => "list_navigation",
                },
                key_display: format!("K-{i:02}"),
                command: Command::MoveDown,
                description: Command::MoveDown.labels().description,
            })
            .collect();

        let mut list = SearchableList::new(rows.len());
        let viewport_rows = 10;
        for _ in 0..20 {
            list.move_selection(1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }

        let overlay = HelpOverlayState { list, rows };
        let (items, row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        let selected_visual = overlay
            .list
            .selected
            .and_then(|selected| row_item_indices.get(selected))
            .copied()
            .expect("selected visual");
        let offset_visual = overlay.list.scroll_offset;
        let visual_row_in_view = selected_visual.saturating_sub(offset_visual);
        assert!(
            visual_row_in_view < items.len().saturating_sub(1),
            "Selection should not anchor to visual bottom too early in help list"
        );
    }

    #[test]
    fn test_build_visible_items_empty_filter_shows_no_matching_message() {
        let rows = vec![FlattenedKeybindingRow {
            section_index: 0,
            section_name: "general",
            key_display: "C-c".to_string(),
            command: Command::Quit,
            description: Command::Quit.labels().description,
        }];
        let mut list = SearchableList::new(rows.len());
        list.filtered = vec![];
        list.selected = None;
        let overlay = HelpOverlayState { list, rows };

        let (items, row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        assert_eq!(items.len(), 1, "Should have exactly one 'no matches' item");
        assert!(
            row_item_indices.is_empty(),
            "No selectable rows when filter is empty"
        );
    }

    #[test]
    fn test_build_visible_items_single_section_no_blank_separator() {
        let rows = vec![
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-c".to_string(),
                command: Command::Quit,
                description: Command::Quit.labels().description,
            },
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-h".to_string(),
                command: Command::ShowHelp,
                description: Command::ShowHelp.labels().description,
            },
        ];
        let mut list = SearchableList::new(rows.len());
        list.selected = Some(0);
        let overlay = HelpOverlayState { list, rows };

        let (items, row_item_indices) = build_visible_items(&overlay, Color::DarkGray);
        // Should be: section header + 2 rows = 3 items, no blank separators
        assert_eq!(items.len(), 3, "Single section: header + 2 rows");
        assert_eq!(row_item_indices, vec![1, 2]);
    }

    #[test]
    fn test_help_visual_metrics_single_section() {
        let rows = vec![
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-c".to_string(),
                command: Command::Quit,
                description: Command::Quit.labels().description,
            },
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-h".to_string(),
                command: Command::ShowHelp,
                description: Command::ShowHelp.labels().description,
            },
        ];
        let overlay = HelpOverlayState {
            list: SearchableList::new(rows.len()),
            rows,
        };
        let (indices, total_visual_rows) = help_visual_metrics(&overlay);
        assert_eq!(indices, vec![1, 2]);
        assert_eq!(total_visual_rows, 3);
    }

    #[test]
    fn test_help_visual_metrics_empty_filter() {
        let rows = vec![FlattenedKeybindingRow {
            section_index: 0,
            section_name: "general",
            key_display: "C-c".to_string(),
            command: Command::Quit,
            description: Command::Quit.labels().description,
        }];
        let mut list = SearchableList::new(rows.len());
        list.filtered = vec![];
        list.selected = None;
        let overlay = HelpOverlayState { list, rows };
        let (indices, total_visual_rows) = help_visual_metrics(&overlay);
        assert!(indices.is_empty());
        assert_eq!(total_visual_rows, 0);
    }

    #[test]
    fn test_help_visual_metrics_counts_section_headers_and_spacers() {
        let rows = vec![
            FlattenedKeybindingRow {
                section_index: 0,
                section_name: "general",
                key_display: "C-c".to_string(),
                command: Command::Quit,
                description: Command::Quit.labels().description,
            },
            FlattenedKeybindingRow {
                section_index: 1,
                section_name: "text_edit",
                key_display: "backspace".to_string(),
                command: Command::DeleteBackwardChar,
                description: Command::DeleteBackwardChar.labels().description,
            },
        ];
        let overlay = HelpOverlayState {
            list: SearchableList::new(rows.len()),
            rows,
        };
        let (indices, total_visual_rows) = help_visual_metrics(&overlay);
        assert_eq!(indices, vec![1, 4]);
        assert_eq!(total_visual_rows, 5);
    }
}
