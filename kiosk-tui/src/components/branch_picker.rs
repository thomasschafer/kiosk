use std::time::Instant;

use ratatui::{Frame, layout::Rect};

use crate::theme::Theme;
use kiosk_core::{config::keys::KeysConfig, state::AppState, view};

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    keys: &KeysConfig,
    theme: &Theme,
    spinner_start: Instant,
) {
    super::repo_list::draw_list(frame, area, state, theme, spinner_start, |state, rows| {
        view::branch_list(state, keys, rows)
    });
}

#[cfg(test)]
mod tests {
    use kiosk_core::product::TEST_PRODUCT;
    use std::time::Instant;

    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    use crate::theme::Theme;
    use kiosk_core::state::{AppState, BranchContext, BranchEntry, Mode, SearchableList};

    use super::draw;
    use kiosk_core::config::keys::KeysConfig;

    #[test]
    fn remote_row_uses_qualified_name_without_redundant_suffix() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.branch_view.entries = BranchEntry::build_remote("origin", &["feature".into()], &[]);
        state.branch_view.list = SearchableList::new(1);
        state.branch_view.list.selected = None;
        let theme = Theme::from_config(&kiosk_core::config::ThemeConfig::default());
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw(
                    frame,
                    area,
                    &mut state,
                    &KeysConfig::default(),
                    &theme,
                    Instant::now(),
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("origin/feature"));
        assert!(!rendered.contains("(remote)"));
        let label = "origin/feature"
            .chars()
            .map(|character| character.to_string())
            .collect::<Vec<_>>();
        let cells = buffer
            .content()
            .windows(label.len())
            .find(|cells| {
                cells
                    .iter()
                    .zip(&label)
                    .all(|(cell, symbol)| cell.symbol() == symbol)
            })
            .expect("remote label cells");
        assert!(cells.iter().all(|cell| cell.fg == theme.muted));
        assert!(
            cells
                .iter()
                .all(|cell| cell.modifier.contains(Modifier::DIM))
        );
    }
}
