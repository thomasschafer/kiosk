use kiosk_core::config::{Command, KeysConfig};
use kiosk_core::keyboard::KeyEvent;
use kiosk_core::state::{AppState, Mode};
use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use std::collections::HashMap;

/// Help overlay showing keybindings
pub fn draw(f: &mut Frame, state: &AppState, theme: &crate::theme::Theme, keys: &KeysConfig) {
    // Get the current mode context for help
    let current_mode = match &state.mode {
        Mode::Help { previous } => previous.as_ref(),
        _ => &state.mode,
    };

    // Create the help content
    let help_content = build_help_content(keys, current_mode);

    // Calculate popup size and position
    let area = f.area();
    let popup_area = super::centered_rect(80, 85, area);

    // Clear the area behind the popup
    f.render_widget(Clear, popup_area);

    // Create the main block
    let block = Block::default()
        .title("Help — key bindings")
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let paragraph = Paragraph::new(help_content)
        .block(block)
        .wrap(ratatui::widgets::Wrap { trim: true });

    f.render_widget(paragraph, popup_area);
}

/// Build the help content based on the current mode
fn build_help_content(keys: &KeysConfig, current_mode: &Mode) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(Span::styled(
        "Press C-h or Esc to close",
        Style::default().add_modifier(Modifier::ITALIC),
    )));
    lines.push(Line::from(""));

    let mode_title = match current_mode {
        Mode::RepoSelect => "Repository selection:",
        Mode::BranchSelect => "Branch selection:",
        Mode::SelectBaseBranch => "Base branch selection:",
        Mode::ConfirmWorktreeDelete { .. } => "Delete confirmation:",
        Mode::Loading(_) => "Loading:",
        Mode::Help { .. } => "General:",
    };

    lines.push(Line::from(Span::styled(
        mode_title,
        Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )));

    let mode_keymap = if matches!(current_mode, Mode::Loading(_)) {
        HashMap::new()
    } else {
        keys.keymap_for_mode(current_mode)
    };
    lines.extend(format_key_section(&mode_keymap));

    if matches!(
        current_mode,
        Mode::RepoSelect | Mode::BranchSelect | Mode::SelectBaseBranch
    ) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Search:",
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        lines.push(Line::from(
            "  Any printable character  Start/continue search",
        ));
    }

    lines
}

/// Format a section of key bindings
fn format_key_section(keymap: &HashMap<KeyEvent, Command>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if keymap.is_empty() {
        lines.push(Line::from("  (No specific bindings)"));
        return lines;
    }

    let mut bindings: Vec<_> = keymap.iter().collect();
    bindings.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));

    for (key_event, command) in bindings {
        if *command == Command::Noop {
            continue;
        }
        let key_str = key_event.to_string();
        let description = command.description();
        let line = format!("  {key_str:<13} {description}");
        lines.push(Line::from(line));
    }

    lines
}
