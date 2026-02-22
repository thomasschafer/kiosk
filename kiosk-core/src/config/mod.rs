pub mod keys;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub use keys::{Command, KeysConfig};

pub const APP_NAME: &str = "kiosk";

fn config_dir() -> PathBuf {
    // Use ~/.config on both Linux and macOS (not ~/Library/Application Support)
    #[cfg(unix)]
    {
        if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME")
            && !xdg_config_home.is_empty()
        {
            return PathBuf::from(xdg_config_home).join(APP_NAME);
        }
        dirs::home_dir()
            .expect("Unable to find home directory")
            .join(".config")
            .join(APP_NAME)
    }
    #[cfg(windows)]
    {
        dirs::config_dir()
            .expect("Unable to find config directory")
            .join(APP_NAME)
    }
}

fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub const DEFAULT_SEARCH_DEPTH: u16 = 1;

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum SearchDirEntry {
    Simple(String),
    Rich { path: String, depth: Option<u16> },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Directories to scan for git repositories. Each directory can be scanned to a specified depth, with a default of 1 (i.e. just the top level).
    /// Supports `~` for the home directory. For example:
    /// ```toml
    /// search_dirs = ["~/Development", { path = "~/Work", depth = 2 }]
    /// ```
    pub search_dirs: Vec<SearchDirEntry>,

    /// Layout when creating a new tmux session.
    #[serde(default)]
    pub session: SessionConfig,

    /// Color theme configuration.
    #[serde(default)]
    pub theme: ThemeConfig,

    /// Key binding configuration.
    /// To unbind an inherited key mapping, assign it to `noop`.
    #[serde(default)]
    pub keys: KeysConfig,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// Command to run in a split pane when creating a new session. For example, to open
    /// Helix in a vertical split:
    /// ```toml
    /// [session]
    /// split_command = "hx"
    /// ```
    pub split_command: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ThemeConfig {
    /// Primary accent color (default: "magenta").
    #[serde(
        default = "ThemeConfig::default_accent",
        deserialize_with = "deserialize_color"
    )]
    pub accent: ThemeColor,
    /// Secondary accent color (default: "cyan").
    #[serde(
        default = "ThemeConfig::default_secondary",
        deserialize_with = "deserialize_color"
    )]
    pub secondary: ThemeColor,
    /// Success/positive color (default: "green").
    #[serde(
        default = "ThemeConfig::default_success",
        deserialize_with = "deserialize_color"
    )]
    pub success: ThemeColor,
    /// Error color (default: "red").
    #[serde(
        default = "ThemeConfig::default_error",
        deserialize_with = "deserialize_color"
    )]
    pub error: ThemeColor,
    /// Warning color (default: "yellow").
    #[serde(
        default = "ThemeConfig::default_warning",
        deserialize_with = "deserialize_color"
    )]
    pub warning: ThemeColor,
    /// Muted/dim text color (default: "gray").
    #[serde(
        default = "ThemeConfig::default_muted",
        deserialize_with = "deserialize_color"
    )]
    pub muted: ThemeColor,
    /// Border color (default: "gray").
    #[serde(
        default = "ThemeConfig::default_border",
        deserialize_with = "deserialize_color"
    )]
    pub border: ThemeColor,
    /// Title color (default: "blue").
    #[serde(
        default = "ThemeConfig::default_title",
        deserialize_with = "deserialize_color"
    )]
    pub title: ThemeColor,
    /// Hint/key binding color (default: "blue").
    #[serde(
        default = "ThemeConfig::default_hint",
        deserialize_with = "deserialize_color"
    )]
    pub hint: ThemeColor,
    /// Foreground color for highlighted/selected items (default: "white").
    #[serde(
        default = "ThemeConfig::default_highlight_fg",
        deserialize_with = "deserialize_color"
    )]
    pub highlight_fg: ThemeColor,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            accent: Self::default_accent(),
            secondary: Self::default_secondary(),
            success: Self::default_success(),
            error: Self::default_error(),
            warning: Self::default_warning(),
            muted: Self::default_muted(),
            border: Self::default_border(),
            title: Self::default_title(),
            hint: Self::default_hint(),
            highlight_fg: Self::default_highlight_fg(),
        }
    }
}

