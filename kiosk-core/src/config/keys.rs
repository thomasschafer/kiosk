use crate::keyboard::{KeyCode, KeyEvent, KeyModifiers};
use crate::state::Mode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

/// Labels for a command: short hint for footer bar, long description for help overlay.
pub struct CommandLabels {
    /// Short label for the footer bar.
    pub hint: &'static str,
    /// Full description for the help overlay.
    pub description: &'static str,
}

/// Single source of truth for every `Command` variant and its metadata.
///
/// Each entry defines: variant name, config string, optional parse aliases,
/// footer hint, and help description. The macro generates the enum plus
/// `FromStr`, `Display`, `Serialize`, and `labels()` — so adding a new
/// command is a one-line change with no risk of forgetting a match arm.
macro_rules! define_commands {
    (
        $(
            $variant:ident {
                config_name: $config_name:literal,
                $(aliases: [$($alias:literal),+ $(,)?],)?
                hint: $hint:literal,
                description: $desc:literal,
            }
        ),* $(,)?
    ) => {
        /// Commands that can be bound to keys
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum Command { $($variant),* }

        impl FromStr for Command {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($config_name $($(| $alias)+)? => Ok(Command::$variant),)*
                    _ => Err(format!("Unknown command: {s}")),
                }
            }
        }

        impl std::fmt::Display for Command {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self {
                    $(Command::$variant => $config_name),*
                })
            }
        }

        impl Serialize for Command {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl Command {
            /// Get the labels (footer hint + help description) for this command.
            pub fn labels(&self) -> CommandLabels {
                match self {
                    $(Command::$variant => CommandLabels {
                        hint: $hint,
                        description: $desc,
                    }),*
                }
            }
        }
    };
}

define_commands! {
    // Special
    Noop {
        config_name: "noop",
        aliases: ["none", "unbound"],
        hint: "unbound",
        description: "Unbound",
    },

    // General
    Quit {
        config_name: "quit",
        hint: "quit",
        description: "Quit the application",
    },
    ShowHelp {
        config_name: "show_help",
        hint: "help",
        description: "Show help",
    },

    // Navigation
    OpenRepo {
        config_name: "open_repo",
        hint: "open",
        description: "Open repository in tmux",
    },
    EnterRepo {
        config_name: "enter_repo",
        hint: "branches",
        description: "Browse branches",
    },
    OpenBranch {
        config_name: "open_branch",
        hint: "open",
        description: "Open branch in tmux",
    },
    GoBack {
        config_name: "go_back",
        hint: "back",
        description: "Go back",
    },
    NewBranch {
        config_name: "new_branch",
        hint: "new branch",
        description: "New branch",
    },
    DeleteWorktree {
        config_name: "delete_worktree",
        hint: "delete worktree",
        description: "Delete worktree",
    },

    // List movement
    MoveUp {
        config_name: "move_up",
        hint: "up",
        description: "Move up",
    },
    MoveDown {
        config_name: "move_down",
        hint: "down",
        description: "Move down",
    },
    HalfPageUp {
        config_name: "half_page_up",
        hint: "half page up",
        description: "Half page up",
    },
    HalfPageDown {
        config_name: "half_page_down",
        hint: "half page down",
        description: "Half page down",
    },
    PageUp {
        config_name: "page_up",
        hint: "page up",
        description: "Page up",
    },
    PageDown {
        config_name: "page_down",
        hint: "page down",
        description: "Page down",
    },
    MoveTop {
        config_name: "move_top",
        hint: "top",
        description: "Move to top",
    },
    MoveBottom {
        config_name: "move_bottom",
        hint: "bottom",
        description: "Move to bottom",
    },

    // Text editing
    DeleteBackwardChar {
        config_name: "delete_backward_char",
        hint: "del char back",
        description: "Delete backward char",
    },
    DeleteForwardChar {
        config_name: "delete_forward_char",
        hint: "del char fwd",
        description: "Delete forward char",
    },
    DeleteBackwardWord {
        config_name: "delete_backward_word",
        hint: "del word back",
        description: "Delete backward word",
    },
    DeleteForwardWord {
        config_name: "delete_forward_word",
        hint: "del word fwd",
        description: "Delete forward word",
    },
    DeleteToStart {
        config_name: "delete_to_start",
        hint: "del to start",
        description: "Delete to start of line",
    },
    DeleteToEnd {
        config_name: "delete_to_end",
        hint: "del to end",
        description: "Delete to end of line",
    },
    MoveCursorLeft {
        config_name: "move_cursor_left",
        hint: "cursor left",
        description: "Move cursor left",
    },
    MoveCursorRight {
        config_name: "move_cursor_right",
        hint: "cursor right",
        description: "Move cursor right",
    },
    MoveCursorWordLeft {
        config_name: "move_cursor_word_left",
        hint: "word left",
        description: "Move cursor word left",
    },
    MoveCursorWordRight {
        config_name: "move_cursor_word_right",
        hint: "word right",
        description: "Move cursor word right",
    },
    MoveCursorStart {
        config_name: "move_cursor_start",
        hint: "cursor start",
        description: "Move cursor to start",
    },
    MoveCursorEnd {
        config_name: "move_cursor_end",
        hint: "cursor end",
        description: "Move cursor to end",
    },

    // Modal
    Confirm {
        config_name: "confirm",
        hint: "confirm",
        description: "Confirm",
    },
    Cancel {
        config_name: "cancel",
        hint: "cancel",
        description: "Cancel",
    },
}

