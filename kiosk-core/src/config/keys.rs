use crate::keyboard::{KeyCode, KeyEvent, KeyModifiers};
use crate::state::Mode;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

/// Commands that can be bound to keys
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    /// No-op: explicitly unbinds a key (removes inherited/default binding)
    Noop,

    // General commands
    Quit,
    ShowHelp,

    // Navigation commands
    OpenRepo,
    EnterRepo,
    OpenBranch,
    GoBack,
    NewBranch,
    DeleteWorktree,

    // List movement commands
    MoveUp,
    MoveDown,
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    MoveTop,
    MoveBottom,

    // Text-edit commands
    DeleteBackwardChar,
    DeleteBackwardWord,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorStart,
    MoveCursorEnd,

    // Generic confirm/cancel commands
    Confirm,
    Cancel,
}

impl FromStr for Command {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "noop" | "none" | "unbound" => Ok(Command::Noop),
            "quit" => Ok(Command::Quit),
            "show_help" => Ok(Command::ShowHelp),
            "open_repo" => Ok(Command::OpenRepo),
            "enter_repo" => Ok(Command::EnterRepo),
            "open_branch" => Ok(Command::OpenBranch),
            "go_back" => Ok(Command::GoBack),
            "new_branch" => Ok(Command::NewBranch),
            "delete_worktree" => Ok(Command::DeleteWorktree),
            "move_up" => Ok(Command::MoveUp),
            "move_down" => Ok(Command::MoveDown),
            "half_page_up" => Ok(Command::HalfPageUp),
            "half_page_down" => Ok(Command::HalfPageDown),
            "page_up" => Ok(Command::PageUp),
            "page_down" => Ok(Command::PageDown),
            "move_top" => Ok(Command::MoveTop),
            "move_bottom" => Ok(Command::MoveBottom),
            "delete_backward_char" => Ok(Command::DeleteBackwardChar),
            "delete_backward_word" => Ok(Command::DeleteBackwardWord),
            "move_cursor_left" => Ok(Command::MoveCursorLeft),
            "move_cursor_right" => Ok(Command::MoveCursorRight),
            "move_cursor_start" => Ok(Command::MoveCursorStart),
            "move_cursor_end" => Ok(Command::MoveCursorEnd),
            "confirm" => Ok(Command::Confirm),
            "cancel" => Ok(Command::Cancel),
            _ => Err(format!("Unknown command: {s}")),
        }
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Command::Noop => "noop",
            Command::Quit => "quit",
            Command::ShowHelp => "show_help",
            Command::OpenRepo => "open_repo",
            Command::EnterRepo => "enter_repo",
            Command::OpenBranch => "open_branch",
            Command::GoBack => "go_back",
            Command::NewBranch => "new_branch",
            Command::DeleteWorktree => "delete_worktree",
            Command::MoveUp => "move_up",
            Command::MoveDown => "move_down",
            Command::HalfPageUp => "half_page_up",
            Command::HalfPageDown => "half_page_down",
            Command::PageUp => "page_up",
            Command::PageDown => "page_down",
            Command::MoveTop => "move_top",
            Command::MoveBottom => "move_bottom",
            Command::DeleteBackwardChar => "delete_backward_char",
            Command::DeleteBackwardWord => "delete_backward_word",
            Command::MoveCursorLeft => "move_cursor_left",
            Command::MoveCursorRight => "move_cursor_right",
            Command::MoveCursorStart => "move_cursor_start",
            Command::MoveCursorEnd => "move_cursor_end",
            Command::Confirm => "confirm",
            Command::Cancel => "cancel",
        };
        write!(f, "{s}")
    }
}

