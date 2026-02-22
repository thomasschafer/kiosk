pub mod keys;

use anyhow::Result;
use serde::Deserialize;
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

#[derive(Debug, Deserialize, Clone)]
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
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            accent: Self::default_accent(),
            secondary: Self::default_secondary(),
            success: Self::default_success(),
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
        let named = match s.to_lowercase().as_str() {
            "black" => NamedColor::Black,
            "red" => NamedColor::Red,
            "green" => NamedColor::Green,
            "yellow" => NamedColor::Yellow,
            "blue" => NamedColor::Blue,
            "magenta" => NamedColor::Magenta,
            "cyan" => NamedColor::Cyan,
            "white" => NamedColor::White,
            _ => return None,
        };
        Some(Self::Named(named))
    }
}

fn deserialize_color<'de, D>(deserializer: D) -> Result<ThemeColor, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    ThemeColor::parse(&s).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "invalid color '{s}': expected a named color (black, red, green, yellow, blue, magenta, cyan, white) or hex (#rrggbb)"
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
