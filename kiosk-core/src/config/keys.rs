use std::{
    collections::HashMap,
    fmt::{self, Write as _},
    str::FromStr,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

use crate::{
    product::Product,
    state::{Mode, TextEdit},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn from_event(event: KeyEvent) -> Self {
        let (code, mut modifiers) = if event.code == KeyCode::BackTab {
            (KeyCode::Tab, event.modifiers | KeyModifiers::SHIFT)
        } else {
            (event.code, event.modifiers)
        };
        let code = match code {
            KeyCode::Char(character)
                if modifiers.contains(KeyModifiers::SHIFT) && character.is_ascii_alphabetic() =>
            {
                KeyCode::Char(character.to_ascii_uppercase())
            }
            KeyCode::Char(character) => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::Char(if character.is_ascii_alphabetic() {
                    character.to_ascii_lowercase()
                } else {
                    character
                })
            }
            code => code,
        };
        Self { code, modifiers }
    }
}

impl FromStr for KeyChord {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err("missing key chord".into());
        }
        let delimiter = value
            .split_once('+')
            .filter(|(first, _)| is_modifier_token(first))
            .map_or('-', |_| '+');
        let mut tokens = value.split(delimiter).collect::<Vec<_>>();
        let key = tokens.pop().ok_or("missing key code")?;
        let mut modifiers = KeyModifiers::NONE;
        for token in tokens {
            let modifier = match token.to_ascii_lowercase().as_str() {
                "c" | "ctrl" | "control" => KeyModifiers::CONTROL,
                "a" | "alt" | "m" => KeyModifiers::ALT,
                "s" | "shift" => KeyModifiers::SHIFT,
                _ => return Err(format!("invalid key modifier '{token}-'")),
            };
            if modifiers.contains(modifier) {
                return Err(format!("repeated key modifier '{token}-'"));
            }
            modifiers.insert(modifier);
        }
        let lower = key.to_ascii_lowercase();
        let code = match lower.as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "del" | "delete" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "page_up" => KeyCode::PageUp,
            "pagedown" | "page_down" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            _ if function_key(&lower).is_some() => {
                KeyCode::F(function_key(&lower).expect("checked above"))
            }
            _ if key.chars().count() == 1 => {
                let mut character = key.chars().next().expect("one character");
                if modifiers.contains(KeyModifiers::SHIFT) {
                    if !character.is_ascii_alphabetic() {
                        return Err(format!(
                            "shift modifier cannot be used with non-letter key '{key}'; bind the resulting character directly"
                        ));
                    }
                    character = character.to_ascii_uppercase();
                } else if character.is_ascii_alphabetic()
                    || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    character = character.to_ascii_lowercase();
                }
                KeyCode::Char(character)
            }
            _ => return Err(format!("invalid key code '{key}'")),
        };
        Ok(Self { code, modifiers })
    }
}

/// The number of a function key spelled `f1` to `f12`.
pub fn function_key(name: &str) -> Option<u8> {
    name.strip_prefix('f')
        .filter(|digits| !digits.starts_with('0'))
        .and_then(|digits| digits.parse().ok())
        .filter(|number| (1..=12).contains(number))
}

fn is_modifier_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "c" | "ctrl" | "control" | "a" | "alt" | "m" | "s" | "shift"
    )
}

impl fmt::Display for KeyChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            formatter.write_str("ctrl+")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            formatter.write_str("alt+")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            formatter.write_str("shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => formatter.write_str("space"),
            KeyCode::Char(character) => write!(formatter, "{}", character.to_ascii_lowercase()),
            KeyCode::Enter => formatter.write_str("enter"),
            KeyCode::Esc => formatter.write_str("esc"),
            KeyCode::Tab => formatter.write_str("tab"),
            KeyCode::Backspace => formatter.write_str("backspace"),
            KeyCode::Delete => formatter.write_str("delete"),
            KeyCode::Up => formatter.write_str("↑"),
            KeyCode::Down => formatter.write_str("↓"),
            KeyCode::Left => formatter.write_str("←"),
            KeyCode::Right => formatter.write_str("→"),
            KeyCode::Home => formatter.write_str("home"),
            KeyCode::End => formatter.write_str("end"),
            KeyCode::PageUp => formatter.write_str("pageup"),
            KeyCode::PageDown => formatter.write_str("pagedown"),
            KeyCode::F(number) => write!(formatter, "f{number}"),
            _ => formatter.write_str("unknown"),
        }
    }
}

