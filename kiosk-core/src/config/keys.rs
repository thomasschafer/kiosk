use crate::keyboard::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

/// Commands that can be bound to keys
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    /// No-op: explicitly unbinds a key (removes the default binding)
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

    // Movement commands
    MoveUp,
    MoveDown,
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    MoveTop,
    MoveBottom,

    // Search commands
    SearchPop,
    SearchDeleteWord,
    CursorLeft,
    CursorRight,
    CursorStart,
    CursorEnd,

    // Confirmation commands
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
            "search_pop" => Ok(Command::SearchPop),
            "search_delete_word" => Ok(Command::SearchDeleteWord),
            "cursor_left" => Ok(Command::CursorLeft),
            "cursor_right" => Ok(Command::CursorRight),
            "cursor_start" => Ok(Command::CursorStart),
            "cursor_end" => Ok(Command::CursorEnd),
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
            Command::SearchPop => "search_pop",
            Command::SearchDeleteWord => "search_delete_word",
            Command::CursorLeft => "cursor_left",
            Command::CursorRight => "cursor_right",
            Command::CursorStart => "cursor_start",
            Command::CursorEnd => "cursor_end",
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
            Command::OpenRepo => "Open repository",
            Command::EnterRepo => "Enter repository",
            Command::OpenBranch => "Open branch",
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
            Command::SearchPop => "Delete search character",
            Command::SearchDeleteWord => "Delete word",
            Command::CursorLeft => "Cursor left",
            Command::CursorRight => "Cursor right",
            Command::CursorStart => "Cursor to start",
            Command::CursorEnd => "Cursor to end",
            Command::Confirm => "Confirm",
            Command::Cancel => "Cancel",
        }
    }
}

/// Key bindings for a specific mode
pub type KeyMap = HashMap<KeyEvent, Command>;

/// Complete key binding configuration
#[derive(Debug, Clone)]
pub struct KeysConfig {
    pub general: KeyMap,
    pub repo_select: KeyMap,
    pub branch_select: KeyMap,
    pub new_branch_base: KeyMap,
    pub confirmation: KeyMap,
}

/// Intermediate structure for deserializing key bindings
#[derive(Debug, Deserialize)]
struct KeysConfigRaw {
    #[serde(default)]
    general: HashMap<String, String>,
    #[serde(default)]
    repo_select: HashMap<String, String>,
    #[serde(default)]
    branch_select: HashMap<String, String>,
    #[serde(default)]
    new_branch_base: HashMap<String, String>,
    #[serde(default)]
    confirmation: HashMap<String, String>,
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
            repo_select: Self::default_repo_select(),
            branch_select: Self::default_branch_select(),
            new_branch_base: Self::default_new_branch_base(),
            confirmation: Self::default_confirmation(),
        }
    }

    /// Common movement + search bindings shared across list modes
    fn common_list_bindings() -> KeyMap {
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
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            Command::SearchPop,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            Command::SearchDeleteWord,
        );
        map
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

    fn default_repo_select() -> KeyMap {
        let mut map = Self::common_list_bindings();
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
        let mut map = Self::common_list_bindings();
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

    fn default_new_branch_base() -> KeyMap {
        let mut map = Self::common_list_bindings();
        map.insert(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Command::OpenBranch,
        );
        map.insert(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Command::GoBack,
        );
        map
    }

    fn default_confirmation() -> KeyMap {
        let mut map = KeyMap::new();
        map.insert(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            Command::Confirm,
        );
        map.insert(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Command::Confirm,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            Command::Cancel,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE),
            Command::Cancel,
        );
        map.insert(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Command::Cancel,
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

    /// Merge user overrides into a keymap, then strip any Noop entries (unbinds)
    fn merge_and_strip(base: &mut KeyMap, overrides: KeyMap) {
        base.extend(overrides);
        base.retain(|_, cmd| *cmd != Command::Noop);
    }

    /// Merge user configuration with defaults
    fn from_raw(raw: &KeysConfigRaw) -> Result<Self, String> {
        let mut config = Self::default();

        Self::merge_and_strip(&mut config.general, Self::parse_keymap(&raw.general)?);
        Self::merge_and_strip(
            &mut config.repo_select,
            Self::parse_keymap(&raw.repo_select)?,
        );
        Self::merge_and_strip(
            &mut config.branch_select,
            Self::parse_keymap(&raw.branch_select)?,
        );
        Self::merge_and_strip(
            &mut config.new_branch_base,
            Self::parse_keymap(&raw.new_branch_base)?,
        );
        Self::merge_and_strip(
            &mut config.confirmation,
            Self::parse_keymap(&raw.confirmation)?,
        );

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
        assert_eq!(Command::from_str("move_up").unwrap(), Command::MoveUp);
        assert!(Command::from_str("invalid_command").is_err());
    }

    #[test]
    fn test_command_display() {
        assert_eq!(Command::Quit.to_string(), "quit");
        assert_eq!(Command::MoveUp.to_string(), "move_up");
    }

    #[test]
    fn test_command_description() {
        assert_eq!(Command::Quit.description(), "Quit the application");
        assert_eq!(Command::MoveUp.description(), "Move up");
    }

    #[test]
    fn test_default_keys_config() {
        let config = KeysConfig::default();
        assert!(!config.general.is_empty());
        assert!(!config.repo_select.is_empty());
        assert!(!config.branch_select.is_empty());
        assert!(!config.confirmation.is_empty());
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
    fn test_from_raw_merge() {
        let raw = KeysConfigRaw {
            general: {
                let mut map = HashMap::new();
                map.insert("F1".to_string(), "show_help".to_string());
                map
            },
            repo_select: HashMap::new(),
            branch_select: HashMap::new(),
            new_branch_base: HashMap::new(),
            confirmation: HashMap::new(),
        };

        let config = KeysConfig::from_raw(&raw).unwrap();

        // Should have default C-h -> show_help plus new F1 -> show_help
        assert!(config.general.len() >= 2);
        let f1_key = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        assert_eq!(config.general.get(&f1_key), Some(&Command::ShowHelp));
    }

    #[test]
    fn test_noop_unbinds_default() {
        let raw = KeysConfigRaw {
            general: {
                let mut map = HashMap::new();
                // Unbind the default C-h -> show_help
                map.insert("C-h".to_string(), "noop".to_string());
                map
            },
            repo_select: HashMap::new(),
            branch_select: HashMap::new(),
            new_branch_base: HashMap::new(),
            confirmation: HashMap::new(),
        };

        let config = KeysConfig::from_raw(&raw).unwrap();

        let ctrl_h = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        assert_eq!(config.general.get(&ctrl_h), None, "C-h should be unbound");
    }

    #[test]
    fn test_noop_aliases() {
        assert_eq!(Command::from_str("noop").unwrap(), Command::Noop);
        assert_eq!(Command::from_str("none").unwrap(), Command::Noop);
        assert_eq!(Command::from_str("unbound").unwrap(), Command::Noop);
    }
}
