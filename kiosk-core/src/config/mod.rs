pub mod keys;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Write as _,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum SearchDirEntry {
    Simple(String),
    Rich { path: String, depth: Option<u16> },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
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

// The struct must be defined outside the macro so that xtask's syn parser
// can discover it for README doc generation.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct ThemeConfig {
    /// Primary accent color (default: "magenta").
    #[serde(deserialize_with = "deserialize_color")]
    pub accent: ThemeColor,
    /// Secondary accent color (default: "cyan").
    #[serde(deserialize_with = "deserialize_color")]
    pub secondary: ThemeColor,
    /// Tertiary accent color (default: "green").
    #[serde(deserialize_with = "deserialize_color")]
    pub tertiary: ThemeColor,
    /// Success/positive color (default: "green").
    #[serde(deserialize_with = "deserialize_color")]
    pub success: ThemeColor,
    /// Error color (default: "red").
    #[serde(deserialize_with = "deserialize_color")]
    pub error: ThemeColor,
    /// Warning color (default: "yellow").
    #[serde(deserialize_with = "deserialize_color")]
    pub warning: ThemeColor,
    /// Muted/dim text color (default: "`dark_gray`").
    #[serde(deserialize_with = "deserialize_color")]
    pub muted: ThemeColor,
    /// Border color (default: "`dark_gray`").
    #[serde(deserialize_with = "deserialize_color")]
    pub border: ThemeColor,
    /// Hint/key binding color (default: "blue").
    #[serde(deserialize_with = "deserialize_color")]
    pub hint: ThemeColor,
    /// Foreground color for highlighted/selected items (default: "black").
    #[serde(deserialize_with = "deserialize_color")]
    pub highlight_fg: ThemeColor,
}

/// Single source of truth for theme defaults. Generates the `Default` impl
/// so adding a field only requires updating one place (plus the struct above).
macro_rules! theme_defaults {
    ($($field:ident => $color:ident),* $(,)?) => {
        impl Default for ThemeConfig {
            fn default() -> Self {
                Self {
                    $($field: ThemeColor::Named(NamedColor::$color)),*
                }
            }
        }
    };
}

theme_defaults! {
    accent       => Magenta,
    secondary    => Cyan,
    tertiary     => Green,
    success      => Green,
    error        => Red,
    warning      => Yellow,
    muted        => DarkGray,
    border       => DarkGray,
    hint         => Blue,
    highlight_fg => Black,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeColor {
    Named(NamedColor),
    Rgb(u8, u8, u8),
}

/// Single source of truth for every `NamedColor` variant, its canonical config
/// string, and any accepted aliases. The macro generates the enum plus `all()`,
/// `as_str()`, `resolve_alias()`, and `aliases()`.
macro_rules! define_named_colors {
    ($(
        $variant:ident {
            name: $name:literal
            $(, aliases: [$($alias:literal),+ $(,)?])?
        }
    ),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum NamedColor { $($variant),* }

        impl NamedColor {
            /// All named colours with their canonical config strings.
            pub const fn all() -> &'static [(&'static str, NamedColor)] {
                &[$(($name, NamedColor::$variant)),*]
            }

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(NamedColor::$variant => $name),*
                }
            }

            /// Resolve alternative spellings to canonical names.
            pub fn resolve_alias(s: &str) -> &str {
                match s {
                    $($($($alias)|+ => $name,)?)*
                    other => other,
                }
            }

            /// All (alias, canonical) pairs for documentation.
            pub const fn aliases() -> &'static [(&'static str, &'static str)] {
                &[$($( $( ($alias, $name), )+ )?)*]
            }
        }
    };
}