impl Serialize for KeyChord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Noop,
    Quit,
    Help,
    DismissToast,
    ScanWarnings,
    MoveUp,
    MoveDown,
    Open,
    BranchesView,
    Back,
    NewBranch,
    Delete,
    Clear,
    Backspace,
    BackspaceOrBack,
    DeleteForward,
    DeleteWord,
    DeleteWordForward,
    DeleteToEnd,
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    CursorStart,
    CursorEnd,
}

impl Command {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Noop => "noop",
            Self::Quit => "quit",
            Self::Help => "help",
            Self::DismissToast => "dismiss_toast",
            Self::ScanWarnings => "scan_warnings",
            Self::MoveUp => "move_up",
            Self::MoveDown => "move_down",
            Self::Open => "open",
            Self::BranchesView => "branches_view",
            Self::Back => "back",
            Self::NewBranch => "new_branch",
            Self::Delete => "delete",
            Self::Clear => "clear",
            Self::Backspace => "backspace",
            Self::BackspaceOrBack => "backspace_or_back",
            Self::DeleteForward => "delete_forward",
            Self::DeleteWord => "delete_word",
            Self::DeleteWordForward => "delete_word_forward",
            Self::DeleteToEnd => "delete_to_end",
            Self::CursorLeft => "cursor_left",
            Self::CursorRight => "cursor_right",
            Self::CursorWordLeft => "cursor_word_left",
            Self::CursorWordRight => "cursor_word_right",
            Self::CursorStart => "cursor_start",
            Self::CursorEnd => "cursor_end",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Noop => "Unbound",
            Self::Quit => "Quit the picker",
            Self::Help => "Show active key bindings",
            Self::DismissToast => "Dismiss the visible notification",
            Self::ScanWarnings => "Show directories that could not be scanned",
            Self::MoveUp => "Move selection up",
            Self::MoveDown => "Move selection down",
            Self::Open => "Open or confirm the selection",
            Self::BranchesView => "Browse repository branches",
            Self::Back => "Go back or cancel",
            Self::NewBranch => "Create a new branch",
            Self::Delete => "Delete the selected checkout",
            Self::Clear => "Clear the search query",
            Self::Backspace => "Delete the previous character",
            Self::BackspaceOrBack => {
                "Delete the previous character, or go back when the search is empty"
            }
            Self::DeleteForward => "Delete the next character",
            Self::DeleteWord => "Delete the previous word",
            Self::DeleteWordForward => "Delete the next word",
            Self::DeleteToEnd => "Delete to the end",
            Self::CursorLeft => "Move the cursor left",
            Self::CursorRight => "Move the cursor right",
            Self::CursorWordLeft => "Move the cursor back a word",
            Self::CursorWordRight => "Move the cursor forward a word",
            Self::CursorStart => "Move the cursor to the start",
            Self::CursorEnd => "Move the cursor to the end",
        }
    }

    /// The edit a text-editing command makes, or `None` for any other command.
    pub const fn text_edit(self) -> Option<TextEdit> {
        Some(match self {
            Self::Clear => TextEdit::Clear,
            Self::Backspace => TextEdit::Backspace,
            Self::DeleteForward => TextEdit::DeleteForward,
            Self::DeleteWord => TextEdit::DeleteWordBackward,
            Self::DeleteWordForward => TextEdit::DeleteWordForward,
            Self::DeleteToEnd => TextEdit::DeleteToEnd,
            Self::CursorLeft => TextEdit::CursorLeft,
            Self::CursorRight => TextEdit::CursorRight,
            Self::CursorWordLeft => TextEdit::CursorWordLeft,
            Self::CursorWordRight => TextEdit::CursorWordRight,
            Self::CursorStart => TextEdit::CursorStart,
            Self::CursorEnd => TextEdit::CursorEnd,
            Self::Noop
            | Self::Quit
            | Self::Help
            | Self::DismissToast
            | Self::ScanWarnings
            | Self::MoveUp
            | Self::MoveDown
            | Self::Open
            | Self::BranchesView
            | Self::Back
            | Self::BackspaceOrBack
            | Self::NewBranch
            | Self::Delete => return None,
        })
    }
}