/// Key bindings for a specific layer/mode
pub type KeyMap = HashMap<KeyEvent, Command>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingEntry {
    pub key: KeyEvent,
    pub command: Command,
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingSection {
    pub name: &'static str,
    pub entries: Vec<KeybindingEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenedKeybindingRow {
    pub section_index: usize,
    pub section_name: &'static str,
    pub key_display: String,
    pub command: Command,
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeKeybindingCatalog {
    pub mode: Mode,
    pub sections: Vec<KeybindingSection>,
    pub flattened: Vec<FlattenedKeybindingRow>,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Layer {
    // Layer precedence source of truth: earlier variants are lower precedence.
    General,
    TextEdit,
    ListNavigation,
    RepoSelect,
    BranchSelect,
    Modal,
}

impl Layer {
    const ORDER_ASC: [Layer; 6] = [
        Layer::General,
        Layer::TextEdit,
        Layer::ListNavigation,
        Layer::RepoSelect,
        Layer::BranchSelect,
        Layer::Modal,
    ];

    fn section_name(self) -> &'static str {
        match self {
            Layer::General => "general",
            Layer::TextEdit => "text_edit",
            Layer::ListNavigation => "list_navigation",
            Layer::RepoSelect => "repo_select",
            Layer::BranchSelect => "branch_select",
            Layer::Modal => "modal",
        }
    }
}

/// Complete key binding configuration, composed from reusable layers.
#[derive(Debug, Clone, Serialize)]
pub struct KeysConfig {
    pub general: KeyMap,
    pub text_edit: KeyMap,
    pub list_navigation: KeyMap,
    pub modal: KeyMap,
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
    modal: HashMap<String, String>,
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
            modal: Self::default_modal(),
            repo_select: Self::default_repo_select(),
            branch_select: Self::default_branch_select(),
        }
    }

    /// Build the effective keymap for a given app mode.
    pub fn keymap_for_mode(&self, mode: &Mode) -> KeyMap {
        let mut combined = KeyMap::new();
        for layer in Layer::ORDER_ASC {
            if Self::mode_uses_layer(mode, layer) {
                Self::apply_layer(&mut combined, self.layer(layer));
            }
        }

        combined
    }

    /// Build keybinding sections for a mode without applying higher-layer overrides.
    pub fn sections_for_mode(&self, mode: &Mode) -> Vec<KeybindingSection> {
        Layer::ORDER_ASC
            .into_iter()
            .filter(|layer| Self::mode_uses_layer(mode, *layer))
            .map(|layer| KeybindingSection {
                name: layer.section_name(),
                entries: Self::entries_for_layer(self.layer(layer)),
            })
            .collect()
    }

    /// Build sectioned and flattened keybinding data for a mode.
    pub fn catalog_for_mode(&self, mode: &Mode) -> ModeKeybindingCatalog {
        let sections = self.sections_for_mode(mode);
        let flattened = sections
            .iter()
            .enumerate()
            .flat_map(|(section_index, section)| {
                section
                    .entries
                    .iter()
                    .map(move |entry| FlattenedKeybindingRow {
                        section_index,
                        section_name: section.name,
                        key_display: entry.key.to_string(),
                        command: entry.command.clone(),
                        description: entry.description,
                    })
            })
            .collect();

        ModeKeybindingCatalog {
            mode: mode.clone(),
            sections,
            flattened,
        }
    }

    /// Return key-layer section names ordered from lowest to highest precedence.
    pub fn docs_section_order_asc() -> Vec<&'static str> {
        Layer::ORDER_ASC
            .into_iter()
            .map(Layer::section_name)
            .collect()
    }

    #[cfg(test)]
    fn layer_order_names_for_mode(mode: &Mode) -> Vec<&'static str> {
        Layer::ORDER_ASC
            .into_iter()
            .filter(|layer| Self::mode_uses_layer(mode, *layer))
            .map(Layer::section_name)
            .collect()
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

    fn entries_for_layer(layer: &KeyMap) -> Vec<KeybindingEntry> {
        let mut entries: Vec<KeybindingEntry> = layer
            .iter()
            .filter_map(|(key, command)| {
                if *command == Command::Noop {
                    None
                } else {
                    Some(KeybindingEntry {
                        key: *key,
                        command: command.clone(),
                        description: command.labels().description,
                    })
                }
            })
            .collect();

        entries.sort_by(|a, b| a.key.to_string().cmp(&b.key.to_string()));
        entries
    }

    fn layer(&self, layer: Layer) -> &KeyMap {
        match layer {
            Layer::General => &self.general,
            Layer::TextEdit => &self.text_edit,
            Layer::ListNavigation => &self.list_navigation,
            Layer::RepoSelect => &self.repo_select,
            Layer::BranchSelect => &self.branch_select,
            Layer::Modal => &self.modal,
        }
    }

    fn mode_uses_layer(mode: &Mode, layer: Layer) -> bool {
        match layer {
            Layer::General => true,
            Layer::TextEdit => mode.supports_text_edit(),
            Layer::ListNavigation => mode.supports_list_navigation(),
            Layer::RepoSelect => mode.supports_repo_select_actions(),
            Layer::BranchSelect => mode.supports_branch_select_actions(),
            Layer::Modal => mode.supports_modal_actions(),
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
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            Command::DeleteForwardChar,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Command::DeleteForwardChar,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            Command::DeleteBackwardWord,
        );
        map.insert(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
            Command::DeleteBackwardWord,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
            Command::DeleteForwardWord,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Command::DeleteToStart,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Command::DeleteToEnd,
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
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            Command::MoveCursorWordLeft,
        );
        map.insert(
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            Command::MoveCursorWordLeft,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
            Command::MoveCursorWordRight,
        );
        map.insert(
            KeyEvent::new(KeyCode::Right, KeyModifiers::ALT),
            Command::MoveCursorWordRight,
        );
        map.insert(
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            Command::MoveCursorStart,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            Command::MoveCursorStart,
        );
        map.insert(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            Command::MoveCursorEnd,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
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
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT),
            Command::HalfPageDown,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT),
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
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
            Command::PageDown,
        );
        map.insert(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT),
            Command::PageUp,
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

    fn default_modal() -> KeyMap {
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

    fn extend_layer(base: &mut KeyMap, raw_map: &HashMap<String, String>) -> Result<(), String> {
        base.extend(Self::parse_keymap(raw_map)?);
        Ok(())
    }

    /// Merge user configuration with defaults.
    ///
    /// Keep `Noop` values so higher-precedence layers can explicitly unbind inherited mappings.
    fn from_raw(raw: &KeysConfigRaw) -> Result<Self, String> {
        let mut config = Self::default();
        Self::extend_layer(&mut config.general, &raw.general)?;
        Self::extend_layer(&mut config.text_edit, &raw.text_edit)?;
        Self::extend_layer(&mut config.list_navigation, &raw.list_navigation)?;
        Self::extend_layer(&mut config.modal, &raw.modal)?;
        Self::extend_layer(&mut config.repo_select, &raw.repo_select)?;
        Self::extend_layer(&mut config.branch_select, &raw.branch_select)?;

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
        assert!(!config.modal.is_empty());
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
            modal: HashMap::new(),
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
            modal: HashMap::new(),
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
    fn test_modal_precedence_over_general_in_confirm_delete() {
        let raw = KeysConfigRaw {
            general: {
                let mut map = HashMap::new();
                map.insert("enter".to_string(), "quit".to_string());
                map
            },
            text_edit: HashMap::new(),
            list_navigation: HashMap::new(),
            modal: HashMap::new(),
            repo_select: HashMap::new(),
            branch_select: HashMap::new(),
        };

        let config = KeysConfig::from_raw(&raw).unwrap();
        let map = config.keymap_for_mode(&Mode::ConfirmWorktreeDelete {
            branch_name: "x".to_string(),
            has_session: false,
        });
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(map.get(&enter), Some(&Command::Confirm));
    }

    #[test]
    fn test_modal_noop_can_unbind_general_in_confirm_delete() {
        let raw = KeysConfigRaw {
            general: {
                let mut map = HashMap::new();
                map.insert("esc".to_string(), "quit".to_string());
                map
            },
            text_edit: HashMap::new(),
            list_navigation: HashMap::new(),
            modal: {
                let mut map = HashMap::new();
                map.insert("esc".to_string(), "noop".to_string());
                map
            },
            repo_select: HashMap::new(),
            branch_select: HashMap::new(),
        };

        let config = KeysConfig::from_raw(&raw).unwrap();
        let map = config.keymap_for_mode(&Mode::ConfirmWorktreeDelete {
            branch_name: "x".to_string(),
            has_session: false,
        });
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(map.get(&esc), None, "Esc should be unbound in modal");
    }

    #[test]
    fn test_layer_order_is_exported_for_docs() {
        assert_eq!(
            KeysConfig::layer_order_names_for_mode(&Mode::RepoSelect),
            vec!["general", "text_edit", "list_navigation", "repo_select"]
        );
        assert_eq!(
            KeysConfig::layer_order_names_for_mode(&Mode::SelectBaseBranch),
            vec!["general", "text_edit", "list_navigation", "modal"]
        );
        assert_eq!(
            KeysConfig::layer_order_names_for_mode(&Mode::ConfirmWorktreeDelete {
                branch_name: "x".to_string(),
                has_session: false,
            }),
            vec!["general", "modal"]
        );
    }

    #[test]
    fn test_docs_section_order_asc_is_derived_from_layer_precedence() {
        assert_eq!(
            KeysConfig::docs_section_order_asc(),
            vec![
                "general",
                "text_edit",
                "list_navigation",
                "repo_select",
                "branch_select",
                "modal",
            ]
        );
    }

    #[test]
    fn test_sections_for_mode_uses_layer_precedence_order() {
        let config = KeysConfig::default();
        let section_names: Vec<&str> = config
            .sections_for_mode(&Mode::BranchSelect)
            .iter()
            .map(|section| section.name)
            .collect();

        assert_eq!(
            section_names,
            vec!["general", "text_edit", "list_navigation", "branch_select"]
        );
    }

    #[test]
    fn test_sections_for_mode_excludes_noop_entries() {
        let raw = KeysConfigRaw {
            general: {
                let mut map = HashMap::new();
                map.insert("C-c".to_string(), "noop".to_string());
                map.insert("C-h".to_string(), "show_help".to_string());
                map
            },
            text_edit: HashMap::new(),
            list_navigation: HashMap::new(),
            modal: HashMap::new(),
            repo_select: HashMap::new(),
            branch_select: HashMap::new(),
        };

        let config = KeysConfig::from_raw(&raw).unwrap();
        let general = config
            .sections_for_mode(&Mode::RepoSelect)
            .into_iter()
            .find(|section| section.name == "general")
            .unwrap();

        assert_eq!(general.entries.len(), 1);
        assert_eq!(general.entries[0].command, Command::ShowHelp);
    }

    #[test]
    fn test_catalog_for_mode_flattened_order_is_deterministic() {
        let config = KeysConfig::default();
        let catalog = config.catalog_for_mode(&Mode::RepoSelect);

        let section_names: Vec<&str> = catalog
            .sections
            .iter()
            .map(|section| section.name)
            .collect();
        assert_eq!(
            section_names,
            vec!["general", "text_edit", "list_navigation", "repo_select"]
        );

        let mut previous_section_index = 0;
        let mut previous_key = String::new();
        for row in &catalog.flattened {
            if row.section_index == previous_section_index {
                assert!(previous_key <= row.key_display);
            } else {
                assert_eq!(row.section_index, previous_section_index + 1);
                previous_section_index = row.section_index;
                previous_key.clear();
            }
            previous_key = row.key_display.clone();
        }
    }

    #[test]
    fn test_modal_overrides_lower_layers_in_select_base_branch() {
        let raw = KeysConfigRaw {
            general: HashMap::new(),
            text_edit: HashMap::new(),
            list_navigation: {
                let mut map = HashMap::new();
                map.insert("enter".to_string(), "move_down".to_string());
                map
            },
            modal: HashMap::new(),
            repo_select: HashMap::new(),
            branch_select: HashMap::new(),
        };

        let config = KeysConfig::from_raw(&raw).unwrap();
        let map = config.keymap_for_mode(&Mode::SelectBaseBranch);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            map.get(&enter),
            Some(&Command::Confirm),
            "modal should have highest precedence in select-base flow"
        );
    }

    #[test]
    fn test_default_text_edit_and_navigation_bindings() {
        let config = KeysConfig::default();
        let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        let alt_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT);
        let alt_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT);
        let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        let alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);

        assert_eq!(
            config
                .keymap_for_mode(&Mode::RepoSelect)
                .get(&ctrl_u)
                .cloned(),
            Some(Command::DeleteToStart)
        );
        assert_eq!(
            config
                .keymap_for_mode(&Mode::RepoSelect)
                .get(&ctrl_d)
                .cloned(),
            Some(Command::DeleteForwardChar)
        );
        assert_eq!(
            config
                .keymap_for_mode(&Mode::RepoSelect)
                .get(&alt_j)
                .cloned(),
            Some(Command::HalfPageDown)
        );
        assert_eq!(
            config
                .keymap_for_mode(&Mode::RepoSelect)
                .get(&alt_k)
                .cloned(),
            Some(Command::HalfPageUp)
        );
        assert_eq!(
            config
                .keymap_for_mode(&Mode::RepoSelect)
                .get(&ctrl_v)
                .cloned(),
            Some(Command::PageDown)
        );
        assert_eq!(
            config
                .keymap_for_mode(&Mode::RepoSelect)
                .get(&alt_v)
                .cloned(),
            Some(Command::PageUp)
        );
    }

    #[test]
    fn test_noop_aliases() {
        assert_eq!(Command::from_str("noop").unwrap(), Command::Noop);
        assert_eq!(Command::from_str("none").unwrap(), Command::Noop);
        assert_eq!(Command::from_str("unbound").unwrap(), Command::Noop);
    }

    #[test]
    fn test_every_command_roundtrips_through_display_and_from_str() {
        let all_commands = [
            Command::Noop,
            Command::Quit,
            Command::ShowHelp,
            Command::OpenRepo,
            Command::EnterRepo,
            Command::OpenBranch,
            Command::GoBack,
            Command::NewBranch,
            Command::DeleteWorktree,
            Command::MoveUp,
            Command::MoveDown,
            Command::HalfPageUp,
            Command::HalfPageDown,
            Command::PageUp,
            Command::PageDown,
            Command::MoveTop,
            Command::MoveBottom,
            Command::DeleteBackwardChar,
            Command::DeleteForwardChar,
            Command::DeleteBackwardWord,
            Command::DeleteForwardWord,
            Command::DeleteToStart,
            Command::DeleteToEnd,
            Command::MoveCursorLeft,
            Command::MoveCursorRight,
            Command::MoveCursorWordLeft,
            Command::MoveCursorWordRight,
            Command::MoveCursorStart,
            Command::MoveCursorEnd,
            Command::Confirm,
            Command::Cancel,
        ];

        for cmd in &all_commands {
            let s = cmd.to_string();
            let parsed = Command::from_str(&s).unwrap_or_else(|e| {
                panic!("Command::{cmd:?} serializes as \"{s}\" but fails to parse back: {e}")
            });
            assert_eq!(
                &parsed, cmd,
                "Roundtrip failed for Command::{cmd:?} (serialized as \"{s}\")"
            );
        }
    }

    #[test]
    fn test_every_command_has_non_empty_description() {
        let all_commands = [
            Command::Noop,
            Command::Quit,
            Command::ShowHelp,
            Command::OpenRepo,
            Command::EnterRepo,
            Command::OpenBranch,
            Command::GoBack,
            Command::NewBranch,
            Command::DeleteWorktree,
            Command::MoveUp,
            Command::MoveDown,
            Command::HalfPageUp,
            Command::HalfPageDown,
            Command::PageUp,
            Command::PageDown,
            Command::MoveTop,
            Command::MoveBottom,
            Command::DeleteBackwardChar,
            Command::DeleteForwardChar,
            Command::DeleteBackwardWord,
            Command::DeleteForwardWord,
            Command::DeleteToStart,
            Command::DeleteToEnd,
            Command::MoveCursorLeft,
            Command::MoveCursorRight,
            Command::MoveCursorWordLeft,
            Command::MoveCursorWordRight,
            Command::MoveCursorStart,
            Command::MoveCursorEnd,
            Command::Confirm,
            Command::Cancel,
        ];

        for cmd in &all_commands {
            let labels = cmd.labels();
            assert!(
                !labels.description.is_empty(),
                "Command::{cmd:?} has an empty description"
            );
        }
    }

    #[test]
    fn test_footer_commands_all_have_hints() {
        let modes: Vec<Mode> = vec![
            Mode::RepoSelect,
            Mode::BranchSelect,
            Mode::SelectBaseBranch,
            Mode::ConfirmWorktreeDelete {
                branch_name: "x".into(),
                has_session: false,
            },
        ];

        for mode in &modes {
            for cmd in mode.footer_commands() {
                assert!(
                    !cmd.labels().hint.is_empty(),
                    "Command::{cmd:?} is in footer_commands for {mode:?} but has an empty hint"
                );
            }
        }
    }

    #[test]
    fn test_footer_commands_have_key_bindings() {
        let keys = KeysConfig::default();
        let modes: Vec<Mode> = vec![
            Mode::RepoSelect,
            Mode::BranchSelect,
            Mode::SelectBaseBranch,
            Mode::ConfirmWorktreeDelete {
                branch_name: "x".into(),
                has_session: false,
            },
        ];

        for mode in &modes {
            let keymap = keys.keymap_for_mode(mode);
            for cmd in mode.footer_commands() {
                assert!(
                    KeysConfig::find_key(&keymap, cmd).is_some(),
                    "Command::{cmd:?} is in footer_commands for {mode:?} but has no default key binding"
                );
            }
        }
    }

    #[test]
    fn test_loading_and_help_have_no_footer_commands() {
        assert!(
            Mode::Loading("test".into()).footer_commands().is_empty(),
            "Loading mode should have no footer commands"
        );
        assert!(
            Mode::Help {
                previous: Box::new(Mode::RepoSelect)
            }
            .footer_commands()
            .is_empty(),
            "Help mode should have no footer commands"
        );
    }
}
