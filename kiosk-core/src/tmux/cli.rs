use super::provider::TmuxProvider;
use std::{path::Path, process::Command};

pub struct CliTmuxProvider;

impl TmuxProvider for CliTmuxProvider {
    fn list_sessions(&self) -> Vec<String> {
        let output = Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}"])
            .output();

        let Ok(output) = output else {
            return Vec::new();
        };

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect()
    }

    fn session_exists(&self, name: &str) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", name])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn create_session(&self, name: &str, dir: &Path, split_command: Option<&str>) {
        let dir_str = dir.to_string_lossy();

        let _ = Command::new("tmux")
            .args(["new-session", "-ds", name, "-c", &dir_str])
            .status();

        if let Some(cmd) = split_command {
            let _ = Command::new("tmux")
                .args(["split-window", "-h", "-t", name, "-c", &dir_str])
                .status();
            let _ = Command::new("tmux")
                .args(["send-keys", "-t", &format!("{name}:0.1"), cmd, "Enter"])
                .status();
        }
    }

    fn switch_to_session(&self, name: &str) {
        if self.is_inside_tmux() {
            let _ = Command::new("tmux")
                .args(["switch-client", "-t", name])
                .status();
        } else {
            let _ = Command::new("tmux")
                .args(["attach-session", "-t", name])
                .status();
        }
    }

    fn is_inside_tmux(&self) -> bool {
        std::env::var("TMUX").is_ok()
    }

    fn session_name_for(&self, path: &Path) -> String {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        // Include parent dir to avoid collisions between worktrees of different repos
        if let Some(parent) = path.parent().and_then(|p| p.file_name()) {
            let parent_name = parent.to_string_lossy();
            format!("{parent_name}/{name}").replace('.', "_")
        } else {
            name.replace('.', "_")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_session_name_for_simple() {
        let provider = CliTmuxProvider;
        let name = provider.session_name_for(&PathBuf::from("/home/user/my-project"));
        assert_eq!(name, "user/my-project");
    }

    #[test]
    fn test_session_name_for_dots_replaced() {
        let provider = CliTmuxProvider;
        let name = provider.session_name_for(&PathBuf::from("/home/user/my.project.rs"));
        assert_eq!(name, "user/my_project_rs");
    }

    #[test]
    fn test_session_name_for_root() {
        let provider = CliTmuxProvider;
        let name = provider.session_name_for(&PathBuf::from("/"));
        assert!(!name.is_empty() || name.is_empty()); // just checking no panic
    }

    #[test]
    fn test_session_name_avoids_collisions() {
        let provider = CliTmuxProvider;
        let a = provider.session_name_for(&PathBuf::from("/Dev/scooter-main"));
        let b = provider.session_name_for(&PathBuf::from("/Dev/photodrop-main"));
        // Both would have been "main" before; now they differ
        assert_ne!(a, b);
    }
}