impl FromStr for Command {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "noop" | "none" | "unbound" => Ok(Self::Noop),
            "quit" => Ok(Self::Quit),
            "help" | "show_help" => Ok(Self::Help),
            "dismiss_toast" => Ok(Self::DismissToast),
            "scan_warnings" => Ok(Self::ScanWarnings),
            "move_up" => Ok(Self::MoveUp),
            "move_down" => Ok(Self::MoveDown),
            "open" => Ok(Self::Open),
            "branches_view" | "enter_repo" => Ok(Self::BranchesView),
            "back" | "go_back" => Ok(Self::Back),
            "new_branch" => Ok(Self::NewBranch),
            "delete" | "delete_worktree" => Ok(Self::Delete),
            "clear" | "clear_query" => Ok(Self::Clear),
            "backspace" => Ok(Self::Backspace),
            "backspace_or_back" => Ok(Self::BackspaceOrBack),
            "delete_forward" => Ok(Self::DeleteForward),
            "delete_word" => Ok(Self::DeleteWord),
            "delete_word_forward" => Ok(Self::DeleteWordForward),
            "delete_to_end" => Ok(Self::DeleteToEnd),
            "cursor_left" => Ok(Self::CursorLeft),
            "cursor_right" => Ok(Self::CursorRight),
            "cursor_word_left" => Ok(Self::CursorWordLeft),
            "cursor_word_right" => Ok(Self::CursorWordRight),
            "cursor_start" => Ok(Self::CursorStart),
            "cursor_end" => Ok(Self::CursorEnd),
            _ => Err(format!("unknown key action '{value}'")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// Layered key bindings for the picker and its dialogs. Keys use lowercase
/// `ctrl+`, `alt+`, and `shift+` modifiers with a character or a named key such as
/// `enter`, `esc`, `tab`, `backspace`, `delete`, an arrow name, `home`, `end`,
/// `pageup`, `pagedown`, or `space`. User bindings extend the defaults; assign
/// `"noop"` to unbind an inherited key.
pub struct KeysConfig {
    /// Bindings active everywhere. Accepted actions are `quit`, `help`,
    /// `dismiss_toast`, `scan_warnings`, and `noop`.
    general: HashMap<KeyChord, Command>,
    /// Bindings active while editing search or name text. Accepted actions are
    /// `clear`, `backspace`, `delete_word`, `cursor_left`, `cursor_right`, and `noop`.
    text_edit: HashMap<KeyChord, Command>,
    /// Bindings active in navigable lists. Accepted actions are `move_up`,
    /// `move_down`, and `noop`.
    list_navigation: HashMap<KeyChord, Command>,
    /// Bindings active in confirmation dialogs, the remote-branch prompt, and the
    /// base-branch picker. Accepted actions are `open`, `back`, and `noop`.
    modal: HashMap<KeyChord, Command>,
    /// Bindings specific to the repository picker. Accepted actions are `open`,
    /// `branches_view`, `quit`, and `noop`.
    repo_select: HashMap<KeyChord, Command>,
    /// Bindings specific to the branch picker. Accepted actions are `open`, `back`,
    /// `new_branch`, `delete`, `backspace`, `backspace_or_back`, and `noop`.
    /// `backspace_or_back` deletes like `backspace` but goes back to the repositories
    /// when the search is empty; bind the key to `backspace` instead to only delete.
    branch_select: HashMap<KeyChord, Command>,
}

/// The `[keys]` section as written, before it is layered over the defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RawKeysConfig {
    general: HashMap<String, String>,
    text_edit: HashMap<String, String>,
    list_navigation: HashMap<String, String>,
    modal: HashMap<String, String>,
    repo_select: HashMap<String, String>,
    branch_select: HashMap<String, String>,
}

impl RawKeysConfig {
    fn into_layers(self) -> [(KeyLayer, HashMap<String, String>); 6] {
        [
            (KeyLayer::General, self.general),
            (KeyLayer::TextEdit, self.text_edit),
            (KeyLayer::ListNavigation, self.list_navigation),
            (KeyLayer::Modal, self.modal),
            (KeyLayer::RepoSelect, self.repo_select),
            (KeyLayer::BranchSelect, self.branch_select),
        ]
    }
}

/// A section of [`KeysConfig`], named as in the `[keys]` config section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLayer {
    General,
    TextEdit,
    ListNavigation,
    Modal,
    RepoSelect,
    BranchSelect,
}