impl Command {
    /// Get a human-readable description of the command for help display
    pub fn description(&self) -> &'static str {
        match self {
            Command::Noop => "Unbound",
            Command::Quit => "Quit the application",
            Command::ShowHelp => "Show help",
            Command::OpenRepo => "Open repository in tmux",
            Command::EnterRepo => "Browse branches",
            Command::OpenBranch => "Open branch in tmux",
            Command::GoBack => "Go back",
            Command::NewBranch => "New branch",
            Command::DeleteWorktree => "Delete worktree",
            Command::MoveUp => "Move up",
            Command::MoveDown => "Move down",
            Command::HalfPageUp => "Half page up",
            Command::HalfPageDown => "Half page down",
            Command::PageUp => "Page up",
            Command::PageDown => "Page down",
            Command::MoveTop => "Move to top",
            Command::MoveBottom => "Move to bottom",
            Command::DeleteBackwardChar => "Delete backward char",
            Command::DeleteBackwardWord => "Delete backward word",
            Command::MoveCursorLeft => "Move cursor left",
            Command::MoveCursorRight => "Move cursor right",
            Command::MoveCursorStart => "Move cursor to start",
            Command::MoveCursorEnd => "Move cursor to end",
            Command::Confirm => "Confirm",
            Command::Cancel => "Cancel",
        }
    }
}

/// Key bindings for a specific layer/mode
pub type KeyMap = HashMap<KeyEvent, Command>;

/// Complete key binding configuration, composed from reusable layers.
#[derive(Debug, Clone)]
pub struct KeysConfig {
    pub general: KeyMap,
    pub text_edit: KeyMap,
    pub list_navigation: KeyMap,
    pub confirm_cancel: KeyMap,
    pub repo_select: KeyMap,
    pub branch_select: KeyMap,
}

/// Intermediate structure for deserializing key bindings
#[derive(Debug, Deserialize)]
struct KeysConfigRaw {
    #[serde(default)]
    general: HashMap<String, String>,
    #[serde(default)]
    text_edit: HashMap<String, String>,
    #[serde(default)]
    list_navigation: HashMap<String, String>,
    #[serde(default)]
    confirm_cancel: HashMap<String, String>,
    #[serde(default)]
    repo_select: HashMap<String, String>,
    #[serde(default)]
    branch_select: HashMap<String, String>,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl KeysConfig {
    pub fn new() -> Self {
        Self {
            general: Self::default_general(),
            text_edit: Self::default_text_edit(),
            list_navigation: Self::default_list_navigation(),
            confirm_cancel: Self::default_confirm_cancel(),
            repo_select: Self::default_repo_select(),
            branch_select: Self::default_branch_select(),
        }
    }

    /// Build the effective keymap for a given app mode using precedence:
    /// general < shared layers < mode-specific
    pub fn keymap_for_mode(&self, mode: &Mode) -> KeyMap {
        let mut combined = KeyMap::new();
        Self::apply_layer(&mut combined, &self.general);

        match mode {
            Mode::RepoSelect => {
                Self::apply_layer(&mut combined, &self.text_edit);
                Self::apply_layer(&mut combined, &self.list_navigation);
                Self::apply_layer(&mut combined, &self.repo_select);
            }
            Mode::BranchSelect => {
                Self::apply_layer(&mut combined, &self.text_edit);
                Self::apply_layer(&mut combined, &self.list_navigation);
                Self::apply_layer(&mut combined, &self.branch_select);
            }
            Mode::NewBranchBase => {
                Self::apply_layer(&mut combined, &self.text_edit);
                Self::apply_layer(&mut combined, &self.list_navigation);
                Self::apply_layer(&mut combined, &self.confirm_cancel);
            }
            Mode::ConfirmDelete { .. } => {
                Self::apply_layer(&mut combined, &self.confirm_cancel);
            }
            Mode::Help { .. } | Mode::Loading(_) => {
                // general-only
            }
        }

        combined
    }

    /// Find the first key bound to a given command in a keymap.
    pub fn find_key(keymap: &KeyMap, command: &Command) -> Option<KeyEvent> {
        // Prefer shorter/simpler key representations
        let mut found: Vec<_> = keymap
            .iter()
            .filter(|(_, cmd)| *cmd == command)
            .map(|(key, _)| *key)
            .collect();
        found.sort();
        found.into_iter().next()
    }

    fn apply_layer(base: &mut KeyMap, layer: &KeyMap) {
        for (key, command) in layer {
            if *command == Command::Noop {
                base.remove(key);
            } else {
                base.insert(*key, command.clone());
            }
        }
    }

    fn default_general() -> KeyMap {
        let mut map = KeyMap::new();
        map.insert(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Command::Quit,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
            Command::ShowHelp,
        );
        map
    }

    fn default_text_edit() -> KeyMap {
        let mut map = KeyMap::new();
        map.insert(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            Command::DeleteBackwardChar,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            Command::DeleteBackwardWord,
        );
        map.insert(
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            Command::MoveCursorLeft,
        );
        map.insert(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            Command::MoveCursorRight,
        );
        map.insert(
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            Command::MoveCursorStart,
        );
        map.insert(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            Command::MoveCursorEnd,
        );
        map
    }

    fn default_list_navigation() -> KeyMap {
        let mut map = KeyMap::new();
        map.insert(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            Command::MoveUp,
        );
        map.insert(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            Command::MoveDown,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            Command::MoveUp,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            Command::MoveDown,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Command::HalfPageDown,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Command::HalfPageUp,
        );
        map.insert(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            Command::PageUp,
        );
        map.insert(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            Command::PageDown,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
            Command::MoveTop,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::ALT),
            Command::MoveBottom,
        );
        map
    }

