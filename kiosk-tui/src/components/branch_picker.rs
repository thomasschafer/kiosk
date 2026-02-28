use crate::theme::Theme;
use kiosk_core::agent::AgentState;
use kiosk_core::config::KeysConfig;
use kiosk_core::state::AppState;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

fn agent_label_prefix(state: &AppState) -> Option<AgentLabelCtx<'_>> {
    let has_agents = state
        .branch_list
        .filtered
        .iter()
        .any(|(idx, _)| state.branches[*idx].agent_status.is_some());
    if !has_agents {
        return None;
    }
    let label_width = state.agent_labels.max_label_width();
    // +2 for the surrounding brackets
    let col_width = label_width + 2;
    Some(AgentLabelCtx {
        labels: &state.agent_labels,
        label_width,
        col_width,
    })
}

struct AgentLabelCtx<'a> {
    labels: &'a kiosk_core::config::AgentLabelsConfig,
    label_width: usize,
    col_width: usize,
}

impl AgentLabelCtx<'_> {
    fn blank_padding(&self) -> String {
        " ".repeat(self.col_width)
    }
}

#[allow(clippy::too_many_lines)]
pub fn draw(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme, _keys: &KeysConfig) {
    let repo_name = state
        .selected_repo_idx
        .map_or("??", |i| state.repos[i].name.as_str());
    let selected_repo_path = state.selected_repo_idx.map(|i| state.repos[i].path.clone());

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);

    // Search bar
    let title = format!("{repo_name} — select branch");
    super::search_bar::draw(
        f,
        chunks[0],
        &super::search_bar::SearchBarStyle {
            title: &title,
            placeholder: "Type to search branches (or type new branch name)...",
            border_color: theme.secondary,
            muted_color: theme.muted,
        },
        &state.branch_list.input.text,
        state.branch_list.input.cursor,
    );

    let label_ctx = agent_label_prefix(state);

    // Branch list
    let mut items: Vec<ListItem> = state
        .branch_list
        .filtered
        .iter()
        .map(|(idx, _)| {
            let branch = &state.branches[*idx];
            let mut spans: Vec<Span<'_>> = Vec::new();

            // Agent label prefix column
            if let Some(ref ctx) = label_ctx {
                if let Some(ref agent_status) = branch.agent_status {
                    let (label, color) = match agent_status.state {
                        AgentState::Running => (&ctx.labels.running, theme.accent),
                        AgentState::Waiting => (&ctx.labels.waiting, theme.warning),
                        AgentState::Idle => (&ctx.labels.idle, theme.muted),
                        AgentState::Unknown => (&ctx.labels.unknown, theme.hint),
                    };
                    spans.push(Span::styled(
                        format!("[{label:<width$}]", width = ctx.label_width),
                        Style::default().fg(color),
                    ));
                } else {
                    spans.push(Span::raw(ctx.blank_padding()));
                }
                spans.push(Span::raw(" "));
            }

            if let Some(remote) = &branch.remote {
                spans.push(Span::styled(&branch.name, Style::default().fg(theme.muted)));
                spans.push(Span::styled(
                    format!(" ({remote})"),
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::ITALIC),
                ));
                return ListItem::new(Line::from(spans));
            }

            spans.push(Span::raw(&branch.name));
            let is_deleting = selected_repo_path
                .as_ref()
                .is_some_and(|repo_path| state.is_branch_pending_delete(repo_path, &branch.name));

            if is_deleting {
                spans.push(Span::styled(
                    " (deleting...)",
                    Style::default().fg(theme.accent),
                ));
            } else if branch.has_session {
                spans.push(Span::styled(
                    " (session)",
                    Style::default().fg(theme.success),
                ));
            } else if branch.worktree_path.is_some() {
                spans.push(Span::styled(
                    " (worktree)",
                    Style::default().fg(theme.warning),
                ));
            }

            if branch.is_current {
                spans.push(Span::styled(" *", Style::default().fg(theme.accent)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    // If search doesn't match anything, show "create new branch" option
    if state.loading_branches && state.branch_list.filtered.is_empty() {
        let mut spans: Vec<Span<'_>> = Vec::new();
        if let Some(ref ctx) = label_ctx {
            spans.push(Span::raw(ctx.blank_padding()));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "Loading branches...",
            Style::default().fg(theme.muted),
        ));
        items.push(ListItem::new(Line::from(spans)));
    } else if state.branch_list.filtered.is_empty() && !state.branch_list.input.text.is_empty() {
        let mut spans: Vec<Span<'_>> = Vec::new();
        if let Some(ref ctx) = label_ctx {
            spans.push(Span::raw(ctx.blank_padding()));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "+ Create branch ",
            Style::default().fg(theme.success),
        ));
        spans.push(Span::styled(
            format!("\"{}\"", state.branch_list.input.text),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " (Enter to pick base)",
            Style::default().fg(theme.muted),
        ));
        items.push(ListItem::new(Line::from(spans)));
    }

    let count = state.branch_list.filtered.len();
    let loading_suffix = if state.loading_branches {
        " | loading..."
    } else if state.fetching_remotes {
        " | fetching..."
    } else {
        ""
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {count} branches{loading_suffix} "))
                .border_style(Style::default().fg(theme.border)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.secondary)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = ListState::default();
    list_state.select(state.branch_list.selected);
    *list_state.offset_mut() = state.branch_list.scroll_offset;
    f.render_stateful_widget(list, chunks[1], &mut list_state);
}