impl KeyLayer {
    pub const ALL: [Self; 6] = [
        Self::General,
        Self::TextEdit,
        Self::ListNavigation,
        Self::Modal,
        Self::RepoSelect,
        Self::BranchSelect,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::TextEdit => "text_edit",
            Self::ListNavigation => "list_navigation",
            Self::Modal => "modal",
            Self::RepoSelect => "repo_select",
            Self::BranchSelect => "branch_select",
        }
    }

    /// The commands a binding in this section may name.
    pub const fn allowed(self) -> &'static [Command] {
        match self {
            Self::General => GENERAL,
            Self::TextEdit => TEXT_EDIT,
            Self::ListNavigation => LIST_NAVIGATION,
            Self::Modal => MODAL,
            Self::RepoSelect => REPO_SELECT,
            Self::BranchSelect => BRANCH_SELECT,
        }
    }
}

/// A binding a product adds to kiosk's defaults: the section it goes in, its chord as
/// the config writes chords, and its command.
pub type KeyBinding = (KeyLayer, &'static str, Command);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingMode {
    Repo,
    Branch,
    BaseBranch,
    Choice,
    Modal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub name: &'static str,
    pub bindings: Vec<(KeyChord, Command)>,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            general: map(&[
                ("C-c", Command::Quit),
                ("C-h", Command::Help),
                ("f1", Command::Help),
                ("C-s", Command::ScanWarnings),
            ]),
            text_edit: map(&[
                ("C-u", Command::Clear),
                ("backspace", Command::Backspace),
                ("delete", Command::DeleteForward),
                ("C-d", Command::DeleteForward),
                ("A-backspace", Command::DeleteWord),
                ("C-w", Command::DeleteWord),
                ("A-d", Command::DeleteWordForward),
                ("A-delete", Command::DeleteWordForward),
                ("C-k", Command::DeleteToEnd),
                ("left", Command::CursorLeft),
                ("C-b", Command::CursorLeft),
                ("right", Command::CursorRight),
                ("C-f", Command::CursorRight),
                ("A-left", Command::CursorWordLeft),
                ("C-left", Command::CursorWordLeft),
                ("A-b", Command::CursorWordLeft),
                ("A-right", Command::CursorWordRight),
                ("C-right", Command::CursorWordRight),
                ("A-f", Command::CursorWordRight),
                ("home", Command::CursorStart),
                ("C-a", Command::CursorStart),
                ("end", Command::CursorEnd),
                ("C-e", Command::CursorEnd),
            ]),
            list_navigation: map(&[
                ("up", Command::MoveUp),
                ("C-p", Command::MoveUp),
                ("down", Command::MoveDown),
                ("C-n", Command::MoveDown),
            ]),
            modal: map(&[("enter", Command::Open), ("esc", Command::Back)]),
            repo_select: map(&[
                ("enter", Command::Open),
                ("tab", Command::BranchesView),
                ("esc", Command::Quit),
            ]),
            branch_select: map(&[
                ("enter", Command::Open),
                ("esc", Command::Back),
                ("C-o", Command::NewBranch),
                ("C-x", Command::Delete),
            ]),
        }
    }
}

fn map(entries: &[(&str, Command)]) -> HashMap<KeyChord, Command> {
    entries
        .iter()
        .map(|(key, command)| (key.parse().expect("valid default chord"), *command))
        .collect()
}

/// `keys`' bindings as a TOML block for the config reference.
pub fn default_keys_toml(keys: &KeysConfig) -> String {
    let mut output = String::from("```toml\n");
    for (layer, bindings) in keys.layers() {
        let _ = writeln!(output, "[keys.{}]", layer.name());
        let mut bindings = bindings.iter().collect::<Vec<_>>();
        bindings.sort_by(|(left_key, left_command), (right_key, right_command)| {
            config_key_name(left_key)
                .cmp(&config_key_name(right_key))
                .then_with(|| left_command.name().cmp(right_command.name()))
        });
        for (key, command) in bindings {
            let _ = writeln!(
                output,
                "\"{}\" = \"{}\"",
                escape_toml_string(&config_key_name(key)),
                escape_toml_string(command.name())
            );
        }
        output.push('\n');
    }
    output.push_str("```\n");
    output
}

fn config_key_name(key: &KeyChord) -> String {
    let mut output = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        output.push_str("ctrl+");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        output.push_str("alt+");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        output.push_str("shift+");
    }
    output.push_str(match key.code {
        KeyCode::Char(' ') => "space",
        KeyCode::Enter => "enter",
        KeyCode::Esc => "esc",
        KeyCode::Tab => "tab",
        KeyCode::Backspace => "backspace",
        KeyCode::Delete => "delete",
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::PageUp => "pageup",
        KeyCode::PageDown => "pagedown",
        KeyCode::F(number) => return format!("{output}f{number}"),
        KeyCode::Char(character) => return format!("{output}{}", character.to_ascii_lowercase()),
        _ => "unknown",
    });
    output
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

