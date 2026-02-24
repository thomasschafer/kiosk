use super::provider::{PaneInfo, TmuxProvider};
use anyhow::{Context, Result, bail};
use std::{path::Path, process::Command};

pub struct CliTmuxProvider;

fn create_session_commands(
    name: &str,
    dir_str: &str,
    split_command: Option<&str>,
) -> Vec<Vec<String>> {
    let mut commands = vec![vec![
        "new-session".to_string(),
        "-ds".to_string(),
        name.to_string(),
        "-c".to_string(),
        dir_str.to_string(),
    ]];

    if let Some(cmd) = split_command.filter(|cmd| !cmd.trim().is_empty()) {
        commands.push(vec![
            "split-window".to_string(),
            "-h".to_string(),
            "-t".to_string(),
            format!("={name}:0"),
            "-c".to_string(),
            dir_str.to_string(),
            cmd.to_string(),
        ]);
    }

    commands
}

impl TmuxProvider for CliTmuxProvider {
    fn list_sessions_with_activity(&self) -> Vec<(String, u64)> {
        let output = Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}:#{session_activity}"])
            .output();

        let Ok(output) = output else {
            return Vec::new();
        };

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let (name, ts) = line.rsplit_once(':')?;
                let ts = ts.parse::<u64>().ok()?;
                Some((name.to_string(), ts))
            })
            .collect()
    }

    fn session_exists(&self, name: &str) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", &format!("={name}")])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn create_session(&self, name: &str, dir: &Path, split_command: Option<&str>) -> Result<()> {
        let dir_str = dir.to_string_lossy();

        for args in create_session_commands(name, &dir_str, split_command) {
            let output = Command::new("tmux")
                .args(&args)
                .output()
                .with_context(|| format!("failed to execute tmux {}", args.join(" ")))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("tmux {} failed: {}", args.join(" "), stderr.trim());
            }
        }

        Ok(())
    }

    fn capture_pane(&self, session: &str, lines: usize) -> Result<String> {
        let target = format!("={session}:0.0");
        let output = Command::new("tmux")
            .args([
                "capture-pane",
                "-t",
                &target,
                "-p",
                "-S",
                &format!("-{lines}"),
            ])
            .output()
            .with_context(|| {
                format!("failed to execute tmux capture-pane for session {session}")
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux capture-pane failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn send_keys(&self, session: &str, keys: &str) -> Result<()> {
        let target = format!("={session}:0.0");
        // Use -l (literal) so tmux doesn't interpret words like "Enter" or "Escape"
        // as special key names, then send Enter separately to submit.
        let literal = Command::new("tmux")
            .args(["send-keys", "-t", &target, "-l", keys])
            .output()
            .with_context(|| format!("failed to execute tmux send-keys for session {session}"))?;
        if !literal.status.success() {
            let stderr = String::from_utf8_lossy(&literal.stderr);
            bail!("tmux send-keys failed: {}", stderr.trim());
        }
        let enter = Command::new("tmux")
            .args(["send-keys", "-t", &target, "Enter"])
            .output()
            .with_context(|| {
                format!("failed to execute tmux send-keys Enter for session {session}")
            })?;
        if !enter.status.success() {
            let stderr = String::from_utf8_lossy(&enter.stderr);
            bail!("tmux send-keys Enter failed: {}", stderr.trim());
        }
        Ok(())
    }

    fn send_keys_raw(&self, session: &str, pane: &str, keys: &[&str]) -> Result<()> {
        let target = format!("={session}:0.{pane}");
        let mut args = vec!["send-keys", "-t", &target];
        args.extend(keys);

        let output = Command::new("tmux").args(&args).output().with_context(|| {
            format!("failed to execute tmux send-keys for session {session} pane {pane}")
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux send-keys failed: {}", stderr.trim());
        }
        Ok(())
    }

    fn send_text_raw(&self, session: &str, pane: &str, text: &str) -> Result<()> {
        let target = format!("={session}:0.{pane}");
        let output = Command::new("tmux")
            .args(["send-keys", "-t", &target, "-l", text])
            .output()
            .with_context(|| {
                format!("failed to execute tmux send-keys for session {session} pane {pane}")
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux send-keys failed: {}", stderr.trim());
        }
        Ok(())
    }

    fn capture_pane_with_pane(&self, session: &str, pane: &str, lines: usize) -> Result<String> {
        let target = format!("={session}:0.{pane}");
        let output = Command::new("tmux")
            .args([
                "capture-pane",
                "-t",
                &target,
                "-p",
                "-S",
                &format!("-{lines}"),
            ])
            .output()
            .with_context(|| {
                format!("failed to execute tmux capture-pane for session {session} pane {pane}")
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux capture-pane failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn pane_current_command(&self, session: &str, pane: &str) -> Result<String> {
        let target = format!("={session}:0.{pane}");
        let output = Command::new("tmux")
            .args([
                "display-message",
                "-t",
                &target,
                "-p",
                "#{pane_current_command}",
            ])
            .output()
            .with_context(|| {
                format!("failed to execute tmux display-message for session {session} pane {pane}")
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux display-message failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn session_activity(&self, session: &str) -> Result<u64> {
        let output = Command::new("tmux")
            .args([
                "display-message",
                "-t",
                &format!("={session}"),
                "-p",
                "#{session_activity}",
            ])
            .output()
            .with_context(|| {
                format!("failed to execute tmux display-message for session {session}")
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux display-message failed: {}", stderr.trim());
        }
        let output_str = String::from_utf8_lossy(&output.stdout);
        let activity_str = output_str.trim();
        activity_str
            .parse::<u64>()
            .with_context(|| format!("failed to parse session activity timestamp: {activity_str}"))
    }

    fn pane_count(&self, session: &str) -> Result<usize> {
        let output = Command::new("tmux")
            .args([
                "list-panes",
                "-t",
                &format!("={session}"),
                "-F",
                "#{pane_index}",
            ])
            .output()
            .with_context(|| format!("failed to execute tmux list-panes for session {session}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux list-panes failed: {}", stderr.trim());
        }

        let pane_count = String::from_utf8_lossy(&output.stdout).lines().count();
        Ok(pane_count)
    }

    fn pipe_pane(&self, session: &str, log_path: &Path) -> Result<()> {
        let target = format!("={session}:0.0");
        let escaped_path = log_path.to_string_lossy().replace('\'', "'\\''");
        let command = format!("cat >> '{escaped_path}'");
        let output = Command::new("tmux")
            .args(["pipe-pane", "-t", &target, "-o", &command])
            .output()
            .with_context(|| format!("failed to execute tmux pipe-pane for session {session}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux pipe-pane failed: {}", stderr.trim());
        }
        Ok(())
    }

    fn list_clients(&self, session: &str) -> Vec<String> {
        let output = Command::new("tmux")
            .args([
                "list-clients",
                "-t",
                &format!("={session}"),
                "-F",
                "#{client_tty}",
            ])
            .output();
        let Ok(output) = output else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(ToString::to_string)
            .collect()
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

    fn kill_session(&self, name: &str) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &format!("={name}")])
            .status();
    }

    fn is_inside_tmux(&self) -> bool {
        std::env::var("TMUX").is_ok()
    }

    fn list_panes_detailed(&self, session: &str) -> Vec<PaneInfo> {
        let output = Command::new("tmux")
            .args([
                "list-panes",
                "-t",
                &format!("={session}"),
                "-F",
                "#{pane_index}|#{pane_current_command}|#{pane_pid}",
            ])
            .output();

        let Ok(output) = output else {
            return Vec::new();
        };

        if !output.status.success() {
            return Vec::new();
        }

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_pane_line)
            .collect()
    }

    fn capture_pane_by_index(&self, session: &str, pane_index: u32, lines: u32) -> Option<String> {
        let output = Command::new("tmux")
            .args([
                "capture-pane",
                "-t",
                &format!("={session}:.{pane_index}"),
                "-p",
                "-S",
                &format!("-{lines}"),
            ])
            .output()
            .ok()?;

        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            None
        }
    }
}

/// Parse a single line of tmux list-panes output in the format:
/// `{pane_index}|{pane_current_command}|{pane_pid}`
fn parse_pane_line(line: &str) -> Option<PaneInfo> {
    let parts: Vec<&str> = line.splitn(3, '|').collect();
    if parts.len() == 3 {
        let pane_index = parts[0].parse().ok()?;
        let command = parts[1].to_string();
        let pid = parts[2].parse().ok()?;
        Some(PaneInfo {
            pane_index,
            command,
            pid,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{create_session_commands, parse_pane_line};

    #[test]
    fn test_create_session_commands_with_split_command_uses_split_window_command_arg() {
        let commands = create_session_commands("demo", "/tmp/demo", Some("hx"));
        assert_eq!(commands.len(), 2);

        assert_eq!(
            commands[0],
            vec![
                "new-session".to_string(),
                "-ds".to_string(),
                "demo".to_string(),
                "-c".to_string(),
                "/tmp/demo".to_string(),
            ]
        );
        assert_eq!(
            commands[1],
            vec![
                "split-window".to_string(),
                "-h".to_string(),
                "-t".to_string(),
                "=demo:0".to_string(),
                "-c".to_string(),
                "/tmp/demo".to_string(),
                "hx".to_string(),
            ]
        );
    }

    #[test]
    fn test_create_session_commands_without_split_command() {
        let commands = create_session_commands("demo", "/tmp/demo", None);
        assert_eq!(commands.len(), 1);
    }

    #[test]
    fn test_parse_pane_line_basic() {
        let info = parse_pane_line("0|bash|12345").unwrap();
        assert_eq!(info.pane_index, 0);
        assert_eq!(info.command, "bash");
        assert_eq!(info.pid, 12345);
    }

    #[test]
    fn test_parse_pane_line_complex_command() {
        let info = parse_pane_line("2|claude-code|99999").unwrap();
        assert_eq!(info.pane_index, 2);
        assert_eq!(info.command, "claude-code");
        assert_eq!(info.pid, 99999);
    }

    #[test]
    fn test_parse_pane_line_invalid_index() {
        assert!(parse_pane_line("abc|bash|12345").is_none());
    }

    #[test]
    fn test_parse_pane_line_invalid_pid() {
        assert!(parse_pane_line("0|bash|notapid").is_none());
    }

    #[test]
    fn test_parse_pane_line_too_few_fields() {
        assert!(parse_pane_line("0|bash").is_none());
        assert!(parse_pane_line("").is_none());
    }
}
