//! The first-run setup wizard.

use std::path::Path;

use anyhow::Result;
use crossterm::event::{self as ct_event, Event, KeyEventKind};
use kiosk_core::{
    config::SearchDirEntry,
    keymap::KeyInput,
    product::Product,
    setup::{self, SetupOutcome, SetupState},
};
use ratatui::DefaultTerminal;

use crate::{components, theme::Theme};

/// Asks for the directories to search, then writes them to a new config at `config_path`.
/// Returns `None` if the user leaves without saving.
pub(crate) fn run(
    terminal: &mut DefaultTerminal,
    product: &Product,
    config_path: &Path,
    theme: &Theme,
) -> Result<Option<Vec<SearchDirEntry>>> {
    let mut state = SetupState::default();
    let home = kiosk_core::path::home_dir();
    let path_display =
        kiosk_core::display::sanitize(&kiosk_core::path::display(config_path)).into_owned();
    loop {
        terminal.draw(|frame| {
            components::setup::draw(frame, &state, product, theme, &path_display);
        })?;
        let Event::Key(key) = ct_event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match state.handle_key(KeyInput::from_event(key), home.as_deref()) {
            SetupOutcome::Continue => {}
            SetupOutcome::Quit => return Ok(None),
            SetupOutcome::Save => {
                let search_dirs = state.search_dirs();
                setup::write_config_atomic(config_path, &search_dirs)?;
                return Ok(Some(search_dirs));
            }
        }
    }
}
