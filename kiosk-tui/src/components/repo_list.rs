use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState},
};

use crate::theme::Theme;
use kiosk_core::{
    state::AppState,
    view::{self, ListView, Tone},
};

use super::list_row;

pub const SPINNER_FOR_LOADING: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The spinner frame to show `spinner_start.elapsed()` after the picker started.
pub(crate) fn spinner(spinner_start: Instant) -> char {
    SPINNER_FOR_LOADING
        [(spinner_start.elapsed().as_millis() / 80) as usize % SPINNER_FOR_LOADING.len()]
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    spinner_start: Instant,
) {
    draw_list(frame, area, state, theme, spinner_start, |state, rows| {
        view::repo_list(state, rows)
    });
}

/// Draws a searchable list: its search bar over its rows, in the list's colour.
pub(crate) fn draw_list(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    spinner_start: Instant,
    list: impl FnOnce(&mut AppState, usize) -> ListView,
) {
    let [search_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    state.active_list_rows = usize::from(list_area.height.saturating_sub(2)).max(1);
    let list = list(state, state.active_list_rows);
    draw_list_view(
        frame,
        [search_area, list_area],
        &list,
        theme.border,
        theme,
        spinner_start,
    );
}

/// Draws `list`'s search bar and its rows, in a frame of `border`, in those two areas.
pub(crate) fn draw_list_view(
    frame: &mut Frame,
    [search_area, list_area]: [Rect; 2],
    list: &ListView,
    border: Color,
    theme: &Theme,
    spinner_start: Instant,
) {
    let color = theme
        .color(list.kind.tone())
        .expect("list tones have colours");
    super::search_bar::draw(
        frame,
        search_area,
        &super::search_bar::SearchBarStyle {
            title: &list.title,
            placeholder: list.placeholder,
            border_color: color,
            muted_color: theme.muted,
        },
        &format!("{}{}", list.query.before, list.query.after),
        list.query.before.len(),
    );
    let row_width = usize::from(list_area.width.saturating_sub(4));
    let mut items: Vec<_> = list
        .rows
        .iter()
        .map(|row| ListItem::new(Line::from(list_row::row(row, theme, row_width))))
        .collect();
    if let Some(empty) = &list.empty {
        items.push(ListItem::new(Span::styled(
            empty.clone(),
            theme.style(Tone::Muted),
        )));
    }
    let activity = list.activity.map_or_else(String::new, |activity| {
        format!(" | {activity}… {}", spinner(spinner_start))
    });
    let widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {}{activity} ", list.summary))
                .border_style(Style::default().fg(border)),
        )
        .highlight_style(
            Style::default()
                .bg(color)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always);
    let mut list_state = ListState::default();
    list_state.select(list.selected);
    frame.render_stateful_widget(widget, list_area, &mut list_state);
}

#[cfg(test)]
mod tests {
    use kiosk_core::product::TEST_PRODUCT;
    use std::time::Instant;

    use ratatui::{Terminal, backend::TestBackend};

    use crate::theme::Theme;
    use kiosk_core::{
        git::Repo,
        state::{AppState, RepoEntry, SearchableList},
    };

    use super::draw;

    fn render(state: &mut AppState, width: u16) -> String {
        let theme = Theme::from_config(&kiosk_core::config::ThemeConfig::default());
        let mut terminal = Terminal::new(TestBackend::new(width, 8)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                draw(frame, area, state, &theme, Instant::now());
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn repo_controls_render_as_visible_text_on_one_row() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.repo_view.loading = false;
        state.repo_view.entries = vec![RepoEntry::new(Repo {
            name: "repo\nname\u{1b}".into(),
            path: "/repo".into(),
            is_git: true,
            worktrees: Vec::new(),
        })];
        state.repo_view.list = SearchableList::new(1);
        state.repo_view.list.selected = None;

        let rendered = render(&mut state, 80);
        assert!(rendered.contains("repo\\nname\\u{1b}"));
        assert!(!rendered.chars().any(char::is_control));
    }

    #[test]
    fn plain_folders_render_with_a_muted_type_marker() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.repo_view.loading = false;
        state.repo_view.entries = vec![RepoEntry::new(Repo {
            name: "folder".into(),
            path: "/folder".into(),
            is_git: false,
            worktrees: Vec::new(),
        })];
        state.repo_view.list = SearchableList::new(1);
        state.repo_view.list.selected = None;

        let rendered = render(&mut state, 40);
        assert!(rendered.contains("folder  dir"));
    }

    #[test]
    fn empty_results_explain_skipped_optional_directories() {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        assert!(render(&mut state, 80).contains("Discovering repos…"));

        state.repo_view.loading = false;
        assert!(
            render(&mut state, 80).contains("No repos found in the configured search directories")
        );

        state.repo_view.missing_optional_dirs = 1;
        assert!(
            render(&mut state, 80)
                .contains("No repos found (1 optional search directory not found)")
        );

        state.repo_view.missing_optional_dirs = 2;
        assert!(
            render(&mut state, 80)
                .contains("No repos found (2 optional search directories not found)")
        );
    }
}