impl KeysConfig {
    /// kiosk's default bindings with `product`'s over them.
    pub fn for_product(product: &Product) -> Result<Self, String> {
        Self::from_raw(RawKeysConfig::default(), product)
    }

    /// The user's bindings over `product`'s, over kiosk's defaults.
    pub(crate) fn from_raw(raw: RawKeysConfig, product: &Product) -> Result<Self, String> {
        let mut keys = Self::default();
        for &(layer, chord, command) in product.keys {
            keys.bind(layer, chord, command)
                .map_err(|error| format!("{}'s default keys: {error}", product.name))?;
        }
        for (layer, bindings) in raw.into_layers() {
            for (chord, action) in bindings {
                keys.bind(layer, &chord, action.parse()?)?;
            }
        }
        Ok(keys)
    }

    fn bind(&mut self, layer: KeyLayer, chord: &str, command: Command) -> Result<(), String> {
        let parsed = chord
            .parse::<KeyChord>()
            .map_err(|error| format!("invalid key chord '{chord}': {error}"))?;
        if !layer.allowed().contains(&command) {
            return Err(format!(
                "key action '{}' is not allowed in [keys.{}]",
                command.name(),
                layer.name()
            ));
        }
        self.layer_mut(layer).insert(parsed, command);
        Ok(())
    }

    fn layers(&self) -> [(KeyLayer, &HashMap<KeyChord, Command>); 6] {
        KeyLayer::ALL.map(|layer| (layer, self.layer(layer)))
    }

    const fn layer(&self, layer: KeyLayer) -> &HashMap<KeyChord, Command> {
        match layer {
            KeyLayer::General => &self.general,
            KeyLayer::TextEdit => &self.text_edit,
            KeyLayer::ListNavigation => &self.list_navigation,
            KeyLayer::Modal => &self.modal,
            KeyLayer::RepoSelect => &self.repo_select,
            KeyLayer::BranchSelect => &self.branch_select,
        }
    }

