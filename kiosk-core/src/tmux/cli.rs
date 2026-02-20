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
            .args(["has-session", "-t", &format!("={name}")])
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
                .args([
                    "split-window",
                    "-h",
                    "-t",
                    &format!("={name}"),
                    "-c",
                    &dir_str,
                ])
                .status();
            let _ = Command::new("tmux")
                .args(["send-keys", "-t", &format!("={name}:0.1"), cmd, "Enter"])
                .status();
        }
    }

    fn switch_to_session(&self, name: &str) {
        if self.is_inside_tmux() {
            let _ = Command::new("tmux")
                .args(["switch-client", "-t", &format!("={name}")])
                .status();
        } else {
            let _ = Command::new("tmux")
                .args(["attach-session", "-t", &format!("={name}")])
                .status();
        }
    }

    fn is_inside_tmux(&self) -> bool {
        std::env::var("TMUX").is_ok()
    }
}
