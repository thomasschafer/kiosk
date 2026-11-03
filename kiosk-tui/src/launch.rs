//! Running a product's picker from start to finish: the terminal, first-run setup, and the
//! picker itself.

use std::{path::Path, sync::Arc};

use anyhow::Result;
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEventKind, KeyModifiers};
use kiosk_core::{
    backend::Backend, config::LoadedConfig, git::GitProvider, picker::RunOutcome, product::Product,
    view::setup::NoSearchDirsView,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme::{Theme, ThemedConfig};

struct TerminalRestoreGuard;

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Runs `product`'s picker in the terminal. A missing config file starts the setup wizard,
/// which writes one. `current_cwd` is where the user is, for preselecting the repository and
/// branch they are in. Without a `backend` the picker can be browsed but opens nothing.
pub fn run<B: Backend, E: ThemedConfig>(
    product: &'static Product,
    mut loaded: LoadedConfig<E>,
    git: Arc<dyn GitProvider>,
    backend: Option<Arc<B>>,
    current_cwd: Option<&Path>,
) -> Result<RunOutcome> {
    let theme = Theme::from_config(loaded.extension.theme());
    // Dropped after the terminal is restored, so the picker is gone while fetches stop.
    let _stop_fetches = kiosk_core::git::StopFetchesOnDrop;
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

    let kiosk_core::start::PickerStart {
        mut state,
        search_dirs,
        keys,
    } = kiosk_core::start::prepare(product, loaded, current_cwd)?;
    crate::app::run(
        &mut terminal,
        &mut state,
        git,
        backend,
        search_dirs,
        &theme,
        keys,
    )
}

fn run_no_search_dirs(terminal: &mut DefaultTerminal, path: Option<&Path>) -> Result<()> {
    let view = kiosk_core::view::setup::no_search_dirs(path);
    loop {
        terminal.draw(|frame| draw_no_search_dirs(frame, &view))?;
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

fn draw_no_search_dirs(frame: &mut Frame, view: &NoSearchDirsView) {
    let [_, area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Fill(1),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                view.title,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::raw(view.location.as_str()),
            Line::raw(""),
            Line::raw(view.hint),
        ])
        .alignment(Alignment::Center),
        area,
    );
}