    fn default_confirm_cancel() -> KeyMap {
        let mut map = KeyMap::new();
        map.insert(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Command::Confirm,
        );
        map.insert(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Command::Cancel,
        );
        map
    }

    fn default_repo_select() -> KeyMap {
        let mut map = KeyMap::new();
        map.insert(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Command::OpenRepo,
        );
        map.insert(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            Command::EnterRepo,
        );
        map.insert(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Command::Quit,
        );
        map
    }

    fn default_branch_select() -> KeyMap {
        let mut map = KeyMap::new();
        map.insert(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Command::OpenBranch,
        );
        map.insert(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Command::GoBack,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            Command::NewBranch,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            Command::DeleteWorktree,
        );
        map
    }

    /// Parse a string representation of keybindings into a `KeyMap`
    fn parse_keymap(raw_map: &HashMap<String, String>) -> Result<KeyMap, String> {
        let mut keymap = KeyMap::new();
        for (key_str, command_str) in raw_map {
            let key_event =
                KeyEvent::from_str(key_str).map_err(|e| format!("Invalid key '{key_str}': {e}"))?;
            let command = Command::from_str(command_str)
                .map_err(|e| format!("Invalid command '{command_str}': {e}"))?;
            keymap.insert(key_event, command);
        }
        Ok(keymap)
    }

    /// Merge user configuration with defaults.
    ///
    /// Keep `Noop` values so higher-precedence layers can explicitly unbind inherited mappings.
    fn from_raw(raw: &KeysConfigRaw) -> Result<Self, String> {
        let mut config = Self::default();

        config.general.extend(Self::parse_keymap(&raw.general)?);
        config.text_edit.extend(Self::parse_keymap(&raw.text_edit)?);
        config
            .list_navigation
            .extend(Self::parse_keymap(&raw.list_navigation)?);
        config
            .confirm_cancel
            .extend(Self::parse_keymap(&raw.confirm_cancel)?);
        config
            .repo_select
            .extend(Self::parse_keymap(&raw.repo_select)?);
        config
            .branch_select
            .extend(Self::parse_keymap(&raw.branch_select)?);

        Ok(config)
    }
}

