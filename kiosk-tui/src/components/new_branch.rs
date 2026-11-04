use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
};

use crate::theme::Theme;
use kiosk_core::{
    config::keys::KeysConfig,
    state::{AppState, Mode, RemoteBranchPrompt},
    view::{self, Dialog},
};

pub fn draw_remote_prompt(
    frame: &mut Frame,
    area: Rect,
    repo: &std::path::Path,
    prompt: &RemoteBranchPrompt,
    theme: &Theme,
    keys: &KeysConfig,
) {
    let Dialog::RemoteBranch {
        title,
        lines: message,
        choices,
        selected,
        hints,
    } = view::remote_branch_dialog(repo, prompt, keys)
    else {
        unreachable!("the remote branch prompt is a remote branch dialog");
    };
    let mut lines = message.into_iter().map(Line::raw).collect::<Vec<_>>();
    lines.push(Line::raw(""));
    for (index, choice) in choices.into_iter().enumerate() {
        let label = choice.label;
        lines.push(if index == selected {
            Line::from(Span::styled(
                format!("▸ {label}"),
                Style::default()
                    .bg(theme.secondary)
                    .fg(theme.highlight_fg)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::raw(format!("  {label}"))
        });
    }
    if let Some(hints) = super::dialog::dialog_hint_line(&hints, theme) {
        lines.push(Line::raw(""));
        lines.push(hints);
    }
    let title = format!(" {title} ");
    super::dialog::Dialog::new(&title, lines, theme.secondary).render(frame, area);
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    spinner_start: Instant,
) {
    let [horizontal] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(horizontal);

    let title = match &state.mode {
        Mode::ValidatingNewBranch { name, .. } => view::new_branch_title(name),
        Mode::SelectBaseBranch { flow, .. } => view::base_branch_title(&flow.new_name),
        _ => return,
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(theme.secondary))
        .padding(Padding::uniform(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    match &mut state.mode {
        Mode::ValidatingNewBranch { .. } => {
            let spinner = super::repo_list::spinner(spinner_start);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(""),
                    Line::from(Span::styled(
                        format!("{spinner} {}", view::VALIDATING_BRANCH),
                        Style::default()
                            .fg(theme.secondary)
                            .add_modifier(Modifier::BOLD),
                    )),
                ])
                .alignment(Alignment::Center),
                inner,
            );
        }
        Mode::SelectBaseBranch { context, flow } => {
            let [search_area, list_area] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
            let list = view::base_list(
                &context.repo_path,
                flow,
                usize::from(list_area.height.saturating_sub(2)).max(1),
            );
            super::repo_list::draw_list_view(
                frame,
                [search_area, list_area],
                &list,
                theme.secondary,
                theme,
                spinner_start,
            );
        }
        Mode::RepoSelect
        | Mode::BranchSelect(_)
        | Mode::ConfirmRemoteBranch { .. }
        | Mode::ConfirmWorktreeDelete(_)
        | Mode::Loading { .. } => {}
    }
}
