//! One picker session for a connected frontend: first-run setup if there is no config, then
//! the picker, until something opens or the user leaves.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyModifiers};
use kiosk_core::{
    backend::Backend,
    config::{
        LoadedConfig,
        keys::{KeyChord, KeysConfig},
    },
    git::GitProvider,
    keyboard::Action,
    keymap::{self, KeyInput},
    picker::{Picker, RunOutcome},
    product::Product,
    setup::{self, SetupOutcome, SetupState},
    state::{AppState, Mode, Overlay},
    view::{self, Viewports},
};

use crate::{
    connection::{Connection, Incoming},
    keystroke::{self, Platform},
    protocol::{ClientMessage, HEARTBEAT, Outcome, PROTOCOL_VERSION, Screen, ServerMessage},
};

/// How often the picker looks for background results while nothing else wakes it.
const TICK: Duration = Duration::from_millis(16);
const TOAST_AUTO_DISMISS: Duration = Duration::from_secs(5);
/// The most messages handled before background results and the view get a turn.
const BATCH: usize = 64;
/// How long a command waits for filtering of what was just typed.
const SETTLE: Duration = Duration::from_secs(2);

/// What a session needs to run a product's picker.
pub struct Start<B: Backend, E> {
    pub loaded: LoadedConfig<E>,
    pub git: Arc<dyn GitProvider>,
    /// Without one the picker can be browsed but opens nothing.
    pub backend: Option<Arc<B>>,
    /// Where the user is, for preselecting the repository and branch they are in.
    pub current_cwd: Option<PathBuf>,
}

/// Runs the session to the end and tells the frontend how it ended.
pub fn run<B: Backend, E>(
    connection: &Connection,
    product: &'static Product,
    start: Start<B, E>,
) -> Result<Outcome> {
    // Dropped once the frontend has been told how the session ended, so fetches stop after
    // the picker is gone rather than outliving this process.
    let _stop_fetches = kiosk_core::git::StopFetchesOnDrop::new();
    let Start {
        mut loaded,
        git,
        backend,
        current_cwd,
    } = start;
    connection.send(&ServerMessage::Hello {
        protocol: PROTOCOL_VERSION,
        product: product.name,
        chords: claimed_chords(&loaded.core.keys),
    })?;
    let mut session = Session {
        connection,
        last_view: None,
    };
    if !loaded.exists
        && let Some(path) = loaded.path.clone()
    {
        let Some(search_dirs) = session.setup(product, &path)? else {
            return session.finish(Outcome::Quit);
        };
        loaded.core.search_dirs = search_dirs;
        loaded.exists = true;
    }
    if loaded.core.search_dirs.is_empty() {
        let path = loaded.path.clone();
        session.no_search_dirs(path.as_deref())?;
        return session.finish(Outcome::Quit);
    }
    let start = kiosk_core::start::prepare(product, loaded, current_cwd.as_deref())?;
    let outcome = session.picker(start, git, backend)?;
    session.finish(outcome)
}

/// Every chord the picker or the setup wizard acts on, as GPUI spells them.
fn claimed_chords(keys: &KeysConfig) -> Vec<String> {
    let mut chords = keys
        .all_chords()
        .into_iter()
        .chain(setup::setup_chords())
        .filter_map(keystroke::unparse)
        .collect::<Vec<_>>();
    chords.sort();
    chords.dedup();
    chords
}

struct Session<'a> {
    connection: &'a Connection,
    last_view: Option<Screen>,
}

/// What a message asks of the session.
enum Step {
    Continue,
    Finish(Outcome),
}

