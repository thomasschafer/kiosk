//! Choices kiosk remembers between runs, beside the removals it has pending in the
//! product's state directory.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{config::resolve_trusted_file_path, pending_delete::write_atomic, product::Product};

const FILE_NAME: &str = "preferences.toml";

/// How the diff tab lays out a change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLayout {
    /// Old and new lines one after the other.
    #[default]
    Unified,
    /// Old lines on the left, new on the right.
    SideBySide,
}

impl DiffLayout {
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Unified => Self::SideBySide,
            Self::SideBySide => Self::Unified,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub diff_layout: DiffLayout,
}

/// Where `product` keeps its preferences, if it has a state directory.
pub fn file(product: &Product) -> Option<PathBuf> {
    file_with(product, &|name| std::env::var(name).ok())
}

fn file_with(product: &Product, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    resolve_trusted_file_path(
        product,
        product.state_dir_var,
        ("XDG_STATE_HOME", ".local/state"),
        &|name: &str| env(name),
        FILE_NAME,
        "state",
    )
    .0
}

/// The preferences saved in `path`, or the defaults when there are none. A file that can't
/// be read is an error to report, not something to overwrite.
pub fn load(path: &Path) -> Result<Preferences> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn save(path: &Path, preferences: Preferences) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    write_atomic(path, toml::to_string(&preferences)?.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::TEST_PRODUCT;

    #[test]
    fn preferences_live_in_the_products_state_directory() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().to_path_buf();
        let env =
            |name: &str| (name == "KIOSK_TEST_STATE_DIR").then(|| state.display().to_string());
        assert_eq!(file_with(&TEST_PRODUCT, &env), Some(state.join(FILE_NAME)));
    }

    #[test]
    fn nothing_saved_is_the_defaults_and_a_saved_layout_comes_back() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state").join(FILE_NAME);
        assert_eq!(load(&path).unwrap(), Preferences::default());
        let side_by_side = Preferences {
            diff_layout: DiffLayout::SideBySide,
        };
        save(&path, side_by_side).unwrap();
        assert_eq!(load(&path).unwrap(), side_by_side);
    }

    #[test]
    fn a_file_kiosk_cant_read_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(FILE_NAME);
        fs::write(&path, "diff_layout = \"sideways\"\n").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn toggling_switches_between_the_two_layouts() {
        assert_eq!(DiffLayout::Unified.toggled(), DiffLayout::SideBySide);
        assert_eq!(DiffLayout::SideBySide.toggled(), DiffLayout::Unified);
    }
}
