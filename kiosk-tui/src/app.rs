//! The picker's event loop and screen.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use kiosk_core::{
    backend::Backend,
    config::{
        ResolvedSearchDir,
        keys::{BindingMode, Command, KeysConfig},
    },
    git::GitProvider,
    picker::{Picker, RunOutcome},
    state::{AppState, Mode},
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{components, theme::Theme};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(40);
const TOAST_AUTO_DISMISS: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct RedrawState {
    dirty: bool,
}

impl RedrawState {
    fn new() -> Self {
        Self { dirty: true }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn take(&mut self, animation_active: bool) -> bool {
        let redraw = self.dirty || animation_active;
        self.dirty = false;
        redraw
    }
}

/// Runs the picker until the user quits or something opens.
pub fn run<B: Backend>(
    terminal: &mut DefaultTerminal,
    state: &mut AppState,
    git: Arc<dyn GitProvider>,
    backend: Option<Arc<B>>,
    search_dirs: Vec<ResolvedSearchDir>,
    theme: &Theme,
    keys: KeysConfig,
) -> Result<RunOutcome> {
    let picker = Picker::start(git, backend, keys, search_dirs, state);
    let spinner_start = Instant::now();
    let mut redraw = RedrawState::new();

    loop {
        if state.tick_toasts(Instant::now(), TOAST_AUTO_DISMISS) {
            redraw.mark_dirty();
        }
        if redraw.take(animation_active(state)) {
            terminal.draw(|frame| draw(frame, state, theme, picker.keys(), spinner_start))?;
        }

        let tick = picker.tick(state);
        if tick.changed {
            redraw.mark_dirty();
        }
        if let Some(outcome) = tick.outcome {
            return Ok(outcome);
        }

        if event::poll(EVENT_POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let Some(action) = crate::keymap::resolve_action(key, state, picker.keys())
                    else {
                        continue;
                    };
                    redraw.mark_dirty();
                    if let Some(outcome) = picker.handle_action(state, action) {
                        return Ok(outcome);
                    }
                }
                Event::Resize(_, _) => redraw.mark_dirty(),
                _ => {}
            }
        }
    }
}

