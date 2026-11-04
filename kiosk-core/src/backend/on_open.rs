//! Running `[on_open]` panes in a session that has just opened, for any backend that can
//! split a pane and run a command in it.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use crate::config::{OnOpenConfig, on_open::OnOpenPaneDirection};

/// One `[on_open]` pane to split off the session's first pane.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneSplitRequest {
    /// The pane to split.
    pub pane_id: String,
    pub direction: OnOpenPaneDirection,
    /// The share of the split the existing pane keeps, when the config gives one.
    pub ratio: Option<f64>,
    pub cwd: PathBuf,
    /// Whether the new pane takes focus.
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSplitResponse {
    pub pane_id: String,
}

/// Why an `[on_open]` pane didn't open with its command, with the backend's error `E`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneOpenError<E> {
    /// No pane was made.
    Split(E),
    /// The pane was made, but its command didn't reach it.
    Run(E),
}

/// Opens each configured pane beside `root_pane_id` with `open_pane`. Panes that fail are
/// reported in the returned warning; the rest still open.
pub fn apply<E: fmt::Display>(
    on_open: &OnOpenConfig,
    root_pane_id: &str,
    checkout: &Path,
    mut open_pane: impl FnMut(&PaneSplitRequest, &str) -> Result<PaneSplitResponse, PaneOpenError<E>>,
) -> Option<String> {
    let mut errors = Vec::new();
    for (index, pane) in on_open.panes.iter().enumerate() {
        let request = PaneSplitRequest {
            pane_id: root_pane_id.into(),
            direction: pane.direction,
            // The config gives the new pane's share; backends split by the existing pane's.
            ratio: pane.ratio.map(|ratio| 1.0 - ratio),
            cwd: checkout.to_path_buf(),
            focus: false,
        };
        match open_pane(&request, &pane.command) {
            Ok(_) => {}
            Err(PaneOpenError::Split(error)) => {
                errors.push(format!("pane {} split failed: {error}", index + 1));
            }
            Err(PaneOpenError::Run(error)) => {
                errors.push(format!("pane {} run failed: {error}", index + 1));
            }
        }
    }
    (!errors.is_empty()).then(|| format!("on_open: {}", errors.join("; ")))
}

/// Both warnings, or whichever there is.
pub fn combine_warnings(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::config::on_open::OnOpenPaneConfig;

    use super::*;

    fn on_open_config() -> OnOpenConfig {
        OnOpenConfig {
            panes: vec![
                OnOpenPaneConfig {
                    command: "hx".into(),
                    direction: OnOpenPaneDirection::Right,
                    ratio: Some(0.4),
                },
                OnOpenPaneConfig {
                    command: "cargo test".into(),
                    direction: OnOpenPaneDirection::Down,
                    ratio: None,
                },
            ],
        }
    }

    #[test]
    fn opens_each_configured_pane_with_its_command_in_order() {
        let mut opened = Vec::new();
        let warning = apply::<String>(
            &on_open_config(),
            "p_root",
            Path::new("/repo"),
            |request, command| {
                opened.push((request.clone(), command.to_string()));
                Ok(PaneSplitResponse {
                    pane_id: format!("p_{}", opened.len() + 1),
                })
            },
        );
        assert!(warning.is_none());
        assert_eq!(
            opened,
            [
                (
                    PaneSplitRequest {
                        pane_id: "p_root".into(),
                        direction: OnOpenPaneDirection::Right,
                        ratio: Some(0.6),
                        cwd: "/repo".into(),
                        focus: false,
                    },
                    "hx".into()
                ),
                (
                    PaneSplitRequest {
                        pane_id: "p_root".into(),
                        direction: OnOpenPaneDirection::Down,
                        ratio: None,
                        cwd: "/repo".into(),
                        focus: false,
                    },
                    "cargo test".into()
                ),
            ]
        );
    }

    #[test]
    fn the_ratio_is_the_new_pane_fraction() {
        let config = OnOpenConfig {
            panes: vec![OnOpenPaneConfig {
                command: "hx".into(),
                direction: OnOpenPaneDirection::Right,
                ratio: Some(0.35),
            }],
        };
        let mut ratio = None;
        apply::<String>(&config, "p_root", Path::new("/repo"), |request, _| {
            ratio = request.ratio;
            Ok(PaneSplitResponse {
                pane_id: "p_2".into(),
            })
        });
        assert!((ratio.expect("a configured ratio") - 0.65).abs() < f64::EPSILON);
    }

    #[test]
    fn failures_are_collected_into_one_warning() {
        let mut results = [
            Err(PaneOpenError::Split("too small".to_owned())),
            Err(PaneOpenError::Run("shell closed".to_owned())),
        ]
        .into_iter();
        let warning = apply::<String>(&on_open_config(), "p_root", Path::new("/repo"), |_, _| {
            results.next().unwrap()
        })
        .unwrap();
        assert!(warning.starts_with("on_open: "));
        assert!(warning.contains("pane 1 split failed"));
        assert!(warning.contains("pane 2 run failed"));
    }
}
