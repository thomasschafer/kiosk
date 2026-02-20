use std::{path::Path, process::Command};

/// Check if we're inside a tmux session
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Get list of active tmux session names
pub fn list_sessions() -> Vec<String> {
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

/// Derive a tmux session name from a path
pub fn session_name_for(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .replace('.', "_")
}

/// Check if a session with this name exists
pub fn session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Create a new tmux session (detached) and optionally split + run a command
pub fn create_session(name: &str, dir: &Path, split_command: Option<&str>) {
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

/// Switch to (or attach to) a session
pub fn switch_to_session(name: &str) {
    if is_inside_tmux() {
        let _ = Command::new("tmux")
            .args(["switch-client", "-t", name])
            .status();
    } else {
        let _ = Command::new("tmux")
            .args(["attach-session", "-t", name])
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_session_name_for_simple() {
        let name = session_name_for(&PathBuf::from("/home/user/my-project"));
        assert_eq!(name, "my-project");
    }

    #[test]
    fn test_session_name_for_dots_replaced() {
        let name = session_name_for(&PathBuf::from("/home/user/my.project.rs"));
        assert_eq!(name, "my_project_rs");
    }

    #[test]
    fn test_session_name_for_root() {
        let name = session_name_for(&PathBuf::from("/"));
        // Should not panic
        assert!(!name.is_empty() || name.is_empty()); // just checking no panic
    }
}