pub(crate) fn draw(
    frame: &mut Frame,
    state: &mut AppState,
    theme: &Theme,
    keys: &KeysConfig,
    spinner_start: Instant,
) {
    let loading_message = match &state.mode {
        Mode::Loading { message, .. } => Some(message.clone()),
        _ => None,
    };
    if let Some(message) = loading_message {
        let spinner =
            components::repo_list::SPINNER_FOR_LOADING[(spinner_start.elapsed().as_millis() / 80)
                as usize
                % components::repo_list::SPINNER_FOR_LOADING.len()];
        let [_, area, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(frame.area());
        let mut lines = vec![Line::from(Span::styled(
            format!("{spinner} {}", kiosk_core::display::sanitize(&message)),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))];
        if let Some(hint) = loading_hint(keys) {
            lines.push(Line::from(Span::styled(
                hint,
                Style::default().fg(theme.muted),
            )));
        }
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
        return;
    }

    let [main_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let mode = state.mode.clone();
    match &mode {
        Mode::RepoSelect => {
            components::repo_list::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::BranchSelect(_) => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::SelectBaseBranch { .. } | Mode::ValidatingNewBranch { .. } => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
            components::new_branch::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::ConfirmRemoteBranch { prompt, .. } => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
            components::new_branch::draw_remote_prompt(frame, main_area, prompt, theme, keys);
        }
        Mode::ConfirmWorktreeDelete(flow) => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
            components::delete::draw_dialog(
                frame,
                main_area,
                flow,
                state.product.backend_name,
                theme,
                keys,
                spinner_start,
            );
        }
        Mode::Loading { .. } => {
            unreachable!("loading mode returned above")
        }
    }
    let binding_mode = KeysConfig::mode_for(&state.mode);
    let footer = footer_spans(keys, binding_mode, &state.mode, theme);
    frame.render_widget(
        Paragraph::new(Line::from(footer)).alignment(Alignment::Center),
        footer_area,
    );
    components::error_toast::draw(frame, frame.area(), state, theme, keys);
    let toast_visible = !state.toasts.is_empty();
    if let Some(overlay) = &mut state.help_overlay {
        components::help::draw(frame, frame.area(), overlay, toast_visible, theme);
    }
}

fn animation_active(state: &AppState) -> bool {
    let branch_spinner =
        state.branch_view.loading || state.branch_view.fetching_remote_repo.is_some();
    match &state.mode {
        Mode::RepoSelect => state.repo_view.loading,
        Mode::BranchSelect(_)
        | Mode::SelectBaseBranch { .. }
        | Mode::ConfirmRemoteBranch { .. } => branch_spinner,
        Mode::ValidatingNewBranch { .. } | Mode::Loading { .. } => true,
        Mode::ConfirmWorktreeDelete(flow) => branch_spinner || flow.in_progress(),
    }
}

fn loading_hint(keys: &KeysConfig) -> Option<String> {
    keys.first_key(BindingMode::Modal, Command::Quit)
        .map(|key| format!("{key} to close (operation continues)"))
}

fn footer_spans<'a>(
    keys: &KeysConfig,
    binding_mode: BindingMode,
    mode: &Mode,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let mut hints = Vec::new();
    let mut add = |command, label: &'static str| {
        if let Some(key) = keys.first_key(binding_mode, command) {
            if !hints.is_empty() {
                hints.push(Span::raw("  "));
            }
            hints.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme.hint),
            ));
            hints.push(Span::raw(format!(" {label}")));
        }
    };
    if matches!(
        mode,
        Mode::ValidatingNewBranch { .. } | Mode::Loading { .. }
    ) {
        add(Command::Quit, "quit");
        return hints;
    }
    add(
        Command::Open,
        if matches!(
            mode,
            Mode::SelectBaseBranch { .. }
                | Mode::ConfirmRemoteBranch { .. }
                | Mode::ConfirmWorktreeDelete(_)
        ) {
            "confirm"
        } else {
            "open"
        },
    );
    if matches!(mode, Mode::RepoSelect) {
        add(Command::BranchesView, "branches");
    }
    if matches!(mode, Mode::BranchSelect(_)) {
        add(Command::NewBranch, "new");
        add(Command::Delete, "delete");
    }
    if matches!(
        mode,
        Mode::BranchSelect(_)
            | Mode::SelectBaseBranch { .. }
            | Mode::ConfirmRemoteBranch { .. }
            | Mode::ConfirmWorktreeDelete(_)
    ) {
        add(Command::Back, "back");
    } else {
        add(Command::Clear, "clear/quit");
    }
    add(Command::Help, "help");
    add(Command::Quit, "quit");
    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::{
        config::ThemeConfig,
        git::Repo,
        picker::new_branch::RemoteBranchPrompt,
        product::TEST_PRODUCT,
        state::{BranchContext, BranchEntry, RepoEntry, SearchableList},
    };
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn clean_idle_tick_does_not_redraw() {
        let mut redraw = RedrawState::new();

        assert!(redraw.take(false));
        assert!(!redraw.take(false));
        redraw.mark_dirty();
        assert!(redraw.take(false));
        assert!(!redraw.take(false));
    }

    #[test]
    fn active_animation_redraws_without_dirty_state() {
        let mut redraw = RedrawState::new();

        assert!(redraw.take(true));
        assert!(redraw.take(true));
        assert!(!redraw.take(false));
    }

    #[test]
    fn modal_hints_follow_effective_remapped_bindings() {
        let keys = toml::from_str::<KeysConfig>(
            "[general]\n\"C-c\" = \"noop\"\n\"C-q\" = \"quit\"\n[modal]\nenter = \"noop\"\nesc = \"noop\"\n\"C-y\" = \"open\"\n\"C-g\" = \"back\"",
        )
        .unwrap();
        assert_eq!(
            loading_hint(&keys).as_deref(),
            Some("ctrl+q to close (operation continues)")
        );
        assert_eq!(
            components::dialog::confirm_cancel_keys(&keys),
            (Some("ctrl+y".into()), Some("ctrl+g".into()))
        );
    }

    fn state_with_branch() -> AppState {
        let mut state = AppState::new(&TEST_PRODUCT, None);
        state.repo_view.entries.push(RepoEntry::new(Repo {
            name: "repo".into(),
            path: "/repo".into(),
            is_git: true,
            worktrees: Vec::new(),
        }));
        state.repo_view.list = SearchableList::new(1);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.branch_view.entries = vec![BranchEntry::local("feature")];
        state.branch_view.list = SearchableList::new(1);
        state
    }

    fn state_with_remote_branches(name: &str, remotes: &[&str]) -> AppState {
        let mut state = state_with_branch();
        state
            .branch_view
            .entries
            .extend(remotes.iter().map(|remote| BranchEntry {
                remote: Some((*remote).into()),
                ..BranchEntry::local(name)
            }));
        state.branch_view.list = SearchableList::new(state.branch_view.entries.len());
        state
    }
    #[test]
    fn validating_new_branch_keeps_branch_view_visible_under_popup() {
        let mut state = state_with_branch();
        state.mode = Mode::ValidatingNewBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            name: "feat/new".into(),
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::from_config(&ThemeConfig::default());
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &mut state,
                    &theme,
                    &KeysConfig::default(),
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
        assert!(rendered.contains("repo — select branch"));
        assert!(rendered.contains("feature"));
        assert!(rendered.contains("New branch \"feat/new\""));
        assert!(rendered.contains("Validating branch name…"));
    }

    fn render_remote_prompt(prompt: RemoteBranchPrompt) -> String {
        let mut state = state_with_remote_branches("feat/x", &["origin-a", "origin-b"]);
        state.mode = Mode::ConfirmRemoteBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            prompt,
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::from_config(&ThemeConfig::default());
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &mut state,
                    &theme,
                    &KeysConfig::default(),
                    Instant::now(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rows = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        rows.join("\n")
    }

    #[test]
    fn remote_prompt_renders_over_branch_view_with_choices_and_hints() {
        let rendered = render_remote_prompt(kiosk_core::picker::new_branch::test_remote_prompt(
            "feat/x",
            &["origin-a", "origin-b"],
            false,
        ));
        assert!(rendered.contains("repo — select branch"), "{rendered}");
        assert!(rendered.contains("Branch exists on remote"), "{rendered}");
        assert!(
            rendered.contains("feat/x already exists on 2 remotes."),
            "{rendered}"
        );
        assert!(rendered.contains("▸ Track origin-a/feat/x"), "{rendered}");
        assert!(rendered.contains("Track origin-b/feat/x"), "{rendered}");
        assert!(rendered.contains("Create new branch feat/x"), "{rendered}");
        assert!(
            rendered.contains("↑/↓ move / enter confirm / esc cancel"),
            "{rendered}"
        );
    }

    #[test]
    fn remote_prompt_explains_pending_remotes() {
        let rendered = render_remote_prompt(kiosk_core::picker::new_branch::test_remote_prompt(
            "feat/x",
            &[],
            true,
        ));
        assert!(rendered.contains("Remotes still loading"), "{rendered}");
        assert!(
            rendered.contains("Remotes are still loading, so feat/x may already exist on one."),
            "{rendered}"
        );
        assert!(
            rendered.contains("▸ Create new branch feat/x"),
            "{rendered}"
        );
        assert!(!rendered.contains("Track"), "{rendered}");

        let rendered = render_remote_prompt(kiosk_core::picker::new_branch::test_remote_prompt(
            "feat/x",
            &["origin"],
            true,
        ));
        assert!(
            rendered.contains("feat/x already exists on a remote."),
            "{rendered}"
        );
        assert!(
            rendered.contains("Other remotes are still loading."),
            "{rendered}"
        );
    }

    #[test]
    fn remote_prompt_says_when_loaded_remotes_have_no_match() {
        let rendered = render_remote_prompt(kiosk_core::picker::new_branch::test_remote_prompt(
            "feat/x",
            &[],
            false,
        ));
        assert!(rendered.contains(" New branch "), "{rendered}");
        assert!(
            rendered.contains("No remote has a branch named feat/x."),
            "{rendered}"
        );
        assert!(!rendered.contains("still loading"), "{rendered}");
        assert!(
            rendered.contains("▸ Create new branch feat/x"),
            "{rendered}"
        );
    }
}
