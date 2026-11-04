use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
};

use crate::theme::Theme;
use kiosk_core::{
    product::Product,
    setup::SetupState,
    view::{
        Hint, Tone,
        setup::{self, AddedDir, Field, FolderModeOption, SetupView},
    },
};

/// The separator between hints on the wizard's screens.
const HINT_SEPARATOR: &str = "   ";

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let [horizontal] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [vertical] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(horizontal);
    vertical
}

pub fn draw(
    frame: &mut Frame,
    state: &SetupState,
    product: &Product,
    theme: &Theme,
    config_path: &str,
) {
    let area = centered(frame.area().width.saturating_mul(9) / 10, 24, frame.area());
    frame.render_widget(Clear, area);
    match setup::setup(state, product, config_path) {
        SetupView::Welcome {
            title,
            heading,
            intro,
            explanation,
            config_label,
            config_path,
            hints,
        } => {
            let lines = vec![
                Line::raw(""),
                heading_line(heading, theme),
                Line::raw(""),
                Line::raw(intro),
                Line::raw(explanation),
                Line::raw(""),
                labelled(config_label, config_path, theme),
                Line::raw(""),
                hint_line(&hints, theme),
            ];
            draw_paragraph(frame, area, title, lines, theme);
        }
        view @ SetupView::Directories { .. } => draw_directories(frame, area, view, theme),
        SetupView::FolderMode {
            title,
            question,
            directory_label,
            directory,
            depth,
            options,
            hints,
        } => draw_folder_mode(
            frame,
            area,
            theme,
            [title, question, directory_label],
            [directory, depth],
            &options,
            &hints,
        ),
        SetupView::Confirm {
            title,
            heading,
            dirs,
            write_label,
            config_path,
            hints,
        } => {
            let mut lines = vec![Line::raw(""), heading_line(heading, theme), Line::raw("")];
            lines.extend(dirs.iter().map(|dir| {
                Line::from(vec![
                    Span::styled("✓ ", theme.style(Tone::Open)),
                    Span::raw(format!("{}  ({})", dir.path, dir.depth)),
                ])
            }));
            lines.extend([
                Line::raw(""),
                labelled(write_label, config_path, theme),
                Line::raw(""),
                hint_line(&hints, theme),
            ]);
            draw_paragraph(frame, area, title, lines, theme);
        }
    }
}

fn draw_directories(frame: &mut Frame, area: Rect, view: SetupView, theme: &Theme) {
    let SetupView::Directories {
        title,
        instructions,
        path,
        depth,
        completions,
        selected_completion,
        added_title,
        added,
        nothing_added,
        message,
        hints,
    } = view
    else {
        unreachable!("only the directories step is drawn here");
    };
    let block = shell(title, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let auxiliary_height = if depth.is_some() {
        3
    } else {
        u16::try_from(completions.len()).unwrap_or(u16::MAX)
    };
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(auxiliary_height),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(instructions).alignment(Alignment::Center),
        chunks[0],
    );
    draw_field(frame, chunks[1], &path, theme.border, theme);
    if let Some(depth) = &depth {
        draw_field(frame, chunks[2], depth, theme.accent, theme);
    } else {
        let completions = completions
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let marker = if selected_completion == Some(index) {
                    "▸ "
                } else {
                    "  "
                };
                ListItem::new(format!("{marker}{value}"))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(completions).style(theme.style(Tone::Muted)),
            chunks[2],
        );
    }
    let dirs = if added.is_empty() {
        vec![ListItem::new(Span::styled(
            nothing_added,
            theme.style(Tone::Muted),
        ))]
    } else {
        added
            .iter()
            .map(|dir| ListItem::new(added_line(dir, theme)))
            .collect()
    };
    frame.render_widget(
        List::new(dirs).block(
            Block::default()
                .title(format!(" {added_title} "))
                .borders(Borders::TOP),
        ),
        chunks[3],
    );
    let (text, tone) = message.map_or_else(
        || (hint_text(&hints), Tone::Muted),
        |message| (message, Tone::Warning),
    );
    frame.render_widget(
        Paragraph::new(text)
            .style(theme.style(tone))
            .alignment(Alignment::Center),
        chunks[4],
    );
}

fn shell(title: &str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(theme.accent))
        .padding(Padding::uniform(1))
}