impl ThemeConfig {
    fn default_accent() -> ThemeColor {
        ThemeColor::Named(NamedColor::Magenta)
    }
    fn default_secondary() -> ThemeColor {
        ThemeColor::Named(NamedColor::Cyan)
    }
    fn default_success() -> ThemeColor {
        ThemeColor::Named(NamedColor::Green)
    }
    fn default_error() -> ThemeColor {
        ThemeColor::Named(NamedColor::Red)
    }
    fn default_warning() -> ThemeColor {
        ThemeColor::Named(NamedColor::Yellow)
    }
    fn default_muted() -> ThemeColor {
        ThemeColor::Named(NamedColor::Gray)
    }
    fn default_border() -> ThemeColor {
        ThemeColor::Named(NamedColor::Gray)
    }
    fn default_title() -> ThemeColor {
        ThemeColor::Named(NamedColor::Blue)
    }
    fn default_hint() -> ThemeColor {
        ThemeColor::Named(NamedColor::Blue)
    }
    fn default_highlight_fg() -> ThemeColor {
        ThemeColor::Named(NamedColor::White)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeColor {
    Named(NamedColor),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Gray,
}

impl NamedColor {
    /// All named colours in alphabetical order, as accepted by the config parser.
    pub const fn all() -> &'static [(&'static str, NamedColor)] {
        &[
            ("black", NamedColor::Black),
            ("blue", NamedColor::Blue),
            ("cyan", NamedColor::Cyan),
            ("gray", NamedColor::Gray),
            ("green", NamedColor::Green),
            ("magenta", NamedColor::Magenta),
            ("red", NamedColor::Red),
            ("white", NamedColor::White),
            ("yellow", NamedColor::Yellow),
        ]
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Red => "red",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Blue => "blue",
            Self::Magenta => "magenta",
            Self::Cyan => "cyan",
            Self::White => "white",
            Self::Gray => "gray",
        }
    }
}

impl std::fmt::Display for ThemeColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(n) => f.write_str(n.as_str()),
            Self::Rgb(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

impl Serialize for ThemeColor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl ThemeColor {
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(hex) = s.strip_prefix('#')
            && hex.len() == 6
        {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Self::Rgb(r, g, b));
        }
        let lower = s.to_lowercase();
        // Handle aliases not in the canonical list
        let lookup = match lower.as_str() {
            "grey" => "gray",
            other => other,
        };
        NamedColor::all()
            .iter()
            .find(|(name, _)| *name == lookup)
            .map(|(_, color)| Self::Named(*color))
    }
}

fn deserialize_color<'de, D>(deserializer: D) -> Result<ThemeColor, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    ThemeColor::parse(&s).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "invalid color '{s}': expected a named color (black, red, green, yellow, blue, magenta, cyan, white, gray/grey) or hex (#rrggbb)"
        ))
    })
}

impl Config {
    pub fn resolved_search_dirs(&self) -> Vec<(PathBuf, u16)> {
        self.search_dirs
            .iter()
            .filter_map(|entry| {
                let (path_str, depth) = match entry {
                    SearchDirEntry::Simple(path) => (path.as_str(), DEFAULT_SEARCH_DEPTH),
                    SearchDirEntry::Rich { path, depth } => {
                        (path.as_str(), depth.unwrap_or(DEFAULT_SEARCH_DEPTH))
                    }
                };

                let resolved_path = if let Some(rest) = path_str.strip_prefix("~/")
                    && let Some(home) = dirs::home_dir()
                {
                    home.join(rest)
                } else if path_str == "~"
                    && let Some(home) = dirs::home_dir()
                {
                    home
                } else {
                    PathBuf::from(path_str)
                };

                if resolved_path.is_dir() {
                    Some((resolved_path, depth))
                } else {
                    None
                }
            })
            .collect()
    }
}

pub fn load_config_from_str(s: &str) -> Result<Config> {
    let config: Config = toml::from_str(s)?;
    Ok(config)
}