// Custom deserializer for KeysConfig
impl<'de> Deserialize<'de> for KeysConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = KeysConfigRaw::deserialize(deserializer)?;
        KeysConfig::from_raw(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_from_str() {
        assert_eq!(Command::from_str("quit").unwrap(), Command::Quit);
        assert_eq!(
            Command::from_str("delete_backward_char").unwrap(),
            Command::DeleteBackwardChar
        );
        assert!(Command::from_str("invalid_command").is_err());
    }

    #[test]
    fn test_command_display() {
        assert_eq!(Command::Quit.to_string(), "quit");
        assert_eq!(
            Command::DeleteBackwardWord.to_string(),
            "delete_backward_word"
        );
    }

    #[test]
    fn test_default_keys_config() {
        let config = KeysConfig::default();
        assert!(!config.general.is_empty());
        assert!(!config.text_edit.is_empty());
        assert!(!config.list_navigation.is_empty());
        assert!(!config.confirm_cancel.is_empty());
        assert!(!config.repo_select.is_empty());
        assert!(!config.branch_select.is_empty());
    }

    #[test]
    fn test_parse_keymap() {
        let mut raw_map = HashMap::new();
        raw_map.insert("C-c".to_string(), "quit".to_string());
        raw_map.insert("enter".to_string(), "confirm".to_string());

        let keymap = KeysConfig::parse_keymap(&raw_map).unwrap();
        assert_eq!(keymap.len(), 2);

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(keymap.get(&ctrl_c), Some(&Command::Quit));

        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(keymap.get(&enter), Some(&Command::Confirm));
    }

    #[test]
    fn test_parse_invalid_key() {
        let mut raw_map = HashMap::new();
        raw_map.insert("invalid-key".to_string(), "quit".to_string());

        let result = KeysConfig::parse_keymap(&raw_map);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_command() {
        let mut raw_map = HashMap::new();
        raw_map.insert("C-c".to_string(), "invalid_command".to_string());

        let result = KeysConfig::parse_keymap(&raw_map);
        assert!(result.is_err());
    }

    #[test]
    fn test_mode_precedence_more_specific_wins() {
        let raw = KeysConfigRaw {
            general: HashMap::new(),
            text_edit: HashMap::new(),
            list_navigation: HashMap::new(),
            confirm_cancel: HashMap::new(),
            repo_select: {
                let mut map = HashMap::new();
                map.insert("C-c".to_string(), "show_help".to_string());
                map
            },
            branch_select: HashMap::new(),
        };

        let config = KeysConfig::from_raw(&raw).unwrap();
        let map = config.keymap_for_mode(&Mode::RepoSelect);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map.get(&ctrl_c), Some(&Command::ShowHelp));
    }

    #[test]
    fn test_noop_can_unbind_inherited_mapping() {
        let raw = KeysConfigRaw {
            general: HashMap::new(),
            text_edit: HashMap::new(),
            list_navigation: HashMap::new(),
            confirm_cancel: HashMap::new(),
            repo_select: HashMap::new(),
            branch_select: {
                let mut map = HashMap::new();
                map.insert("C-n".to_string(), "noop".to_string());
                map
            },
        };

        let config = KeysConfig::from_raw(&raw).unwrap();
        let map = config.keymap_for_mode(&Mode::BranchSelect);
        let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(map.get(&ctrl_n), None, "C-n should be unbound");
    }

    #[test]
    fn test_find_key_reverse_lookup() {
        let config = KeysConfig::default();
        let keymap = config.keymap_for_mode(&Mode::RepoSelect);
        let key = KeysConfig::find_key(&keymap, &Command::Quit);
        assert_eq!(
            key,
            Some(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn test_default_text_edit_bindings() {
        let config = KeysConfig::default();
        let keymap = config.keymap_for_mode(&Mode::RepoSelect);

        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let home = KeyEvent::new(KeyCode::Home, KeyModifiers::NONE);
        let end = KeyEvent::new(KeyCode::End, KeyModifiers::NONE);

        assert_eq!(keymap.get(&left), Some(&Command::MoveCursorLeft));
        assert_eq!(keymap.get(&right), Some(&Command::MoveCursorRight));
        assert_eq!(keymap.get(&home), Some(&Command::MoveCursorStart));
        assert_eq!(keymap.get(&end), Some(&Command::MoveCursorEnd));
    }

    #[test]
    fn test_noop_aliases() {
        assert_eq!(Command::from_str("noop").unwrap(), Command::Noop);
        assert_eq!(Command::from_str("none").unwrap(), Command::Noop);
        assert_eq!(Command::from_str("unbound").unwrap(), Command::Noop);
    }
}