fn draw_paragraph(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'_>>, theme: &Theme) {
    frame.render_widget(
        Paragraph::new(lines)
            .block(shell(title, theme))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn heading_line(heading: impl Into<String>, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        heading.into(),
        theme.style(Tone::Accent).add_modifier(Modifier::BOLD),
    ))
}

fn labelled(label: &'static str, value: String, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(label, theme.style(Tone::Muted)),
        Span::raw(value),
    ])
}

fn hint_line(hints: &[Hint], theme: &Theme) -> Line<'static> {
    super::dialog::hint_line(hints, HINT_SEPARATOR, theme).unwrap_or_default()
}

/// Hints as plain text, for lines drawn in one colour.
fn hint_text(hints: &[Hint]) -> String {
    hints
        .iter()
        .map(|hint| format!("{} {}", hint.key, hint.label))
        .collect::<Vec<_>>()
        .join(HINT_SEPARATOR)
}

fn draw_field(
    frame: &mut Frame,
    area: Rect,
    field: &Field,
    border_color: ratatui::style::Color,
    theme: &Theme,
) {
    super::search_bar::draw(
        frame,
        area,
        &super::search_bar::SearchBarStyle {
            title: field.label,
            placeholder: field.placeholder,
            border_color,
            muted_color: theme.muted,
        },
        &format!("{}{}", field.query.before, field.query.after),
        field.query.before.len(),
    );
}

fn added_line(dir: &AddedDir, theme: &Theme) -> Line<'static> {
    let mut spans = vec![
        Span::styled("✓ ", theme.style(Tone::Open)),
        Span::raw(dir.path.clone()),
        Span::styled(format!("  {}", dir.depth), theme.style(Tone::Muted)),
    ];
    if dir.all_folders {
        spans.push(Span::styled(
            format!("  {}", setup::ALL_FOLDERS_SUFFIX),
            theme.style(Tone::Muted),
        ));
    }
    Line::from(spans)
}

fn draw_folder_mode(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    [title, question, directory_label]: [&'static str; 3],
    [directory, depth]: [String; 2],
    options: &[FolderModeOption],
    hints: &[Hint],
) {
    let block = shell(title, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(question).alignment(Alignment::Center),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(directory_label, theme.style(Tone::Muted)),
            Span::raw(directory),
            Span::styled(format!("  {depth}"), theme.style(Tone::Muted)),
        ]))
        .alignment(Alignment::Center),
        chunks[1],
    );
    let options = options
        .iter()
        .map(|option| {
            let style = if option.selected {
                theme.style(Tone::Accent).add_modifier(Modifier::BOLD)
            } else {
                theme.style(Tone::Muted)
            };
            ListItem::new(format!(
                "{} {}",
                if option.selected { ">" } else { " " },
                option.label
            ))
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(options), chunks[2]);
    frame.render_widget(
        Paragraph::new(hint_text(hints))
            .style(theme.style(Tone::Hint))
            .alignment(Alignment::Center),
        chunks[4],
    );
}

#[cfg(test)]
mod tests {
    use kiosk_core::product::TEST_PRODUCT;
    use ratatui::{Terminal, backend::TestBackend};

    use crate::theme::Theme;
    use kiosk_core::{
        config::ThemeConfig,
        setup::{SetupDir, SetupState, SetupStep},
    };

    use super::draw;

    #[test]
    fn added_directories_tag_only_the_all_folders_mode() {
        let mut state = SetupState {
            step: SetupStep::Directories,
            ..SetupState::default()
        };
        state.dirs = vec![
            SetupDir {
                path: "~/GitOnly".into(),
                depth: 1,
                include_non_git: None,
            },
            SetupDir {
                path: "~/Everything".into(),
                depth: 2,
                include_non_git: Some(true),
            },
        ];
        let theme = Theme::from_config(&ThemeConfig::default());
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw(frame, &state, &TEST_PRODUCT, &theme, "/config.toml"))
            .unwrap();

        let rows = terminal
            .backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|cells| {
                cells
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let git_only = rows
            .iter()
            .find(|row| row.contains("~/GitOnly"))
            .expect("git-only directory row");
        let all_folders = rows
            .iter()
            .find(|row| row.contains("~/Everything"))
            .expect("all-folders directory row");
        assert!(!git_only.contains("all folders"));
        assert!(all_folders.contains("all folders"));
    }
}
