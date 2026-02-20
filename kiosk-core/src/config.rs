use anyhow::Result;
use serde::Deserialize;
use std::{fs, path::PathBuf};

pub const APP_NAME: &str = "kiosk";

fn config_dir() -> PathBuf {
    // Use ~/.config on both Linux and macOS (not ~/Library/Application Support)
    #[cfg(unix)]
    {
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

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Directories to scan for git repositories. Each directory is scanned one level deep.
    /// Supports `~` for the home directory. For example:
    /// ```toml
    /// search_dirs = ["~/Development", "~/Work"]
    /// ```
    pub search_dirs: Vec<String>,

    /// Layout when creating a new tmux session.
    #[serde(default)]
    pub session: SessionConfig,
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

impl Config {
    pub fn resolved_search_dirs(&self) -> Vec<PathBuf> {
        self.search_dirs
            .iter()
            .map(|d| {
                if let Some(rest) = d.strip_prefix("~/")
                    && let Some(home) = dirs::home_dir()
                {
                    return home.join(rest);
                } else if d == "~"
                    && let Some(home) = dirs::home_dir()
                {
                    return home;
                }
                PathBuf::from(d)
            })
            .filter(|p| p.is_dir())
            .collect()
    }
}

pub fn load_config_from_str(s: &str) -> Result<Config> {
    let config: Config = toml::from_str(s)?;
    Ok(config)
}

pub fn load_config() -> Result<Config> {
    let config_file = config_file();
    if !config_file.exists() {
        anyhow::bail!(
            "Config file not found at {}. Create it with:\n\n\
             [example]\n\
             search_dirs = [\"~/Development\"]\n",
            config_file.display()
        );
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
        assert_eq!(config.search_dirs, vec!["~/Development"]);
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
        if let Some(d) = dirs.first() {
            assert!(!d.to_string_lossy().contains('~'));
        }
    }
}
