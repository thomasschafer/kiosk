use std::path::Path;

pub trait TmuxProvider: Send + Sync {
    /// List sessions with their last activity timestamp (`session_name`, `unix_timestamp`)
    fn list_sessions_with_activity(&self) -> Vec<(String, u64)>;
    fn session_exists(&self, name: &str) -> bool;
    fn create_session(
        &self,
        name: &str,
        dir: &Path,
        split_command: Option<&str>,
    ) -> anyhow::Result<()>;
    fn capture_pane(&self, session: &str, lines: usize) -> anyhow::Result<String>;
    fn send_keys(&self, session: &str, keys: &str) -> anyhow::Result<()>;
    fn pipe_pane(&self, session: &str, log_path: &Path) -> anyhow::Result<()>;
    fn list_clients(&self, session: &str) -> Vec<String>;
    fn switch_to_session(&self, name: &str);
    fn kill_session(&self, name: &str);
    fn is_inside_tmux(&self) -> bool;
}