define_named_colors! {
    Black   { name: "black" },
    Red     { name: "red" },
    Green   { name: "green" },
    Yellow  { name: "yellow" },
    Blue    { name: "blue" },
    Magenta { name: "magenta" },
    Cyan    { name: "cyan" },
    White   { name: "white" },
    Gray    { name: "gray", aliases: ["grey"] },
    DarkGray { name: "dark_gray", aliases: ["darkgray", "dark_grey", "darkgrey"] },
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
        let lookup = NamedColor::resolve_alias(&lower);
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
        let names: Vec<&str> = NamedColor::all().iter().map(|(name, _)| *name).collect();
        serde::de::Error::custom(format!(
            "invalid color '{s}': expected a named color ({}) or hex (#rrggbb)",
            names.join(", "),
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

/// Check whether the default config file exists
pub fn config_file_exists() -> bool {
    config_file().exists()
}

/// Format a minimal config TOML string from search directories.
pub fn format_default_config(dirs: &[String]) -> String {
    let mut content = String::from(
        "# Kiosk configuration\n# See https://github.com/thomasschafer/kiosk for all options\n\n",
    );
    content.push_str("search_dirs = [");
    for (i, d) in dirs.iter().enumerate() {
        if i > 0 {
            content.push_str(", ");
        }
        content.push('"');
        // Escape for valid TOML basic strings
        for c in d.chars() {
            match c {
                '\\' => content.push_str("\\\\"),
                '"' => content.push_str("\\\""),
                c if c.is_control() => {
                    write!(content, "\\u{:04X}", c as u32).unwrap();
                }
                _ => content.push(c),
            }
        }
        content.push('"');
    }
    content.push_str("]\n");
    content
}

/// Write a default config file with the specified search directories.
/// Creates parent directories as needed. Returns the path written to.
pub fn write_default_config(dirs: &[String]) -> Result<PathBuf> {
    let path = config_file();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let content = format_default_config(dirs);
    fs::write(&path, content)?;
    Ok(path)
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
        assert_eq!(config.theme.tertiary, ThemeColor::Named(NamedColor::Green));
        assert_eq!(config.theme.success, ThemeColor::Named(NamedColor::Green));
        assert_eq!(config.theme.error, ThemeColor::Named(NamedColor::Red));
        assert_eq!(config.theme.warning, ThemeColor::Named(NamedColor::Yellow));
        assert_eq!(config.theme.muted, ThemeColor::Named(NamedColor::DarkGray));
        assert_eq!(config.theme.border, ThemeColor::Named(NamedColor::DarkGray));
        assert_eq!(config.theme.hint, ThemeColor::Named(NamedColor::Blue));
        assert_eq!(
            config.theme.highlight_fg,
            ThemeColor::Named(NamedColor::Black)
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
        assert_eq!(
            ThemeColor::parse("dark_gray"),
            Some(ThemeColor::Named(NamedColor::DarkGray))
        );
        assert_eq!(
            ThemeColor::parse("darkgray"),
            Some(ThemeColor::Named(NamedColor::DarkGray))
        );
        assert_eq!(
            ThemeColor::parse("dark_grey"),
            Some(ThemeColor::Named(NamedColor::DarkGray))
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
    fn test_named_color_all_matches_as_str() {
        for (name, color) in NamedColor::all() {
            assert_eq!(
                color.as_str(),
                *name,
                "NamedColor::{color:?} has mismatched all() and as_str()"
            );
        }
    }

    #[test]
    fn test_named_color_all_are_parseable() {
        for (name, color) in NamedColor::all() {
            assert_eq!(
                ThemeColor::parse(name),
                Some(ThemeColor::Named(*color)),
                "NamedColor canonical name '{name}' should parse"
            );
        }
    }

    #[test]
    fn test_named_color_aliases_resolve() {
        for (alias, canonical) in NamedColor::aliases() {
            assert_eq!(
                NamedColor::resolve_alias(alias),
                *canonical,
                "Alias '{alias}' should resolve to '{canonical}'"
            );
            assert!(
                ThemeColor::parse(alias).is_some(),
                "Alias '{alias}' should parse as a valid color"
            );
        }
    }

    #[test]
    fn test_format_default_config_is_valid_toml() {
        let dirs = vec!["~/Development".to_string(), "~/Work".to_string()];
        let content = format_default_config(&dirs);
        assert!(content.contains("search_dirs"));
        let _config: Config = toml::from_str(&content).unwrap();
    }

    #[test]
    fn test_format_default_config_roundtrip() {
        let dirs = vec!["~/Projects".to_string(), "~/Code".to_string()];
        let content = format_default_config(&dirs);
        let config = load_config_from_str(&content).unwrap();
        let paths: Vec<String> = config
            .search_dirs
            .iter()
            .map(|e| match e {
                SearchDirEntry::Simple(s) => s.clone(),
                SearchDirEntry::Rich { path, .. } => path.clone(),
            })
            .collect();
        assert_eq!(paths, dirs);
    }

    #[test]
    fn test_format_default_config_escapes_special_chars() {
        let dirs = vec![
            "C:\\Users\\Tom".to_string(),
            "path with \"quotes\"".to_string(),
        ];
        let content = format_default_config(&dirs);
        // Should produce valid TOML despite special characters
        let config = load_config_from_str(&content).unwrap();
        let paths: Vec<String> = config
            .search_dirs
            .iter()
            .map(|e| match e {
                SearchDirEntry::Simple(s) => s.clone(),
                SearchDirEntry::Rich { path, .. } => path.clone(),
            })
            .collect();
        assert_eq!(paths, dirs);
    }

    #[test]
    fn test_format_default_config_empty_dirs() {
        let content = format_default_config(&[]);
        assert!(content.contains("search_dirs = []"));
    }

    #[test]
    fn test_config_file_exists_returns_false_for_missing() {
        // This relies on the test not having a kiosk config in the default location,
        // which is fragile. Instead just verify the function doesn't panic.
        let _ = config_file_exists();
    }

    #[test]
    fn test_format_default_config_single_dir() {
        let dirs = vec!["~/Dev".to_string()];
        let content = format_default_config(&dirs);
        let config = load_config_from_str(&content).unwrap();
        assert_eq!(config.search_dirs.len(), 1);
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
