//! The `[worktrees]` section of products whose backend leaves worktrees to kiosk. Products
//! whose backend places checkouts itself leave it out, so their users never see a setting
//! that would be ignored.

use std::path::Path;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::backend::WorktreePlacement;

pub const DEFAULT_DIRECTORY: &str = "~/.kiosk/worktrees";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Where kiosk creates checkouts for branches that do not have one.
pub struct ManagedWorktreesConfig {
    /// Directory holding new checkouts, each at `<directory>/<repository>/<branch>`.
    /// `~` and paths beginning with `~/` expand from the user's home directory; the path
    /// must be absolute after expansion.
    pub directory: String,
}

impl Default for ManagedWorktreesConfig {
    fn default() -> Self {
        Self {
            directory: DEFAULT_DIRECTORY.into(),
        }
    }
}

impl ManagedWorktreesConfig {
    pub fn validate(&self) -> Result<()> {
        if self.directory.trim().is_empty() {
            return Err(anyhow!("worktrees directory must not be empty"));
        }
        Ok(())
    }

    pub fn placement(&self, home: Option<&Path>) -> Result<WorktreePlacement> {
        let directory = super::expand_tilde(&self.directory, home)?;
        WorktreePlacement::new(directory).map_err(|message| anyhow!(message))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_default_directory_expands_below_home() {
        let home = std::env::temp_dir().join("home");
        let placement = ManagedWorktreesConfig::default()
            .placement(Some(&home))
            .unwrap();
        assert_eq!(placement.root(), home.join(".kiosk/worktrees"));
    }

    #[test]
    fn relative_and_empty_directories_are_rejected() {
        let relative = ManagedWorktreesConfig {
            directory: "worktrees".into(),
        };
        assert!(relative.placement(None).is_err());
        let empty = ManagedWorktreesConfig {
            directory: " ".into(),
        };
        assert!(empty.validate().is_err());
    }

    #[test]
    fn tilde_needs_a_home() {
        assert!(ManagedWorktreesConfig::default().placement(None).is_err());
        let absolute = ManagedWorktreesConfig {
            directory: std::env::temp_dir().join("trees").display().to_string(),
        };
        assert_eq!(
            absolute.placement(None).unwrap().root(),
            PathBuf::from(&absolute.directory)
        );
    }
}
