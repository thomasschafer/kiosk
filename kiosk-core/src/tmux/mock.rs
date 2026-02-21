use super::provider::TmuxProvider;
use std::path::Path;

#[derive(Default)]
pub struct MockTmuxProvider {
    pub sessions: Vec<String>,
    pub inside_tmux: bool,
}

impl TmuxProvider for MockTmuxProvider {
    fn list_sessions(&self) -> Vec<String> {
        self.sessions.clone()
    }

    fn session_exists(&self, name: &str) -> bool {
        self.sessions.contains(&name.to_string())
    }

    fn create_session(&self, _name: &str, _dir: &Path, _split_command: Option<&str>) {}

    fn switch_to_session(&self, _name: &str) {}
    fn kill_session(&self, _name: &str) {}

    fn is_inside_tmux(&self) -> bool {
        self.inside_tmux
    }
}
