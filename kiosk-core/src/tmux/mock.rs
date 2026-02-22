use super::provider::TmuxProvider;
use std::path::Path;
use std::sync::Mutex;

#[derive(Default)]
pub struct MockTmuxProvider {
    pub sessions: Vec<String>,
    pub sessions_with_activity: Vec<(String, u64)>,
    pub inside_tmux: bool,
    pub killed_sessions: Mutex<Vec<String>>,
}

impl TmuxProvider for MockTmuxProvider {
    fn list_sessions(&self) -> Vec<String> {
        self.sessions.clone()
    }

    fn list_sessions_with_activity(&self) -> Vec<(String, u64)> {
        if self.sessions_with_activity.is_empty() {
            // Fall back to sessions with timestamp 0
            self.sessions.iter().map(|s| (s.clone(), 0)).collect()
        } else {
            self.sessions_with_activity.clone()
        }
    }

    fn session_exists(&self, name: &str) -> bool {
        self.sessions.contains(&name.to_string())
    }

    fn create_session(
        &self,
        _name: &str,
        _dir: &Path,
        _split_command: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn switch_to_session(&self, _name: &str) {}

    fn kill_session(&self, name: &str) {
        self.killed_sessions.lock().unwrap().push(name.to_string());
    }

    fn is_inside_tmux(&self) -> bool {
        self.inside_tmux
    }
}
