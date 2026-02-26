use std::path::PathBuf;

const APP_NAME: &str = "kiosk";
const LOG_FILE_NAME: &str = "kiosk.log";

pub const DEFAULT_LOG_LEVEL: &str = "warn";

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_NAME)
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
    fn cache_dir_ends_with_app_name() {
        let dir = cache_dir();
        assert_eq!(dir.file_name().unwrap(), APP_NAME);
    }

    #[test]
    fn default_log_file_ends_with_log_filename() {
        let path = default_log_file();
        assert_eq!(path.file_name().unwrap(), LOG_FILE_NAME);
        assert!(path.parent().unwrap().ends_with(APP_NAME));
    }
}