    /// Parses a `[keys]` section's contents for `TEST_PRODUCT`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_toml(contents: &str) -> Result<Self, String> {
        let raw = toml::from_str(contents).map_err(|error| error.to_string())?;
        Self::from_raw(raw, &crate::product::TEST_PRODUCT)
    }

    fn layer_mut(&mut self, layer: KeyLayer) -> &mut HashMap<KeyChord, Command> {
        match layer {
            KeyLayer::General => &mut self.general,
            KeyLayer::TextEdit => &mut self.text_edit,
            KeyLayer::ListNavigation => &mut self.list_navigation,
            KeyLayer::Modal => &mut self.modal,
            KeyLayer::RepoSelect => &mut self.repo_select,
            KeyLayer::BranchSelect => &mut self.branch_select,
        }
    }

    /// The chords the text-editing bindings act on.
    pub fn text_edit_chords(&self) -> impl Iterator<Item = KeyChord> + '_ {
        self.text_edit
            .iter()
            .filter(|(_, command)| **command != Command::Noop)
            .map(|(chord, _)| *chord)
    }

    /// The edit `chord` makes in a text field, by the text-editing bindings alone.
    pub fn text_edit(&self, chord: KeyChord) -> Option<TextEdit> {
        self.text_edit
            .get(&chord)
            .copied()
            .and_then(Command::text_edit)
    }

    pub fn command_for(&self, mode: BindingMode, chord: KeyChord) -> Option<Command> {
        let mut command = self.general.get(&chord).copied();
        if matches!(
            mode,
            BindingMode::Repo | BindingMode::Branch | BindingMode::BaseBranch
        ) {
            command = self.text_edit.get(&chord).copied().or(command);
        }
        if !matches!(mode, BindingMode::Modal) {
            command = self.list_navigation.get(&chord).copied().or(command);
        }
        command = match mode {
            BindingMode::Repo => self.repo_select.get(&chord).copied().or(command),
            BindingMode::Branch => self.branch_select.get(&chord).copied().or(command),
            BindingMode::BaseBranch | BindingMode::Choice | BindingMode::Modal => {
                self.modal.get(&chord).copied().or(command)
            }
        };
        command
    }

    /// Every chord bound to a command in any layer, so a frontend can claim them before
    /// its host's own shortcuts do.
    pub fn all_chords(&self) -> Vec<KeyChord> {
        let mut chords = self
            .layers()
            .into_iter()
            .flat_map(|(_, bindings)| bindings)
            .filter(|(_, command)| **command != Command::Noop)
            .map(|(chord, _)| *chord)
            .collect::<Vec<_>>();
        chords.sort_by_key(ToString::to_string);
        chords.dedup();
        chords
    }

    pub fn is_dismiss_toast(&self, chord: KeyChord) -> bool {
        self.general.get(&chord) == Some(&Command::DismissToast)
    }

    pub fn dismiss_toast_key(&self) -> Option<KeyChord> {
        self.general_key(Command::DismissToast)
    }

    pub fn scan_warnings_key(&self) -> Option<KeyChord> {
        self.general_key(Command::ScanWarnings)
    }

    fn general_key(&self, wanted: Command) -> Option<KeyChord> {
        self.general
            .iter()
            .filter_map(|(key, command)| (*command == wanted).then_some(*key))
            .min_by_key(ToString::to_string)
    }

    pub fn mode_for(mode: &Mode) -> BindingMode {
        match mode {
            Mode::RepoSelect => BindingMode::Repo,
            Mode::BranchSelect(_) => BindingMode::Branch,
            Mode::SelectBaseBranch { .. } => BindingMode::BaseBranch,
            Mode::ConfirmRemoteBranch { .. } => BindingMode::Choice,
            Mode::ValidatingNewBranch { .. }
            | Mode::ConfirmWorktreeDelete(_)
            | Mode::Loading { .. } => BindingMode::Modal,
        }
    }

    pub fn first_key(&self, mode: BindingMode, wanted: Command) -> Option<KeyChord> {
        self.sections(mode)
            .into_iter()
            .flat_map(|section| section.bindings)
            .find_map(|(key, command)| (command == wanted).then_some(key))
    }

    /// The key to show for `wanted`: like `first_key`, but favouring unmodified named keys
    /// such as arrows or `esc` over chords like `ctrl+p`, which sort first but are the less
    /// obvious binding to show; then function keys, which laptops often hide behind `fn`;
    /// then plain characters, which a search types instead while it has text.
    pub fn preferred_key(&self, mode: BindingMode, wanted: Command) -> Option<KeyChord> {
        self.sections(mode)
            .into_iter()
            .flat_map(|section| section.bindings)
            .filter_map(|(key, command)| (command == wanted).then_some(key))
            .enumerate()
            .min_by_key(|(order, key)| (key_preference(key), *order))
            .map(|(_, key)| key)
    }

    pub fn sections(&self, mode: BindingMode) -> Vec<HelpSection> {
        let effective = |key| self.command_for(mode, key);
        let mut sections = Vec::new();
        let layers: Vec<(&'static str, &HashMap<KeyChord, Command>)> = match mode {
            BindingMode::Repo => vec![
                ("repository", &self.repo_select),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("general", &self.general),
            ],
            BindingMode::Branch => vec![
                ("branches", &self.branch_select),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("general", &self.general),
            ],
            BindingMode::BaseBranch => vec![
                ("base branch", &self.modal),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("general", &self.general),
            ],
            BindingMode::Choice => vec![
                ("dialog", &self.modal),
                ("navigation", &self.list_navigation),
                ("general", &self.general),
            ],
            BindingMode::Modal => vec![("dialog", &self.modal), ("general", &self.general)],
        };
        for (name, layer) in layers {
            let mut bindings = layer
                .iter()
                .filter_map(|(key, command)| {
                    (*command != Command::Noop && effective(*key) == Some(*command))
                        .then_some((*key, *command))
                })
                .collect::<Vec<_>>();
            bindings.sort_by(|left, right| {
                command_rank(left.1)
                    .cmp(&command_rank(right.1))
                    .then_with(|| left.0.to_string().cmp(&right.0.to_string()))
            });
            if !bindings.is_empty() {
                sections.push(HelpSection { name, bindings });
            }
        }
        sections
    }
}

/// How readily a key is offered for its command, lowest first (see
/// [`KeysConfig::preferred_key`]).
pub fn key_preference(key: &KeyChord) -> u8 {
    match (key.modifiers == KeyModifiers::NONE, key.code) {
        (true, KeyCode::Char(_)) => 3,
        (_, KeyCode::F(_)) => 2,
        (true, _) => 0,
        (false, _) => 1,
    }
}

