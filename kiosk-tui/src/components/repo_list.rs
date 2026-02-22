use crate::theme::Theme;
use kiosk_core::config::KeysConfig;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme, _keys: &KeysConfig) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    super::search_bar::draw(
        f,
        chunks[0],
        &super::search_bar::SearchBarStyle {
            title: "kiosk — select repo",
            placeholder: "Type to search repos...",
            border_color: theme.accent,
            muted_color: theme.muted,
        },
        &state.repo_list.search,
        state.repo_list.cursor,
    );

    // Repo list
    let mut items: Vec<ListItem> = state
        .repo_list
        .filtered
        .iter()
        .map(|(idx, _)| {
            let repo = &state.repos[*idx];
            let wt_count = repo.worktrees.len();
            let branch = repo
                .worktrees
                .first()
                .and_then(|wt| wt.branch.as_deref())
                .unwrap_or("??");

            let mut spans = vec![Span::raw(&repo.name)];
            spans.push(Span::styled(
                format!(" [{branch}]"),
                Style::default().fg(theme.muted),
            ));
            if wt_count > 1 {
                spans.push(Span::styled(
                    format!(" +{} worktrees", wt_count - 1),
                    Style::default().fg(theme.warning),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    if state.loading_repos && state.repo_list.filtered.is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "Discovering repos...",
            Style::default().fg(theme.muted),
        )])));
    }

    let count = state.repo_list.filtered.len();
    let loading_suffix = if state.loading_repos {
        " | loading..."
    } else {
        ""
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {count} repos{loading_suffix} "))
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.accent)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(state.repo_list.selected);
    *list_state.offset_mut() = state.repo_list.scroll_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
