use std::path::Path;

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
    /// Send keys to the target session's primary pane.
    ///
    /// Implementations may append `Enter` to submit the provided keys as a command.
    fn send_keys(&self, session: &str, keys: &str) -> anyhow::Result<()>;
    fn pipe_pane(&self, session: &str, log_path: &Path) -> anyhow::Result<()>;
    fn list_clients(&self, session: &str) -> Vec<String>;
    fn switch_to_session(&self, name: &str);
    fn kill_session(&self, name: &str);
    fn is_inside_tmux(&self) -> bool;
}
