use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane_id: String,
    pub command: String,
    pub pid: u32,
}

/// Pre-fetched pane info and session metadata for a single session.
/// Returned by [`TmuxProvider::list_all_panes_with_activity`] to enable
/// batched agent detection without per-session tmux subprocess calls.
#[derive(Debug, Clone)]
pub struct SessionPaneData {
    pub panes: Vec<PaneInfo>,
    pub session_activity: u64,
}

pub trait TmuxProvider: Send + Sync {
    /// List all panes across all sessions in a single call, along with each
    /// session's last activity timestamp.
    ///
    /// This batches what would otherwise be N `list_panes_detailed` + N
    /// `session_activity` calls into a single tmux invocation, significantly
    /// reducing subprocess overhead during agent status polling.
    fn list_all_panes_with_activity(&self) -> std::collections::HashMap<String, SessionPaneData>;
    /// List sessions with their last activity timestamp (`session_name`, `unix_timestamp`)
    fn list_sessions_with_activity(&self) -> Vec<(String, u64)>;
    /// List session names only, discarding activity timestamps.
    fn list_session_names(&self) -> Vec<String> {
        self.list_sessions_with_activity()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }
    fn session_exists(&self, name: &str) -> bool;
    fn create_session(
        &self,
        name: &str,
        dir: &Path,
        split_command: Option<&str>,
    ) -> anyhow::Result<()>;
    fn capture_pane(&self, session: &str, lines: usize) -> anyhow::Result<String>;
    /// Capture pane output for a specific pane.
    fn capture_pane_with_pane(
        &self,
        session: &str,
        pane: &str,
        lines: usize,
    ) -> anyhow::Result<String>;
    /// Get the current command running in a specific pane.
    fn pane_current_command(&self, session: &str, pane: &str) -> anyhow::Result<String>;
    /// Get session activity timestamp.
    fn session_activity(&self, session: &str) -> anyhow::Result<u64>;
    /// Get pane count for a session.
    fn pane_count(&self, session: &str) -> anyhow::Result<usize>;
    /// Send keys to the target session's primary pane.
    ///
    /// Implementations always append `Enter` after the supplied keys to execute
    /// them as a command.
    fn send_keys(&self, session: &str, keys: &str) -> anyhow::Result<()>;
    /// Send tmux key names (e.g. C-c, Escape, Enter) to the target pane WITHOUT auto-appending Enter.
    fn send_keys_raw(&self, session: &str, pane: &str, keys: &[&str]) -> anyhow::Result<()>;
    /// Send literal text to the target pane WITHOUT auto-appending Enter.
    fn send_text_raw(&self, session: &str, pane: &str, text: &str) -> anyhow::Result<()>;
    fn pipe_pane(&self, session: &str, log_path: &Path) -> anyhow::Result<()>;
    fn list_clients(&self, session: &str) -> Vec<String>;
    fn switch_to_session(&self, name: &str);
    fn kill_session(&self, name: &str);
    fn is_inside_tmux(&self) -> bool;
    fn list_panes_detailed(&self, session: &str) -> Vec<PaneInfo>;
    fn capture_pane_content(&self, pane_id: &str, lines: u32) -> Option<String>;
}
