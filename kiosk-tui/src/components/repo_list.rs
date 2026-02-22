use crate::theme::Theme;
use kiosk_core::config::{Command, KeysConfig};
use kiosk_core::state::{AppState, Mode};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme, keys: &KeysConfig) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    super::search_bar::draw(
        f,
        chunks[0],
        "kiosk — select repo",
        &state.repo_list.search,
        state.repo_list.cursor,
        "Type to search repos...",
        theme.accent,
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
                Style::default().fg(Color::DarkGray),
            ));
            if wt_count > 1 {
                spans.push(Span::styled(
                    format!(" +{} worktrees", wt_count - 1),
                    Style::default().fg(Color::Yellow),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    if state.loading_repos && state.repo_list.filtered.is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "Discovering repos...",
            Style::default().fg(Color::DarkGray),
        )])));
    }

    let hints = build_repo_hints(keys);
    let loading_suffix = if state.loading_repos {
        " | loading..."
    } else {
        ""
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " {} repos ({hints}{loading_suffix}) ",
                    state.repo_list.filtered.len()
                ))
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(state.repo_list.selected);
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}

fn build_repo_hints(keys: &KeysConfig) -> String {
    let keymap = keys.keymap_for_mode(&Mode::RepoSelect);
    let mut hints = Vec::new();

    if let Some(key) = KeysConfig::find_key(&keymap, &Command::OpenRepo) {
        hints.push(format!("{key}: open"));
    }
    if let Some(key) = KeysConfig::find_key(&keymap, &Command::EnterRepo) {
        hints.push(format!("{key}: branches"));
    }

    hints.join(", ")
}
