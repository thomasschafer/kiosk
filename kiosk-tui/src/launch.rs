//! Running a product's picker from start to finish: the terminal, first-run setup, and the
//! picker itself.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEventKind, KeyModifiers};
use kiosk_core::{
    backend::Backend,
    config::{self, LoadedConfig},
    git::GitProvider,
    picker::RunOutcome,
    product::Product,
    state::{AppState, ToastKind},
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme::Theme;

struct TerminalRestoreGuard;

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Runs `product`'s picker in the terminal. A missing config file starts the setup wizard,
/// which writes one. `current_cwd` is where the user is, for preselecting the repository and
/// branch they are in. Without a `backend` the picker can be browsed but opens nothing.
pub fn run<B: Backend, E>(
    product: &'static Product,
    mut loaded: LoadedConfig<E>,
    git: Arc<dyn GitProvider>,
    backend: Option<Arc<B>>,
    current_cwd: Option<&Path>,
) -> Result<RunOutcome> {
    let home = kiosk_core::path::home_dir();
    let theme = Theme::from_config(&loaded.core.theme);
    let _restore_guard = TerminalRestoreGuard;
    let mut terminal = ratatui::try_init()?;
    if !loaded.exists
        && let Some(path) = loaded.path.as_ref()
    {
        let Some(search_dirs) = crate::wizard::run(&mut terminal, product, path, &theme)? else {
            return Ok(RunOutcome::Quit);
        };
        loaded.core.search_dirs = search_dirs;
        loaded.exists = true;
    }
    if loaded.core.search_dirs.is_empty() {
        run_no_search_dirs(&mut terminal, loaded.path.as_deref())?;
        return Ok(RunOutcome::Quit);
    }

    let resolved = loaded
        .core
        .resolved_search_dirs_with(home.as_deref(), config::probe_search_dir)?;
    let mut warnings = loaded.warnings;
    warnings.extend(resolved.warnings);
    let current_cwd =
        current_cwd.map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)));
    let mut state = AppState::new(product, current_cwd);
    state.repo_view.missing_optional_dirs = resolved.missing_optional.len();
    warnings.extend(kiosk_core::picker::delete::load_pending(&mut state));
    for warning in warnings {
        state.push_toast(ToastKind::Warning, warning.message);
    }
    crate::app::run(
        &mut terminal,
        &mut state,
        git,
        backend,
        resolved.dirs,
        &theme,
        loaded.core.keys,
    )
}

fn run_no_search_dirs(terminal: &mut DefaultTerminal, path: Option<&Path>) -> Result<()> {
    let location = path.map_or_else(
        || "No trusted config path could be resolved.".to_string(),
        |path| {
            format!(
                "Config path: {}",
                kiosk_core::display::sanitize(&kiosk_core::path::display(path))
            )
        },
    );
    loop {
        terminal.draw(|frame| draw_no_search_dirs(frame, &location))?;
        let Event::Key(key) = ct_event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Press
            && (matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
                || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Ok(());
        }
    }
}

fn draw_no_search_dirs(frame: &mut Frame, location: &str) {
    let [_, area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Fill(1),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "No search directories configured",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::raw(location),
            Line::raw(""),
            Line::raw("q / esc / ctrl+c to quit"),
        ])
        .alignment(Alignment::Center),
        area,
    );
}