pub fn load_config(config_override: Option<&Path>) -> Result<Config> {
    let config_file = match config_override {
        Some(path) => path.to_path_buf(),
        None => config_file(),
    };
    if !config_file.exists() {
        anyhow::bail!("Config file not found at {}", config_file.display());
    }
    let contents = fs::read_to_string(&config_file)?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config() {
        let config = load_config_from_str(r#"search_dirs = ["~/Development"]"#).unwrap();
        assert!(
            matches!(&config.search_dirs[0], SearchDirEntry::Simple(s) if s == "~/Development")
        );
        assert!(config.session.split_command.is_none());
    }

    #[test]
    fn test_full_config() {
        let config = load_config_from_str(
            r#"
search_dirs = ["~/Development", "~/Work"]

[session]
split_command = "hx"
"#,
        )
        .unwrap();
        assert_eq!(config.search_dirs.len(), 2);
        assert!(
            matches!(&config.search_dirs[0], SearchDirEntry::Simple(s) if s == "~/Development")
        );
        assert!(matches!(&config.search_dirs[1], SearchDirEntry::Simple(s) if s == "~/Work"));
        assert_eq!(config.session.split_command.as_deref(), Some("hx"));
    }

    #[test]
    fn test_empty_config_fails() {
        let result = load_config_from_str("");
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_field_rejected() {
        let result = load_config_from_str(
            r#"
search_dirs = ["~/Development"]
unknown_field = true
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_tilde_expansion() {
        let config =
            load_config_from_str(r#"search_dirs = ["~/", "~/nonexistent_dir_xyz"]"#).unwrap();
        let dirs = config.resolved_search_dirs();
        // ~ should resolve to home (which exists), nonexistent should be filtered
        assert!(dirs.len() <= 1);
        if let Some((d, depth)) = dirs.first() {
            assert!(!d.to_string_lossy().contains('~'));
            assert_eq!(*depth, 1); // default depth
        }
    }

    #[test]
    fn test_theme_config_defaults() {
        let config = load_config_from_str(r#"search_dirs = ["~/Development"]"#).unwrap();
        assert_eq!(config.theme.accent, ThemeColor::Named(NamedColor::Magenta));
        assert_eq!(config.theme.secondary, ThemeColor::Named(NamedColor::Cyan));
        assert_eq!(config.theme.success, ThemeColor::Named(NamedColor::Green));
        assert_eq!(config.theme.error, ThemeColor::Named(NamedColor::Red));
        assert_eq!(config.theme.warning, ThemeColor::Named(NamedColor::Yellow));
        assert_eq!(config.theme.muted, ThemeColor::Named(NamedColor::Gray));
        assert_eq!(config.theme.border, ThemeColor::Named(NamedColor::Gray));
        assert_eq!(config.theme.title, ThemeColor::Named(NamedColor::Blue));
        assert_eq!(config.theme.hint, ThemeColor::Named(NamedColor::Blue));
        assert_eq!(
            config.theme.highlight_fg,
            ThemeColor::Named(NamedColor::White)
        );
    }

    #[test]
    fn test_theme_config_custom() {
        let config = load_config_from_str(
            r##"
search_dirs = ["~/Development"]

[theme]
accent = "blue"
secondary = "#ff00ff"
"##,
        )
        .unwrap();
        assert_eq!(config.theme.accent, ThemeColor::Named(NamedColor::Blue));
        assert_eq!(config.theme.secondary, ThemeColor::Rgb(255, 0, 255));
        assert_eq!(config.theme.success, ThemeColor::Named(NamedColor::Green));
    }

    #[test]
    fn test_theme_invalid_color_rejected() {
        let result = load_config_from_str(
            r#"
search_dirs = ["~/Development"]

[theme]
accent = "notacolor"
"#,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid color"), "Error was: {err}");
    }

    #[test]
    fn test_theme_color_parse() {
        assert_eq!(
            ThemeColor::parse("magenta"),
            Some(ThemeColor::Named(NamedColor::Magenta))
        );
        assert_eq!(
            ThemeColor::parse("RED"),
            Some(ThemeColor::Named(NamedColor::Red))
        );
        assert_eq!(
            ThemeColor::parse("#ff0000"),
            Some(ThemeColor::Rgb(255, 0, 0))
        );
        assert_eq!(
            ThemeColor::parse("gray"),
            Some(ThemeColor::Named(NamedColor::Gray))
        );
        assert_eq!(
            ThemeColor::parse("grey"),
            Some(ThemeColor::Named(NamedColor::Gray))
        );
        assert_eq!(ThemeColor::parse("notacolor"), None);
        assert_eq!(ThemeColor::parse("#fff"), None);
        assert_eq!(ThemeColor::parse("#zzzzzz"), None);
    }

    #[test]
    fn test_theme_unknown_field_rejected() {
        let result = load_config_from_str(
            r#"
search_dirs = ["~/Development"]

[theme]
accent = "blue"
unknown = "bad"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_rich_search_dirs() {
        let config = load_config_from_str(
            r#"search_dirs = [
                "~/Development",
                { path = "~/Work", depth = 3 },
                { path = "~/Projects" }
            ]"#,
        )
        .unwrap();
        assert_eq!(config.search_dirs.len(), 3);

        assert!(
            matches!(&config.search_dirs[0], SearchDirEntry::Simple(s) if s == "~/Development")
        );
        match &config.search_dirs[1] {
            SearchDirEntry::Rich { path, depth } => {
                assert_eq!(path, "~/Work");
                assert_eq!(*depth, Some(3));
            }
            SearchDirEntry::Simple(_) => panic!("Expected Rich variant"),
        }
        match &config.search_dirs[2] {
            SearchDirEntry::Rich { path, depth } => {
                assert_eq!(path, "~/Projects");
                assert_eq!(*depth, None);
            }
            SearchDirEntry::Simple(_) => panic!("Expected Rich variant"),
        }
    }
}
