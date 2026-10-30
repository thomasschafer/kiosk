//! kiosk's own config sections, read beside the shared ones.

use std::time::Duration;

use anyhow::{Result, bail};
use kiosk_core::config::{ConfigExtension, ManagedWorktreesConfig};
use serde::{Deserialize, Serialize};

const MIN_POLL_INTERVAL_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Direction in which tmux splits off a configured pane.
pub enum OnOpenPaneDirection {
    Right,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// A command pane split off a new session's first pane.
pub struct OnOpenPaneConfig {
    /// Shell command typed into the new pane's shell, which stays when the command ends.
    /// The command must not be empty.
    pub command: String,
    /// Split direction: `right` or `down`.
    pub direction: OnOpenPaneDirection,
    /// Fraction of the split occupied by the new command pane. The value must be greater
    /// than 0 and less than 1, and defaults to 0.5 when omitted.
    #[serde(default)]
    pub ratio: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// The section is optional and contains no pane definitions by default.
pub struct OnOpenConfig {
    /// Pane definitions, created in order without moving focus from the first pane.
    /// Commands run from the opened repository or worktree. They run only when kiosk
    /// creates a session, not when it switches to an existing one.
    pub panes: Vec<OnOpenPaneConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Agent status shown beside open checkouts and used by `kiosk next`.
pub struct AgentConfig {
    /// Whether to show agent status at all. Set to `false` to stop kiosk looking at
    /// panes.
    pub enabled: bool,
    /// Whether to read the status programs report over OSC 7501. Kiosk pipes each pane's
    /// output (`pipe-pane`) to a small collector and sets global tmux hooks so new panes
    /// get one; panes you already pipe elsewhere are left alone. Set to `false` to turn
    /// this off, which also removes kiosk's hooks and collectors the next time kiosk runs.
    /// Panes without OSC 7501 status fall back to recognising known agents' screens.
    pub osc7501: bool,
    /// Milliseconds between status refreshes while the picker is open. The value must be
    /// at least 100.
    pub poll_interval_ms: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            osc7501: true,
            poll_interval_ms: 500,
        }
    }
}

impl AgentConfig {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.poll_interval_ms)
    }

    /// Whether kiosk should keep collectors running.
    pub fn collects_osc7501(&self) -> bool {
        self.enabled && self.osc7501
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// Kiosk reads `$XDG_CONFIG_HOME/kiosk/config.toml`, which is
/// `~/.config/kiosk/config.toml` when `XDG_CONFIG_HOME` is unset, or `config.toml` in
/// `$KIOSK_CONFIG_DIR` when that is set. All sections are optional, and the defaults are
/// shown below alongside curated examples for settings that benefit from one.
pub struct Config {
    /// Where kiosk creates worktrees for branches that have none.
    pub worktrees: ManagedWorktreesConfig,
    /// Configure command panes created after kiosk creates a session.
    pub on_open: OnOpenConfig,
    /// Configure agent status.
    pub agent: AgentConfig,
}

impl ConfigExtension for Config {
    fn validate(&self) -> Result<()> {
        self.worktrees.validate()?;
        for (index, pane) in self.on_open.panes.iter().enumerate() {
            if pane.command.trim().is_empty() {
                bail!("on_open pane {} command must not be empty", index + 1);
            }
            if pane
                .ratio
                .is_some_and(|ratio| !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0)
            {
                bail!(
                    "on_open pane {} ratio must be greater than 0 and less than 1",
                    index + 1
                );
            }
        }
        if self.agent.poll_interval_ms < MIN_POLL_INTERVAL_MS {
            bail!("agent poll_interval_ms must be at least {MIN_POLL_INTERVAL_MS}");
        }
        Ok(())
    }
}

/// The source of this module, for generating the README's config reference.
pub const SOURCE: &str = include_str!("config.rs");

#[cfg(test)]
mod tests {
    use kiosk_core::config::parse_config;

    use super::*;

    fn parse(contents: &str) -> Result<Config> {
        parse_config::<Config>(contents).map(|(_, config, _)| config)
    }

    #[test]
    fn defaults_turn_agent_status_and_osc7501_on() {
        let config = parse("").unwrap();
        assert_eq!(config, Config::default());
        assert!(config.agent.collects_osc7501());
        assert_eq!(config.agent.poll_interval(), Duration::from_millis(500));
        assert_eq!(config.worktrees.directory, "~/.kiosk/worktrees");
    }

    #[test]
    fn every_section_parses_without_warnings() {
        let (_, config, warnings) = parse_config::<Config>(
            r#"
[worktrees]
directory = "~/trees"

[on_open]
panes = [{ command = "hx", direction = "right", ratio = 0.4 }]

[agent]
osc7501 = false
poll_interval_ms = 250
"#,
        )
        .unwrap();
        assert_eq!(warnings, []);
        assert_eq!(config.worktrees.directory, "~/trees");
        assert_eq!(
            config.on_open.panes[0].direction,
            OnOpenPaneDirection::Right
        );
        assert!(!config.agent.collects_osc7501());
        assert_eq!(config.agent.poll_interval_ms, 250);
    }

    #[test]
    fn invalid_values_are_errors() {
        for (contents, message) in [
            (
                "[on_open]\npanes = [{ command = \" \", direction = \"right\" }]",
                "command must not be empty",
            ),
            (
                "[on_open]\npanes = [{ command = \"hx\", direction = \"down\", ratio = 1 }]",
                "ratio must be greater than 0",
            ),
            ("[agent]\npoll_interval_ms = 10", "at least 100"),
            ("[worktrees]\ndirectory = \"\"", "must not be empty"),
        ] {
            let error = format!("{:#}", parse(contents).unwrap_err());
            assert!(error.contains(message), "{contents}: {error}");
        }
    }

    #[test]
    fn unknown_keys_in_kiosk_sections_are_reported() {
        let (_, _, warnings) =
            parse_config::<Config>("[agent]\nlabels = 1\n[session]\nsplit_command = \"hx\"\n")
                .unwrap();
        let messages: Vec<_> = warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert_eq!(
            messages,
            [
                "unknown config key ignored: session",
                "unknown config key ignored: agent.labels",
            ]
        );
    }
}
