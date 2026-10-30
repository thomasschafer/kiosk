//! The first-run setup wizard.

use std::path::Path;

use anyhow::Result;
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kiosk_core::{
    config::SearchDirEntry,
    product::Product,
    setup::{self, SetupState, SetupStep},
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
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }
        match &state.step {
            SetupStep::Welcome => match key.code {
                KeyCode::Enter => state.continue_from_welcome(),
                KeyCode::Esc => return Ok(None),
                _ => {}
            },
            SetupStep::Directories => {
                if handle_directory_key(&mut state, key, home.as_deref()) {
                    return Ok(None);
                }
            }
            SetupStep::Depth { .. } => handle_depth_key(&mut state, key, home.as_deref()),
            SetupStep::FolderMode { .. } => handle_folder_mode_key(&mut state, key),
            SetupStep::Confirm => match key.code {
                KeyCode::Enter => {
                    let search_dirs = state.search_dirs();
                    setup::write_config_atomic(config_path, &search_dirs)?;
                    return Ok(Some(search_dirs));
                }
                KeyCode::Esc => {
                    state.step = SetupStep::Directories;
                    state.input.clear();
                }
                _ => {}
            },
        }
    }
}

fn handle_directory_key(state: &mut SetupState, key: KeyEvent, home: Option<&Path>) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Enter => {
            if let Err(message) = state.begin_depth() {
                state.message = Some(message.into());
            }
        }
        KeyCode::Tab => state.tab_complete(home),
        KeyCode::Esc if state.input.text.is_empty() => return true,
        KeyCode::Esc => {
            state.input.clear();
            state.update_completions(home);
        }
        KeyCode::Char('x') if control => {
            if state.remove_last().is_none() {
                state.message = Some("No directory to remove".into());
            }
        }
        KeyCode::Backspace if alt => {
            state.input.delete_word();
            state.update_completions(home);
        }
        KeyCode::Backspace => {
            state.input.backspace();
            state.update_completions(home);
        }
        KeyCode::Char('w') if control => {
            state.input.delete_word();
            state.update_completions(home);
        }
        KeyCode::Left if !control && !alt => state.input.cursor_left(),
        KeyCode::Right if !control && !alt => state.input.cursor_right(),
        KeyCode::Char(character) if !control && !alt && !character.is_control() => {
            state.input.insert_char(character);
            state.message = None;
            state.update_completions(home);
        }
        _ => {}
    }
    false
}

fn handle_depth_key(state: &mut SetupState, key: KeyEvent, home: Option<&Path>) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Enter => {
            if let Err(message) = state.commit_depth() {
                state.message = Some(message);
            }
        }
        KeyCode::Esc => {
            state.cancel_depth();
            state.update_completions(home);
        }
        KeyCode::Backspace => {
            state.depth_default_pristine = false;
            state.input.backspace();
        }
        KeyCode::Char(character) if character.is_ascii_digit() && !control && !alt => {
            if state.depth_default_pristine {
                state.input.clear();
            }
            state.depth_default_pristine = false;
            state.input.insert_char(character);
        }
        _ => {}
    }
}

fn handle_folder_mode_key(state: &mut SetupState, key: KeyEvent) {
    match key.code {
        KeyCode::Up | KeyCode::Left => state.select_previous_folder_mode(),
        KeyCode::Down | KeyCode::Right => state.select_next_folder_mode(),
        KeyCode::Enter => {
            if let Err(message) = state.commit_folder_mode() {
                state.message = Some(message.into());
            }
        }
        KeyCode::Esc => state.cancel_folder_mode(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_directory_input_accepts_shifted_uppercase_letters() {
        let mut state = SetupState::default();

        handle_directory_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
            None,
        );

        assert_eq!(state.input.text, "A");
    }

    #[test]
    fn wizard_directory_input_accepts_unicode() {
        let mut state = SetupState::default();
        for character in ['é', '界'] {
            handle_directory_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                None,
            );
        }
        assert_eq!(state.input.text, "é界");
    }

    #[test]
    fn wizard_depth_input_accepts_shift_modifier_and_rejects_control() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "/Repo".into();
        state.begin_depth().unwrap();

        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::SHIFT),
            None,
        );
        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('3'), KeyModifiers::CONTROL),
            None,
        );

        assert_eq!(state.input.text, "2");
    }

    #[test]
    fn wizard_depth_replaces_the_default_only_on_the_first_edit() {
        for (digits, expected) in [("10", "10"), ("12", "12"), ("2", "2")] {
            let mut state = SetupState::default();
            state.continue_from_welcome();
            state.input.text = "/repo".into();
            state.input.cursor = state.input.text.len();
            state.begin_depth().unwrap();
            for digit in digits.chars() {
                handle_depth_key(
                    &mut state,
                    KeyEvent::new(KeyCode::Char(digit), KeyModifiers::NONE),
                    None,
                );
            }
            assert_eq!(state.input.text, expected);
        }
    }

    #[test]
    fn backspace_removes_the_pristine_depth_default_once() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "/repo".into();
        state.input.cursor = state.input.text.len();
        state.begin_depth().unwrap();
        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            None,
        );
        assert_eq!(state.input.text, "");
        assert!(!state.depth_default_pristine);
        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            None,
        );
        assert_eq!(state.input.text, "2");
    }

    #[test]
    fn folder_mode_escape_restores_the_depth_for_editing() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "/repo".into();
        state.begin_depth().unwrap();
        state.input.text = "12".into();
        state.input.cursor = 2;
        state.commit_depth().unwrap();

        handle_folder_mode_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(
            state.step,
            SetupStep::Depth {
                path: "/repo".into()
            }
        );
        assert_eq!(state.input.text, "12");
        assert_eq!(state.input.cursor, 2);
        assert!(!state.depth_default_pristine);
    }
}
