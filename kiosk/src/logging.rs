use std::path::PathBuf;

const APP_NAME: &str = "kiosk";
const LOG_FILE_NAME: &str = "kiosk.log";

pub const DEFAULT_LOG_LEVEL: &str = "warn";

pub fn cache_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Ok(xdg_cache_home) = std::env::var("XDG_CACHE_HOME")
            && !xdg_cache_home.is_empty()
        {
            return PathBuf::from(xdg_cache_home).join(APP_NAME);
        }
        dirs::home_dir()
            .expect("Unable to find home directory")
            .join(".cache")
            .join(APP_NAME)
    }
    #[cfg(windows)]
    {
        if let Some(cache) = dirs::cache_dir() {
            cache.join(APP_NAME)
        } else {
            std::env::temp_dir().join(APP_NAME)
        }
    }
}

pub fn default_log_file() -> PathBuf {
    cache_dir().join(LOG_FILE_NAME)
}

pub fn setup_logging(level: log::LevelFilter) -> anyhow::Result<()> {
    let log_file = default_log_file();
    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    simple_log::file(log_file.to_string_lossy().into_owned(), level, 10, 10)
        .map_err(|e| anyhow::anyhow!(e))?;
    log::info!("kiosk logging initialised (level={level})");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_respects_xdg_override() {
        let dir = tempfile::tempdir().unwrap();
        let custom = dir.path().join("custom-cache");

        unsafe { std::env::set_var("XDG_CACHE_HOME", &custom) };
        let result = cache_dir();
        unsafe { std::env::remove_var("XDG_CACHE_HOME") };

        assert_eq!(result, custom.join(APP_NAME));
    }

    #[test]
    fn cache_dir_ignores_empty_xdg() {
        unsafe { std::env::set_var("XDG_CACHE_HOME", "") };
        let result = cache_dir();
        unsafe { std::env::remove_var("XDG_CACHE_HOME") };

        assert!(
            result.ends_with(format!(".cache/{APP_NAME}").as_str()),
            "expected default .cache/kiosk path, got: {result:?}"
        );
    }

    #[test]
    fn default_log_file_ends_with_log_filename() {
        let path = default_log_file();
        assert_eq!(path.file_name().unwrap(), LOG_FILE_NAME);
        assert!(path.parent().unwrap().ends_with(APP_NAME));
    }
}
