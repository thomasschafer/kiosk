use crate::{
    components::{centered_rect, path_input, search_bar},
    theme::Theme,
};
use kiosk_core::state::{AppState, Mode, SetupStep};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap},
};

fn draw_welcome(f: &mut Frame, theme: &Theme) {
    let area = centered_rect(60, 50, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Welcome ")
        .border_style(Style::default().fg(theme.accent))
        .padding(Padding::uniform(1));

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Welcome to Kiosk!",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Kiosk is a worktree-aware tmux session manager that helps"),
        Line::from("you manage git repositories and their branches."),
        Line::from(""),
        Line::from("Let's set up your configuration file."),
        Line::from(""),
        Line::from(vec![
            Span::raw("Press "),
            Span::styled(
                "Enter",
                Style::default().fg(theme.hint).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to continue."),
        ]),
    ];

    let paragraph = Paragraph::new(content)
        .block(block)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

#[allow(clippy::too_many_lines)]
fn draw_search_dirs(f: &mut Frame, state: &AppState, theme: &Theme) {
    let Some(setup) = &state.setup else {
        return;
    };

    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Setup: Add search directories ")
        .border_style(Style::default().fg(theme.accent))
        .padding(Padding::uniform(1));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Layout: description, input+completions, added dirs, instructions
    let has_completions = !setup.completions.is_empty();
    let completions_height = if has_completions {
        u16::try_from(setup.completions.len())
            .unwrap_or(u16::MAX)
            .min(6)
    } else {
        0
    };

    let chunks = Layout::vertical([
        Constraint::Length(2),                  // description
        Constraint::Length(3),                  // input
        Constraint::Length(completions_height), // completions dropdown
        Constraint::Min(1),                     // added dirs
        Constraint::Length(2),                  // instructions
    ])
    .split(inner);

    // Description
    let desc = Paragraph::new("Kiosk scans these directories for git repos.")
        .style(Style::default().fg(theme.muted))
        .alignment(Alignment::Center);
    f.render_widget(desc, chunks[0]);

    // Input (reuse search_bar)
    search_bar::draw(
        f,
        chunks[1],
        &search_bar::SearchBarStyle {
            title: "Add directory",
            placeholder: "~/Development",
            border_color: theme.accent,
            muted_color: theme.muted,
        },
        &setup.input,
        setup.cursor,
    );

    // Completions dropdown
    if has_completions {
        let items: Vec<ListItem> = setup
            .completions
            .iter()
            .map(|c| ListItem::new(c.as_str()))
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(theme.border)),
            )
            .highlight_style(
                Style::default()
                    .fg(theme.highlight_fg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        let mut list_state = ListState::default();
        list_state.select(setup.selected_completion);
        f.render_stateful_widget(list, chunks[2], &mut list_state);
    }

    // Added dirs
    let dirs_area = chunks[3];
    if setup.dirs.is_empty() {
        let msg = Paragraph::new("No directories added yet.")
            .style(Style::default().fg(theme.muted))
            .alignment(Alignment::Center);
        f.render_widget(msg, dirs_area);
    } else {
        let header_and_list =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(dirs_area);

        let header = Paragraph::new(Span::styled(
            "Added:",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        f.render_widget(header, header_and_list[0]);

        let items: Vec<ListItem> = setup
            .dirs
            .iter()
            .map(|dir| {
                if path_input::path_exists(dir) {
                    ListItem::new(Line::from(Span::styled(
                        format!("  ✓ {dir}"),
                        Style::default().fg(theme.success),
                    )))
                } else {
                    ListItem::new(Line::from(Span::styled(
                        format!("  ⚠ {dir} (doesn't exist yet)"),
                        Style::default().fg(theme.warning),
                    )))
                }
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, header_and_list[1]);
    }

    // Instructions
    let instructions_area = chunks[4];
    let hints = Line::from(vec![
        Span::styled(
            "[Enter]",
            Style::default().fg(theme.hint).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" add / finish  "),
        Span::styled(
            "[Tab]",
            Style::default().fg(theme.hint).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" autocomplete  "),
        Span::styled(
            "[Esc]",
            Style::default().fg(theme.hint).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" quit"),
    ]);
    let instructions = Paragraph::new(hints).alignment(Alignment::Center);
    f.render_widget(instructions, instructions_area);
}

pub fn draw(f: &mut Frame, state: &AppState, theme: &Theme) {
    match &state.mode {
        Mode::Setup(SetupStep::Welcome) => draw_welcome(f, theme),
        Mode::Setup(SetupStep::SearchDirs) => draw_search_dirs(f, state, theme),
        _ => {}
    }
}
