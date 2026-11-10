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
    tabs::{TabCommand, TabId},
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
    NextTab,
    PreviousTab,
    ShowTab(TabId),
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
    PageDown,
    PageUp,
    ScrollTop,
    ScrollBottom,
    NextFile,
    PreviousFile,
    NextRange,
    PreviousRange,
    ToggleFile,
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
            Self::NextTab => "next_tab",
            Self::PreviousTab => "previous_tab",
            Self::ShowTab(tab) => tab.command_name(),
            Self::ScrollDown => "scroll_down",
            Self::ScrollUp => "scroll_up",
            Self::ScrollLeft => "scroll_left",
            Self::ScrollRight => "scroll_right",
            Self::PageDown => "page_down",
            Self::PageUp => "page_up",
            Self::ScrollTop => "scroll_top",
            Self::ScrollBottom => "scroll_bottom",
            Self::NextFile => "next_file",
            Self::PreviousFile => "previous_file",
            Self::NextRange => "next_range",
            Self::PreviousRange => "previous_range",
            Self::ToggleFile => "toggle_file",
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
            Self::NextTab => "Show the next tab",
            Self::PreviousTab => "Show the previous tab",
            Self::ShowTab(tab) => tab.command_description(),
            Self::ScrollDown => "Scroll down a line",
            Self::ScrollUp => "Scroll up a line",
            Self::ScrollLeft => "Scroll long lines left",
            Self::ScrollRight => "Scroll long lines right",
            Self::PageDown => "Scroll down a screen",
            Self::PageUp => "Scroll up a screen",
            Self::ScrollTop => "Scroll to the top",
            Self::ScrollBottom => "Scroll to the bottom",
            Self::NextFile => "Show the next file",
            Self::PreviousFile => "Show the previous file, or the top of this one",
            Self::NextRange => "Show the next range of changes",
            Self::PreviousRange => "Show the previous range of changes",
            Self::ToggleFile => "Collapse or expand the file at the top",
        }
    }

    /// The edit a text-editing command makes, or `None` for any other command. Where a
    /// key can only edit text, as in help's search, `backspace_or_back` only deletes.
    pub const fn text_edit(self) -> Option<TextEdit> {
        Some(match self {
            Self::Clear => TextEdit::Clear,
            Self::Backspace | Self::BackspaceOrBack => TextEdit::Backspace,
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
            | Self::NewBranch
            | Self::Delete
            | Self::NextTab
            | Self::PreviousTab
            | Self::ShowTab(_)
            | Self::ScrollDown
            | Self::ScrollUp
            | Self::ScrollLeft
            | Self::ScrollRight
            | Self::PageDown
            | Self::PageUp
            | Self::ScrollTop
            | Self::ScrollBottom
            | Self::NextFile
            | Self::PreviousFile
            | Self::NextRange
            | Self::PreviousRange
            | Self::ToggleFile => return None,
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
            "next_tab" => Ok(Self::NextTab),
            "scroll_down" => Ok(Self::ScrollDown),
            "scroll_up" => Ok(Self::ScrollUp),
            "scroll_left" => Ok(Self::ScrollLeft),
            "scroll_right" => Ok(Self::ScrollRight),
            "page_down" => Ok(Self::PageDown),
            "page_up" => Ok(Self::PageUp),
            "scroll_top" => Ok(Self::ScrollTop),
            "scroll_bottom" => Ok(Self::ScrollBottom),
            "next_file" => Ok(Self::NextFile),
            "previous_file" => Ok(Self::PreviousFile),
            "next_range" => Ok(Self::NextRange),
            "previous_range" => Ok(Self::PreviousRange),
            "toggle_file" => Ok(Self::ToggleFile),
            "previous_tab" => Ok(Self::PreviousTab),
            _ => TabId::from_command_name(value)
                .map(Self::ShowTab)
                .ok_or_else(|| format!("unknown key action '{value}'")),
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
    /// Bindings that change tab, active in every tab and ahead of every other section.
    /// Accepted actions are `next_tab`, `previous_tab`, one `<tab>_tab` per tab, such as
    /// `diff_tab`, and `noop`.
    tabs: HashMap<KeyChord, Command>,
    /// Bindings specific to the diff tab. Accepted actions are `quit`, `noop` and the
    /// diff tab's own, such as `scroll_down`, `next_file` and `toggle_file`.
    diff: HashMap<KeyChord, Command>,
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
    tabs: HashMap<String, String>,
    diff: HashMap<String, String>,
}

impl RawKeysConfig {
    fn into_layers(self) -> [(KeyLayer, HashMap<String, String>); KeyLayer::ALL.len()] {
        [
            (KeyLayer::General, self.general),
            (KeyLayer::TextEdit, self.text_edit),
            (KeyLayer::ListNavigation, self.list_navigation),
            (KeyLayer::Modal, self.modal),
            (KeyLayer::RepoSelect, self.repo_select),
            (KeyLayer::BranchSelect, self.branch_select),
            (KeyLayer::Tabs, self.tabs),
            (KeyLayer::Diff, self.diff),
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
    Tabs,
    Diff,
}

impl KeyLayer {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::TextEdit,
        Self::ListNavigation,
        Self::Modal,
        Self::RepoSelect,
        Self::BranchSelect,
        Self::Tabs,
        Self::Diff,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::TextEdit => "text_edit",
            Self::ListNavigation => "list_navigation",
            Self::Modal => "modal",
            Self::RepoSelect => "repo_select",
            Self::BranchSelect => "branch_select",
            Self::Tabs => "tabs",
            Self::Diff => "diff",
        }
    }

    /// Whether the README documents this section. The tab sections stay out of it while
    /// tabs are behind `experimental_tabs`.
    pub const fn documented(self) -> bool {
        !matches!(self, Self::Tabs | Self::Diff)
    }

    /// The commands a binding in this section may name.
    pub fn allowed(self) -> &'static [Command] {
        match self {
            Self::General => GENERAL,
            Self::TextEdit => TEXT_EDIT,
            Self::ListNavigation => LIST_NAVIGATION,
            Self::Modal => MODAL,
            Self::RepoSelect => REPO_SELECT,
            Self::BranchSelect => BRANCH_SELECT,
            Self::Tabs => &TABS,
            Self::Diff => DIFF,
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
            tabs: default_tab_keys(),
            diff: map(&[
                ("esc", Command::Quit),
                ("j", Command::ScrollDown),
                ("down", Command::ScrollDown),
                ("k", Command::ScrollUp),
                ("up", Command::ScrollUp),
                ("h", Command::ScrollLeft),
                ("left", Command::ScrollLeft),
                ("l", Command::ScrollRight),
                ("right", Command::ScrollRight),
                ("C-d", Command::PageDown),
                ("pagedown", Command::PageDown),
                ("space", Command::PageDown),
                ("C-u", Command::PageUp),
                ("pageup", Command::PageUp),
                ("g", Command::ScrollTop),
                ("home", Command::ScrollTop),
                ("S-g", Command::ScrollBottom),
                ("end", Command::ScrollBottom),
                ("n", Command::NextFile),
                ("p", Command::PreviousFile),
                ("]", Command::NextRange),
                ("[", Command::PreviousRange),
                ("e", Command::ToggleFile),
            ]),
        }
    }
}

/// Moving between tabs, then `ctrl+<n>` for the tab at position `n` of the tab bar.
fn default_tab_keys() -> HashMap<KeyChord, Command> {
    let mut keys = map(&[
        ("C-pagedown", Command::NextTab),
        ("C-tab", Command::NextTab),
        ("C-pageup", Command::PreviousTab),
        ("C-S-tab", Command::PreviousTab),
    ]);
    for (position, tab) in TabId::ALL.iter().enumerate().take(9) {
        let chord = format!("C-{}", position + 1)
            .parse()
            .expect("valid default chord");
        keys.insert(chord, Command::ShowTab(*tab));
    }
    keys
}

fn map(entries: &[(&str, Command)]) -> HashMap<KeyChord, Command> {
    entries
        .iter()
        .map(|(key, command)| (key.parse().expect("valid default chord"), *command))
        .collect()
}

/// `keys`' bindings in the documented sections as a TOML block for the config reference.
pub fn default_keys_toml(keys: &KeysConfig) -> String {
    let mut output = String::from("```toml\n");
    for (layer, bindings) in keys
        .layers()
        .into_iter()
        .filter(|(layer, _)| layer.documented())
    {
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
            parse_chord(chord)
                .and_then(|parsed| keys.bind(layer, parsed, command))
                .map_err(|error| format!("{}'s default keys: {error}", product.name))?;
        }
        for (layer, bindings) in raw.into_layers() {
            // Spellings of one chord, such as `C-b` and `ctrl+b`, would otherwise race in
            // the map's arbitrary order.
            let mut written: HashMap<KeyChord, (String, Command)> = HashMap::new();
            for (chord, action) in bindings {
                let parsed = parse_chord(&chord)?;
                let command = action.parse()?;
                if let Some((earlier, earlier_command)) =
                    written.insert(parsed, (chord.clone(), command))
                    && earlier_command != command
                {
                    return Err(format!(
                        "'{earlier}' and '{chord}' in [keys.{}] are the same key bound to different actions",
                        layer.name()
                    ));
                }
                keys.bind(layer, parsed, command)?;
            }
        }
        Ok(keys)
    }

    fn bind(&mut self, layer: KeyLayer, parsed: KeyChord, command: Command) -> Result<(), String> {
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

    fn layers(&self) -> [(KeyLayer, &HashMap<KeyChord, Command>); KeyLayer::ALL.len()] {
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
            KeyLayer::Tabs => &self.tabs,
            KeyLayer::Diff => &self.diff,
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
            KeyLayer::Tabs => &mut self.tabs,
            KeyLayer::Diff => &mut self.diff,
        }
    }

    /// These bindings with no tab keys, for when kiosk shows only the picker, so the keys
    /// stay the app's.
    #[must_use]
    pub fn without_tabs(mut self) -> Self {
        self.tabs.clear();
        self
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
        self.tab_binding(chord).or(command)
    }

    /// The tab-changing command bound to `chord`. It wins over every other section, in
    /// every tab, and `noop` there only unbinds the tab key.
    fn tab_binding(&self, chord: KeyChord) -> Option<Command> {
        self.tabs
            .get(&chord)
            .copied()
            .filter(|command| *command != Command::Noop)
    }

    /// The change of tab `chord` asks for, if any.
    pub fn tab_command(&self, chord: KeyChord) -> Option<TabCommand> {
        match self.tab_binding(chord)? {
            Command::NextTab => Some(TabCommand::Next),
            Command::PreviousTab => Some(TabCommand::Previous),
            Command::ShowTab(tab) => Some(TabCommand::Show(tab)),
            command => unreachable!("[keys.tabs] does not allow '{}'", command.name()),
        }
    }

    /// What `chord` does in the diff tab, short of changing tab.
    pub fn diff_command(&self, chord: KeyChord) -> Option<Command> {
        self.diff
            .get(&chord)
            .or_else(|| self.general.get(&chord))
            .copied()
    }

    /// The key to show for `wanted` in the diff tab: one that runs it there, rather than a
    /// tab key or a diff key taking it first, preferring keys as [`Self::preferred_key`] does.
    pub fn diff_key(&self, wanted: Command) -> Option<KeyChord> {
        self.diff
            .iter()
            .chain(&self.general)
            .filter(|(chord, command)| {
                **command == wanted
                    && self.tab_binding(**chord).is_none()
                    && self.diff_command(**chord) == Some(wanted)
            })
            .map(|(chord, _)| *chord)
            .min_by_key(|chord| (key_preference(chord), chord.to_string()))
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
        self.layer_key(KeyLayer::General, wanted)
    }

    /// The key bound to `wanted` in `layer`, the first by name if there are several.
    pub fn layer_key(&self, layer: KeyLayer, wanted: Command) -> Option<KeyChord> {
        self.layer(layer)
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
                ("tabs", &self.tabs),
                ("general", &self.general),
            ],
            BindingMode::Branch => vec![
                ("branches", &self.branch_select),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("tabs", &self.tabs),
                ("general", &self.general),
            ],
            BindingMode::BaseBranch => vec![
                ("base branch", &self.modal),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("tabs", &self.tabs),
                ("general", &self.general),
            ],
            BindingMode::Choice => vec![
                ("dialog", &self.modal),
                ("navigation", &self.list_navigation),
                ("tabs", &self.tabs),
                ("general", &self.general),
            ],
            BindingMode::Modal => vec![
                ("dialog", &self.modal),
                ("tabs", &self.tabs),
                ("general", &self.general),
            ],
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
static TABS: std::sync::LazyLock<Vec<Command>> = std::sync::LazyLock::new(|| {
    [Command::Noop, Command::NextTab, Command::PreviousTab]
        .into_iter()
        .chain(TabId::ALL.iter().copied().map(Command::ShowTab))
        .collect()
});
const DIFF: &[Command] = &[
    Command::Noop,
    Command::Quit,
    Command::ScrollDown,
    Command::ScrollUp,
    Command::ScrollLeft,
    Command::ScrollRight,
    Command::PageDown,
    Command::PageUp,
    Command::ScrollTop,
    Command::ScrollBottom,
    Command::NextFile,
    Command::PreviousFile,
    Command::NextRange,
    Command::PreviousRange,
    Command::ToggleFile,
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

fn parse_chord(chord: &str) -> Result<KeyChord, String> {
    chord
        .parse()
        .map_err(|error| format!("invalid key chord '{chord}': {error}"))
}

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
        Command::ScrollDown
        | Command::ScrollUp
        | Command::ScrollLeft
        | Command::ScrollRight
        | Command::PageDown
        | Command::PageUp
        | Command::ScrollTop
        | Command::ScrollBottom
        | Command::NextFile
        | Command::PreviousFile
        | Command::NextRange
        | Command::PreviousRange
        | Command::ToggleFile
        | Command::NextTab => 20,
        Command::PreviousTab => 21,
        Command::ShowTab(_) => 22,
        Command::Help => 23,
        Command::DismissToast => 24,
        Command::ScanWarnings => 25,
        Command::Quit => 26,
        Command::Noop => 27,
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
    fn two_spellings_of_a_key_may_not_disagree_within_a_section() {
        let error =
            KeysConfig::from_toml("[branch_select]\n\"C-b\" = \"open\"\n\"ctrl+b\" = \"back\"")
                .unwrap_err();
        assert!(
            error.contains("same key bound to different actions"),
            "{error}"
        );
        assert!(error.contains("[keys.branch_select]"), "{error}");
        let agreeing =
            KeysConfig::from_toml("[branch_select]\n\"C-b\" = \"open\"\n\"ctrl+b\" = \"open\"")
                .unwrap();
        assert_eq!(
            agreeing.command_for(BindingMode::Branch, "C-b".parse().unwrap()),
            Some(Command::Open)
        );
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

    #[test]
    fn tab_keys_move_between_tabs_and_show_each_one() {
        let keys = KeysConfig::default();
        let tab = |chord: &str| keys.tab_command(chord.parse().unwrap());
        assert_eq!(tab("ctrl+pagedown"), Some(TabCommand::Next));
        assert_eq!(tab("ctrl+tab"), Some(TabCommand::Next));
        assert_eq!(tab("ctrl+pageup"), Some(TabCommand::Previous));
        assert_eq!(tab("ctrl+shift+tab"), Some(TabCommand::Previous));
        for (position, id) in TabId::ALL.iter().enumerate() {
            assert_eq!(
                tab(&format!("ctrl+{}", position + 1)),
                Some(TabCommand::Show(*id))
            );
        }
        assert_eq!(tab("tab"), None);
    }

    #[test]
    fn tab_keys_win_over_every_other_section() {
        let keys = KeysConfig::from_toml(
            "[repo_select]\n\"ctrl+1\" = \"branches_view\"\n[tabs]\n\"ctrl+o\" = \"diff_tab\"\n",
        )
        .unwrap();
        assert_eq!(
            keys.command_for(BindingMode::Repo, "ctrl+1".parse().unwrap()),
            Some(Command::ShowTab(TabId::Picker))
        );
        assert_eq!(
            keys.command_for(BindingMode::Branch, "ctrl+o".parse().unwrap()),
            Some(Command::ShowTab(TabId::Diff))
        );
    }

    #[test]
    fn unbinding_a_tab_key_leaves_the_key_to_the_other_sections() {
        let keys = KeysConfig::from_toml(
            "[tabs]\n\"ctrl+tab\" = \"noop\"\n[repo_select]\n\"ctrl+tab\" = \"branches_view\"\n",
        )
        .unwrap();
        let chord = "ctrl+tab".parse().unwrap();
        assert_eq!(keys.tab_command(chord), None);
        assert_eq!(
            keys.command_for(BindingMode::Repo, chord),
            Some(Command::BranchesView)
        );
    }

    #[test]
    fn tab_actions_are_named_after_their_tabs_and_allowed_only_in_tabs() {
        assert_eq!(
            "diff_tab".parse::<Command>(),
            Ok(Command::ShowTab(TabId::Diff))
        );
        assert_eq!(Command::ShowTab(TabId::Picker).name(), "picker_tab");
        assert!(
            KeysConfig::from_toml("[general]\n\"ctrl+g\" = \"diff_tab\"\n")
                .unwrap_err()
                .contains("not allowed in [keys.general]")
        );
        assert!(
            KeysConfig::from_toml("[tabs]\n\"ctrl+g\" = \"logs_tab\"\n")
                .unwrap_err()
                .contains("unknown key action 'logs_tab'")
        );
    }

    #[test]
    fn the_diff_tab_takes_its_own_keys_over_the_general_ones() {
        let keys = KeysConfig::from_toml("[diff]\n\"ctrl+c\" = \"noop\"\n").unwrap();
        assert_eq!(
            keys.diff_command("esc".parse().unwrap()),
            Some(Command::Quit)
        );
        assert_eq!(
            keys.diff_command("ctrl+c".parse().unwrap()),
            Some(Command::Noop)
        );
        assert_eq!(
            keys.diff_command("ctrl+h".parse().unwrap()),
            Some(Command::Help)
        );
    }

    #[test]
    fn the_diff_tab_shows_a_quit_key_that_quits_there() {
        let default = KeysConfig::default();
        assert_eq!(
            default.diff_key(Command::Quit),
            Some("esc".parse().unwrap())
        );
        let taken = KeysConfig::from_toml("[tabs]\n\"esc\" = \"picker_tab\"\n").unwrap();
        assert_eq!(
            taken.diff_key(Command::Quit),
            Some("ctrl+c".parse().unwrap())
        );
        let none = KeysConfig::from_toml(
            "[tabs]\n\"esc\" = \"picker_tab\"\n[general]\n\"ctrl+c\" = \"noop\"\n",
        )
        .unwrap();
        assert_eq!(none.diff_key(Command::Quit), None);
    }

    #[test]
    fn help_lists_the_tab_keys_in_every_mode() {
        let keys = KeysConfig::default();
        for mode in [
            BindingMode::Repo,
            BindingMode::Branch,
            BindingMode::BaseBranch,
            BindingMode::Choice,
            BindingMode::Modal,
        ] {
            let tabs = keys
                .sections(mode)
                .into_iter()
                .find(|section| section.name == "tabs")
                .unwrap_or_else(|| panic!("no tabs section in {mode:?}"));
            assert!(
                tabs.bindings
                    .contains(&("ctrl+2".parse().unwrap(), Command::ShowTab(TabId::Diff))),
                "{mode:?}: {tabs:?}"
            );
        }
    }
}
