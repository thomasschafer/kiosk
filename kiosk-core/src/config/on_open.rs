//! The `[on_open]` section: command panes a backend creates beside a checkout it opens.
//! Products that support it include [`OnOpenConfig`] in their config extension and call
//! [`OnOpenConfig::validate`] from its `validate`. Each product describes in its own words
//! when the panes are created, so these doc comments are generic.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Side of the opened checkout's first pane on which a configured pane is created.
pub enum OnOpenPaneDirection {
    Right,
    Down,
}

impl OnOpenPaneDirection {
    /// The name the config file uses.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// A command pane created when a checkout is opened.
pub struct OnOpenPaneConfig {
    /// Shell command run in the new pane, in the opened checkout. The command must not be
    /// empty.
    pub command: String,
    /// Split direction: `right` or `down`.
    pub direction: OnOpenPaneDirection,
    /// Fraction of the resulting split occupied by the new command pane. The value
    /// must be greater than 0 and less than 1, and defaults to 0.5 when omitted.
    #[serde(default)]
    pub ratio: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// The section is optional and contains no pane definitions by default.
pub struct OnOpenConfig {
    /// Pane definitions, created in order without moving focus from the first pane.
    pub panes: Vec<OnOpenPaneConfig>,
}

impl OnOpenConfig {
    pub fn validate(&self) -> Result<()> {
        for (index, pane) in self.panes.iter().enumerate() {
            let number = index + 1;
            if pane.command.trim().is_empty() {
                bail!("on_open pane {number} command must not be empty");
            }
            if pane
                .ratio
                .is_some_and(|ratio| !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0)
            {
                bail!("on_open pane {number} ratio must be greater than 0 and less than 1");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> Result<OnOpenConfig> {
        let config: OnOpenConfig = toml::from_str(contents)?;
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn parses_panes_with_and_without_a_ratio() {
        let config = parse(
            r#"panes = [
    { command = "hx", direction = "right" },
    { command = "cargo test", direction = "down", ratio = 0.35 },
]"#,
        )
        .unwrap();
        assert_eq!(
            config.panes,
            [
                OnOpenPaneConfig {
                    command: "hx".into(),
                    direction: OnOpenPaneDirection::Right,
                    ratio: None,
                },
                OnOpenPaneConfig {
                    command: "cargo test".into(),
                    direction: OnOpenPaneDirection::Down,
                    ratio: Some(0.35),
                },
            ]
        );
        assert_eq!(parse("").unwrap(), OnOpenConfig::default());
    }

    #[test]
    fn direction_names_match_the_config_file() {
        for direction in [OnOpenPaneDirection::Right, OnOpenPaneDirection::Down] {
            let parsed: OnOpenPaneDirection = toml::Value::String(direction.as_str().into())
                .try_into()
                .unwrap();
            assert_eq!(parsed, direction);
        }
    }

    #[test]
    fn rejects_unknown_directions_empty_commands_and_out_of_range_ratios() {
        let direction = format!(
            "{:#}",
            parse(r#"panes = [{ command = "hx", direction = "left" }]"#).unwrap_err()
        );
        assert!(direction.contains("unknown variant `left`"), "{direction}");
        assert!(
            direction.contains("expected `right` or `down`"),
            "{direction}"
        );

        let command = format!(
            "{:#}",
            parse(
                r#"panes = [
    { command = "hx", direction = "right" },
    { command = "  ", direction = "right" },
]"#
            )
            .unwrap_err()
        );
        assert_eq!(command, "on_open pane 2 command must not be empty");

        for ratio in ["0", "1", "-0.1", "1.1", "nan", "inf"] {
            let error = format!(
                "{:#}",
                parse(&format!(
                    "panes = [{{ command = \"hx\", direction = \"right\", ratio = {ratio} }}]"
                ))
                .unwrap_err()
            );
            assert_eq!(
                error, "on_open pane 1 ratio must be greater than 0 and less than 1",
                "ratio {ratio}"
            );
        }
    }
}
