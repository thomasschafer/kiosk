//! Carrying over the config of herdr-kiosk's earlier app mod, which kept it under the
//! herdr-kiosk name.

use std::{fs, io, path::Path};

use kiosk_core::{config::resolve_config_path, product::Product};

/// Where the earlier mod kept its config: the same place, under herdr-kiosk's name.
const EARLIER: Product = Product {
    name: "herdr-kiosk",
    ..crate::PRODUCT
};

/// Copies the earlier mod's config to where this product reads it, if this product has
/// none yet. Returns a message for the person when it did; the earlier file is left as it
/// was.
pub fn carry_over_earlier_config(
    get_env: impl Fn(&str) -> Option<String>,
) -> io::Result<Option<String>> {
    let (Some(current), Some(earlier)) = (
        resolve_config_path(&crate::PRODUCT, &get_env).path,
        resolve_config_path(&EARLIER, &get_env).path,
    ) else {
        return Ok(None);
    };
    copy_if_missing(&earlier, &current)
}

fn copy_if_missing(earlier: &Path, current: &Path) -> io::Result<Option<String>> {
    if current.exists() || !earlier.is_file() {
        return Ok(None);
    }
    if let Some(parent) = current.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(earlier, current)?;
    Ok(Some(format!(
        "Kiosk now keeps its config in {}; it copied yours from {}.",
        kiosk_core::path::display(current),
        kiosk_core::path::display(earlier)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(home: &Path) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| (name == "XDG_CONFIG_HOME").then(|| home.display().to_string())
    }

    #[test]
    fn the_earlier_config_is_copied_once_and_left_in_place() {
        let config = tempfile::tempdir().unwrap();
        let earlier = config.path().join("herdr-kiosk/config.toml");
        fs::create_dir_all(earlier.parent().unwrap()).unwrap();
        fs::write(&earlier, "search_dirs = [\"~/code\"]\n").unwrap();

        let message = carry_over_earlier_config(env(config.path()))
            .unwrap()
            .unwrap();
        let current = config.path().join("herdr-app-kiosk/config.toml");
        assert_eq!(
            fs::read_to_string(&current).unwrap(),
            "search_dirs = [\"~/code\"]\n"
        );
        assert!(message.contains("copied yours"), "{message}");
        assert!(earlier.is_file());

        fs::write(&current, "search_dirs = [\"~/work\"]\n").unwrap();
        assert_eq!(carry_over_earlier_config(env(config.path())).unwrap(), None);
        assert_eq!(
            fs::read_to_string(&current).unwrap(),
            "search_dirs = [\"~/work\"]\n",
            "an existing config is never replaced"
        );
    }

    #[test]
    fn without_an_earlier_config_nothing_happens() {
        let config = tempfile::tempdir().unwrap();
        assert_eq!(carry_over_earlier_config(env(config.path())).unwrap(), None);
        assert!(!config.path().join("herdr-app-kiosk").exists());
    }
}
