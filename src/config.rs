use anyhow::Result;
use serde::Deserialize;
use std::{
    fs,
    path::PathBuf,
};

pub const APP_NAME: &str = "wts";

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .expect("Unable to find config directory")
        .join(APP_NAME)
}

fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Directories to scan for git repos (scanned 1 level deep)
    pub search_dirs: Vec<String>,

    /// Layout when creating a new tmux session
    #[serde(default)]
    pub session: SessionConfig,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// Command to run in a split pane when creating a new session (e.g. "hx")
    pub split_command: Option<String>,
}

impl Config {
    pub fn resolved_search_dirs(&self) -> Vec<PathBuf> {
        self.search_dirs
            .iter()
            .map(|d| {
                if d.starts_with('~') {
                    if let Some(home) = dirs::home_dir() {
                        return home.join(&d[2..]);
                    }
                }
                PathBuf::from(d)
            })
            .filter(|p| p.is_dir())
            .collect()
    }
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