const GENERAL: &[Command] = &[
    Command::Noop,
    Command::Quit,
    Command::Help,
    Command::DismissToast,
    Command::ScanWarnings,
];
const TEXT_EDIT: &[Command] = &[
    Command::Noop,
    Command::Clear,
    Command::Backspace,
    Command::DeleteForward,
    Command::DeleteWord,
    Command::DeleteWordForward,
    Command::DeleteToEnd,
    Command::CursorLeft,
    Command::CursorRight,
    Command::CursorWordLeft,
    Command::CursorWordRight,
    Command::CursorStart,
    Command::CursorEnd,
];
const LIST_NAVIGATION: &[Command] = &[Command::Noop, Command::MoveUp, Command::MoveDown];
const MODAL: &[Command] = &[Command::Noop, Command::Open, Command::Back];
const REPO_SELECT: &[Command] = &[
    Command::Noop,
    Command::Open,
    Command::BranchesView,
    Command::Quit,
];
const BRANCH_SELECT: &[Command] = &[
    Command::Noop,
    Command::Open,
    Command::Back,
    Command::NewBranch,
    Command::Delete,
    Command::Backspace,
    Command::BackspaceOrBack,
];

const fn command_rank(command: Command) -> u8 {
    match command {
        Command::Open => 0,
        Command::BranchesView => 1,
        Command::NewBranch => 2,
        Command::Delete => 3,
        Command::Back => 4,
        Command::MoveUp => 5,
        Command::MoveDown => 6,
        Command::Clear => 7,
        Command::Backspace => 8,
        Command::BackspaceOrBack => 9,
        Command::DeleteForward => 10,
        Command::DeleteWord => 11,
        Command::DeleteWordForward => 12,
        Command::DeleteToEnd => 13,
        Command::CursorLeft => 14,
        Command::CursorRight => 15,
        Command::CursorWordLeft => 16,
        Command::CursorWordRight => 17,
        Command::CursorStart => 18,
        Command::CursorEnd => 19,
        Command::Help => 20,
        Command::DismissToast => 21,
        Command::ScanWarnings => 22,
        Command::Quit => 23,
        Command::Noop => 24,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_key_favours_unmodified_bindings_but_follows_remaps() {
        let keys = KeysConfig::default();
        assert_eq!(
            keys.preferred_key(BindingMode::Choice, Command::MoveUp),
            Some(KeyChord::new(KeyCode::Up, KeyModifiers::NONE))
        );
        let keys =
            KeysConfig::from_toml("[list_navigation]\nup = \"noop\"\ndown = \"noop\"").unwrap();
        assert_eq!(
            keys.preferred_key(BindingMode::Choice, Command::MoveUp),
            Some(KeyChord::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn preferred_key_shows_a_chord_before_a_letter_a_search_would_type() {
        let keys = KeysConfig::from_toml("[general]\nq = \"quit\"").unwrap();
        assert_eq!(
            keys.preferred_key(BindingMode::Branch, Command::Quit),
            Some(KeyChord::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            keys.preferred_key(BindingMode::Repo, Command::Quit),
            Some(KeyChord::new(KeyCode::Esc, KeyModifiers::NONE))
        );
        let keys = KeysConfig::from_toml("[general]\nq = \"quit\"\n\"C-c\" = \"noop\"").unwrap();
        assert_eq!(
            keys.preferred_key(BindingMode::Branch, Command::Quit),
            Some(KeyChord::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            "a letter is still shown when it is the only way"
        );
    }

    #[test]
    fn parses_and_displays_valid_chords() {
        assert_eq!("C-b".parse::<KeyChord>().unwrap().to_string(), "ctrl+b");
        assert_eq!(
            "Alt-backspace".parse::<KeyChord>().unwrap().to_string(),
            "alt+backspace"
        );
        assert_eq!(
            "shift-tab".parse::<KeyChord>().unwrap().to_string(),
            "shift+tab"
        );
        assert_eq!("space".parse::<KeyChord>().unwrap().to_string(), "space");
        let shifted = "S-a".parse::<KeyChord>().unwrap();
        assert_eq!(shifted.to_string(), "shift+a");
        assert_eq!(
            shifted,
            KeyChord::from_event(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT))
        );
        assert_eq!(shifted.to_string().parse::<KeyChord>().unwrap(), shifted);
        assert_eq!("A".parse::<KeyChord>().unwrap(), "a".parse().unwrap());
    }

    #[test]
    fn rejects_invalid_chords_and_unknown_actions() {
        let bad_chord =
            KeysConfig::from_toml("[branch_select]\n\"Hyper-Nope\" = \"new_branch\"").unwrap_err();
        assert!(bad_chord.contains("invalid key chord"));
        let bad_action =
            KeysConfig::from_toml("[branch_select]\n\"C-b\" = \"launch_missiles\"").unwrap_err();
        assert!(bad_action.contains("unknown key action"));
        let shifted_number =
            KeysConfig::from_toml("[repo_select]\n\"S-1\" = \"branches_view\"").unwrap_err();
        assert!(shifted_number.contains("bind the resulting character directly"));
    }

    const BACKSPACE_OR_BACK_PRODUCT: Product = Product {
        keys: &[(
            KeyLayer::BranchSelect,
            "backspace",
            Command::BackspaceOrBack,
        )],
        ..crate::product::TEST_PRODUCT
    };

    #[test]
    fn product_keys_sit_over_the_defaults_and_under_the_users() {
        let backspace = "backspace".parse().unwrap();
        let product = KeysConfig::for_product(&BACKSPACE_OR_BACK_PRODUCT).unwrap();
        assert_eq!(
            product.command_for(BindingMode::Branch, backspace),
            Some(Command::BackspaceOrBack)
        );
        assert_eq!(
            product.command_for(BindingMode::Repo, backspace),
            Some(Command::Backspace)
        );
        let raw = toml::from_str("[branch_select]\nbackspace = \"backspace\"").unwrap();
        let user = KeysConfig::from_raw(raw, &BACKSPACE_OR_BACK_PRODUCT).unwrap();
        assert_eq!(
            user.command_for(BindingMode::Branch, backspace),
            Some(Command::Backspace)
        );
    }

    #[test]
    fn a_product_key_must_be_allowed_where_it_is_bound() {
        let product = Product {
            keys: &[(KeyLayer::General, "backspace", Command::BackspaceOrBack)],
            ..crate::product::TEST_PRODUCT
        };
        let error = KeysConfig::for_product(&product).unwrap_err();
        assert!(error.contains("kiosk-test's default keys"), "{error}");
        assert!(error.contains("not allowed in [keys.general]"), "{error}");
    }

    #[test]
    fn user_keys_merge_with_defaults_and_override_matching_chords() {
        let keys =
            KeysConfig::from_toml("[branch_select]\n\"C-b\" = \"new_branch\"\n\"C-o\" = \"noop\"")
                .unwrap();
        assert_eq!(
            keys.command_for(BindingMode::Branch, "C-b".parse().unwrap()),
            Some(Command::NewBranch)
        );
        assert_eq!(
            keys.command_for(BindingMode::Branch, "enter".parse().unwrap()),
            Some(Command::Open)
        );
        assert_eq!(
            keys.command_for(BindingMode::Branch, "C-o".parse().unwrap()),
            Some(Command::Noop)
        );
    }

    #[test]
    fn help_rows_are_derived_from_overridden_bindings() {
        let keys =
            KeysConfig::from_toml("[branch_select]\n\"C-b\" = \"new_branch\"\n\"C-o\" = \"noop\"")
                .unwrap();
        let rendered = keys
            .sections(BindingMode::Branch)
            .into_iter()
            .flat_map(|section| section.bindings)
            .map(|(key, command)| format!("{key} {command:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("ctrl+b NewBranch"));
        assert!(!rendered.contains("ctrl+o NewBranch"));
    }

    #[test]
    fn base_branch_mode_layers_search_navigation_and_dialog_bindings() {
        let keys = KeysConfig::default();
        for (chord, command) in [
            ("up", Command::MoveUp),
            ("down", Command::MoveDown),
            ("C-p", Command::MoveUp),
            ("C-n", Command::MoveDown),
            ("backspace", Command::Backspace),
            ("C-w", Command::DeleteWord),
            ("left", Command::CursorLeft),
            ("right", Command::CursorRight),
            ("enter", Command::Open),
            ("esc", Command::Back),
        ] {
            assert_eq!(
                keys.command_for(BindingMode::BaseBranch, chord.parse().unwrap()),
                Some(command),
                "unexpected base-branch command for {chord}"
            );
        }
    }

    #[test]
    fn delete_dialog_remains_modal() {
        let keys = KeysConfig::default();
        assert_eq!(
            keys.command_for(BindingMode::Modal, "backspace".parse().unwrap()),
            None
        );
        assert_eq!(
            keys.command_for(BindingMode::Modal, "down".parse().unwrap()),
            None
        );
    }
}