impl Session<'_> {
    fn finish(&self, outcome: Outcome) -> Result<Outcome> {
        self.connection.send(&ServerMessage::Exit { outcome })?;
        Ok(outcome)
    }

    /// Sends `screen` unless the frontend already shows it.
    fn show(&mut self, screen: Screen) -> Result<()> {
        if self.last_view.as_ref() == Some(&screen) {
            return Ok(());
        }
        self.connection
            .send(&ServerMessage::View { view: &screen })?;
        self.last_view = Some(screen);
        Ok(())
    }

    /// The next message, or `None` once `timeout` passes without one. Fails on anything that
    /// is not a message, and on a second hello.
    fn next(&self, timeout: Duration) -> Result<Option<ClientMessage>> {
        match self.connection.receive(timeout) {
            None => {
                self.connection.keep_alive(HEARTBEAT)?;
                Ok(None)
            }
            Some(Incoming::Message(ClientMessage::Hello { .. })) => {
                bail!("the frontend said hello twice")
            }
            Some(Incoming::Message(message)) => Ok(Some(message)),
            Some(Incoming::Invalid(reason)) => {
                bail!("the frontend sent an invalid message: {reason}")
            }
            Some(Incoming::Closed) => Ok(Some(ClientMessage::Close)),
        }
    }

    /// Runs the setup wizard. Returns the search directories written to `config_path`, or
    /// `None` if the user left without saving.
    fn setup(
        &mut self,
        product: &'static Product,
        config_path: &Path,
    ) -> Result<Option<Vec<kiosk_core::config::SearchDirEntry>>> {
        let home = kiosk_core::path::home_dir();
        let shown_path = kiosk_core::path::display(config_path);
        let mut state = SetupState::default();
        loop {
            self.show(Screen::Setup(view::setup::setup(
                &state,
                product,
                &shown_path,
            )))?;
            let Some(message) = self.next(HEARTBEAT)? else {
                continue;
            };
            let outcome = match message {
                ClientMessage::Key {
                    keystroke,
                    key_char,
                } => match keystroke::parse(&keystroke, key_char.as_deref(), Platform::current()) {
                    Some(input) => state.handle_key(input, home.as_deref()),
                    None => SetupOutcome::Continue,
                },
                ClientMessage::Paste { text } => {
                    for character in text.chars().filter(|character| !character.is_control()) {
                        state.handle_key(typed(character), home.as_deref());
                    }
                    SetupOutcome::Continue
                }
                ClientMessage::FolderMode { mode } => {
                    state.choose_folder_mode(mode.into());
                    SetupOutcome::Continue
                }
                ClientMessage::Completion { index } => {
                    state.accept_completion(index, home.as_deref());
                    SetupOutcome::Continue
                }
                ClientMessage::Close => SetupOutcome::Quit,
                ClientMessage::Viewport { .. }
                | ClientMessage::Activate { .. }
                | ClientMessage::Command { .. }
                | ClientMessage::Dismiss
                | ClientMessage::Scroll { .. } => SetupOutcome::Continue,
                ClientMessage::Hello { .. } => unreachable!("next refuses a second hello"),
            };
            match outcome {
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

    /// Says the config has nothing to search until the user leaves.
    fn no_search_dirs(&mut self, config_path: Option<&Path>) -> Result<()> {
        self.show(Screen::NoSearchDirs(view::setup::no_search_dirs(
            config_path,
        )))?;
        loop {
            match self.next(HEARTBEAT)? {
                Some(ClientMessage::Close) => return Ok(()),
                Some(ClientMessage::Key {
                    keystroke,
                    key_char,
                }) => {
                    let quits =
                        keystroke::parse(&keystroke, key_char.as_deref(), Platform::current())
                            .is_some_and(|input| leaves_no_search_dirs(input.chord));
                    if quits {
                        return Ok(());
                    }
                }
                Some(_) | None => {}
            }
        }
    }

    fn picker<B: Backend>(
        &mut self,
        start: kiosk_core::start::PickerStart,
        git: Arc<dyn GitProvider>,
        backend: Option<Arc<B>>,
    ) -> Result<Outcome> {
        let kiosk_core::start::PickerStart {
            mut state,
            search_dirs,
            keys,
        } = start;
        let picker = Picker::start(git, backend, keys, search_dirs, &state);
        let mut viewports = Viewports {
            list: 20,
            bases: 10,
            help: 20,
            scan_warnings: 10,
        };
        let mut changed = true;
        loop {
            let tick = picker.tick(&mut state);
            if let Some(outcome) = tick.outcome {
                return Ok(outcome.into());
            }
            changed |= tick.changed;
            changed |= state.tick_toasts(Instant::now(), TOAST_AUTO_DISMISS);
            if std::mem::take(&mut changed) {
                self.show(Screen::Picker(view::picker(
                    &mut state,
                    picker.keys(),
                    viewports,
                )))?;
            }
            let mut timeout = TICK;
            for _ in 0..BATCH {
                let Some(message) = self.next(timeout)? else {
                    break;
                };
                changed = true;
                let step = handle_message(message, &picker, &mut state, &mut viewports);
                if let Step::Finish(outcome) = step {
                    return Ok(outcome);
                }
                timeout = Duration::ZERO;
            }
        }
    }
}

fn handle_message<B: Backend>(
    message: ClientMessage,
    picker: &Picker<B>,
    state: &mut AppState,
    viewports: &mut Viewports,
) -> Step {
    if acts_on_selection(&message)
        && let Some(outcome) = picker.settle_filters(state, SETTLE)
    {
        return Step::Finish(outcome.into());
    }
    let outcome = match message {
        ClientMessage::Key {
            keystroke,
            key_char,
        } => keystroke::parse(&keystroke, key_char.as_deref(), Platform::current())
            .and_then(|input| keymap::resolve_input(input, state, picker.keys()))
            .and_then(|action| picker.handle_action(state, action)),
        ClientMessage::Paste { text } => text
            .chars()
            .filter(|character| !character.is_control())
            .find_map(|character| picker.handle_action(state, Action::Insert(character))),
        ClientMessage::Viewport {
            list,
            bases,
            help,
            scan_warnings,
        } => {
            *viewports = Viewports {
                list: list.max(1),
                bases: bases.max(1),
                help: help.max(1),
                scan_warnings: scan_warnings.max(1),
            };
            None
        }
        ClientMessage::Activate {
            position,
            id,
            command,
        } => picker.activate(state, position, &id, command.command()),
        ClientMessage::Command { command } => command
            .action(state)
            .and_then(|action| picker.handle_action(state, action)),
        ClientMessage::Dismiss => {
            keymap::dismiss(state).and_then(|action| picker.handle_action(state, action))
        }
        ClientMessage::Scroll { delta } => picker.handle_action(
            state,
            Action::Scroll {
                delta,
                viewport_rows: viewport_in_front(state, *viewports),
            },
        ),
        ClientMessage::Close => Some(RunOutcome::Quit),
        ClientMessage::FolderMode { .. } | ClientMessage::Completion { .. } => None,
        ClientMessage::Hello { .. } => unreachable!("next refuses a second hello"),
    };
    outcome.map_or(Step::Continue, |outcome| Step::Finish(outcome.into()))
}

/// Whether `message` may act on the selection, which must first reflect what was typed.
/// Typing itself only filters, and waiting for each key's filtering would slow typing down.
fn acts_on_selection(message: &ClientMessage) -> bool {
    match message {
        ClientMessage::Key { key_char, .. } => key_char
            .as_deref()
            .is_none_or(|text| text.chars().any(char::is_control)),
        ClientMessage::Activate { .. }
        | ClientMessage::Command { .. }
        | ClientMessage::Dismiss
        | ClientMessage::Scroll { .. } => true,
        ClientMessage::Paste { .. }
        | ClientMessage::Viewport { .. }
        | ClientMessage::FolderMode { .. }
        | ClientMessage::Completion { .. }
        | ClientMessage::Close
        | ClientMessage::Hello { .. } => false,
    }
}

/// The rows the list in front has room for.
fn viewport_in_front(state: &AppState, viewports: Viewports) -> usize {
    match (&state.overlay, &state.mode) {
        (Some(Overlay::Help(_)), _) => viewports.help,
        (Some(Overlay::ScanWarnings(_)), _) => viewports.scan_warnings,
        (None, Mode::SelectBaseBranch { .. }) => viewports.bases,
        (None, _) => viewports.list,
    }
}

/// A character typed on its own, as pasted text is.
fn typed(character: char) -> KeyInput {
    KeyInput {
        chord: KeyChord::new(KeyCode::Char(character), KeyModifiers::NONE),
        text: Some(character),
    }
}

/// `q`, `esc` and `ctrl+c` leave the no-search-directories screen, as its hint says.
fn leaves_no_search_dirs(chord: KeyChord) -> bool {
    matches!(
        (chord.code, chord.modifiers),
        (KeyCode::Esc | KeyCode::Char('q'), KeyModifiers::NONE)
            | (KeyCode::Char('c'), KeyModifiers::CONTROL)
    )
}

impl From<RunOutcome> for Outcome {
    fn from(outcome: RunOutcome) -> Self {
        match outcome {
            RunOutcome::Opened => Self::Opened,
            RunOutcome::Quit => Self::Quit,
        }
    }
}
