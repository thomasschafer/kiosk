use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const APP_NAME: &str = "kiosk";
const PENDING_DELETE_FILE_NAME: &str = "pending_deletes.toml";
const PENDING_DELETE_STATE_VERSION: u32 = 1;
const PENDING_DELETE_TTL_SECS: u64 = 60 * 60 * 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingWorktreeDelete {
    pub repo_path: PathBuf,
    pub branch_name: String,
    pub worktree_path: PathBuf,
    pub started_at_unix_secs: u64,
}

impl PendingWorktreeDelete {
    pub fn new(repo_path: PathBuf, branch_name: String, worktree_path: PathBuf) -> Self {
        Self {
            repo_path,
            branch_name,
            worktree_path,
            started_at_unix_secs: now_unix_secs(),
        }
    }

    pub fn is_expired(&self) -> bool {
        let age_secs = now_unix_secs().saturating_sub(self.started_at_unix_secs);
        age_secs > PENDING_DELETE_TTL_SECS
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PendingDeleteFile {
    version: u32,
    entries: Vec<PendingWorktreeDelete>,
}

fn state_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME")
            && !xdg_state_home.is_empty()
        {
            return PathBuf::from(xdg_state_home).join(APP_NAME);
        }
        dirs::home_dir()
            .expect("Unable to find home directory")
            .join(".local")
            .join("state")
            .join(APP_NAME)
    }
    #[cfg(windows)]
    {
        if let Some(local_data) = dirs::data_local_dir() {
            local_data.join(APP_NAME)
        } else {
            std::env::temp_dir().join(APP_NAME)
        }
    }
}

fn state_file() -> PathBuf {
    state_dir().join(PENDING_DELETE_FILE_NAME)
}

pub fn load_pending_worktree_deletes() -> Vec<PendingWorktreeDelete> {
    let file_path = state_file();
    let Ok(contents) = fs::read_to_string(&file_path) else {
        return Vec::new();
    };

    let Ok(parsed) = toml::from_str::<PendingDeleteFile>(&contents) else {
        return Vec::new();
    };

    if parsed.version != PENDING_DELETE_STATE_VERSION {
        return Vec::new();
    }

    parsed
        .entries
        .into_iter()
        .filter(|entry| !entry.is_expired())
        .collect()
}

pub fn save_pending_worktree_deletes(entries: &[PendingWorktreeDelete]) -> Result<()> {
    let state_dir = state_dir();
    fs::create_dir_all(&state_dir)?;

    let file_path = state_file();
    if entries.is_empty() {
        if file_path.exists() {
            fs::remove_file(file_path)?;
        }
        return Ok(());
    }

    let state = PendingDeleteFile {
        version: PENDING_DELETE_STATE_VERSION,
        entries: entries.to_vec(),
    };
    let serialized = toml::to_string(&state)?;
    fs::write(file_path, serialized)?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pending_delete_expiry() {
        let entry = PendingWorktreeDelete {
            repo_path: PathBuf::from("/tmp/repo"),
            branch_name: "dev".to_string(),
            worktree_path: PathBuf::from("/tmp/repo-dev"),
            started_at_unix_secs: 0,
        };
        assert!(entry.is_expired());
    }
}
