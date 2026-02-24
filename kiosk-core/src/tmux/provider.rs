use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub window_index: u32,
    pub pane_index: u32,
    pub command: String,
    pub pid: u32,
}

pub trait TmuxProvider: Send + Sync {
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
    fn capture_pane_by_index(
        &self,
        session: &str,
        window_index: u32,
        pane_index: u32,
        lines: u32,
    ) -> Option<String>;
}
