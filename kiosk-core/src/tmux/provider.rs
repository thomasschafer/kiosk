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
    fn switch_to_session(&self, name: &str);
    fn kill_session(&self, name: &str);
    fn is_inside_tmux(&self) -> bool;
}
