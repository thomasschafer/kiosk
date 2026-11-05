//! Notifications the picker can't show because its popup has closed, such as an `[on_open]`
//! pane that failed to open after it reported the repository opened. They are saved in the
//! app's home, one JSON object per line, for the mod to show the next time it opens.

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use serde::Serialize;

/// Where the notices go, below the app's home. The mod reads the same file (`SAVED_NOTICES` in
/// `mods/kiosk/main.js`).
pub const NOTICE_FILE: &str = "data/kiosk/notices.jsonl";

pub trait LaterNotices: Send + Sync {
    fn save(&self, title: &str, body: &str) -> Result<(), String>;
}

/// The file in the app's home that the mod reads.
pub struct NoticeFile {
    path: PathBuf,
}

impl NoticeFile {
    pub fn in_app_home(app_home: &Path) -> Self {
        Self {
            path: app_home.join(NOTICE_FILE),
        }
    }
}

#[derive(Serialize)]
struct Notice<'a> {
    title: &'a str,
    body: &'a str,
}

impl LaterNotices for NoticeFile {
    fn save(&self, title: &str, body: &str) -> Result<(), String> {
        let failed = |error: std::io::Error| format!("saving to {}: {error}", self.path.display());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        let mut line =
            serde_json::to_string(&Notice { title, body }).expect("a notice always serializes");
        line.push('\n');
        // One write per notice, appended, so notices saved at once by two pickers stay whole.
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .map_err(failed)
    }
}

/// For a picker started without the app's home, which has no file the mod reads: the notice
/// goes to the picker's log instead.
pub struct LogNotices;

impl LaterNotices for LogNotices {
    fn save(&self, title: &str, body: &str) -> Result<(), String> {
        eprintln!("{title}: {body}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn notices_are_appended_to_the_file_the_mod_reads_as_json_lines() {
        let home = TempDir::new().unwrap();
        let notices = NoticeFile::in_app_home(home.path());

        notices.save("First", "one").unwrap();
        notices.save("Second", "two \"quoted\"").unwrap();

        let text = std::fs::read_to_string(home.path().join(NOTICE_FILE)).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                json!({ "title": "First", "body": "one" }),
                json!({ "title": "Second", "body": "two \"quoted\"" }),
            ]
        );
    }
}
