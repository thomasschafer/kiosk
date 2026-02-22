use super::provider::TmuxProvider;
use anyhow::Result;
use std::path::Path;
use std::sync::Mutex;

#[derive(Default)]
pub struct MockTmuxProvider {
    pub sessions: Vec<String>,
    pub sessions_with_activity: Vec<(String, u64)>,
    pub inside_tmux: bool,
    pub killed_sessions: Mutex<Vec<String>>,
    pub created_sessions: Mutex<Vec<String>>,
    pub switched_sessions: Mutex<Vec<String>>,
    pub sent_keys: Mutex<Vec<(String, String)>>,
    pub piped_sessions: Mutex<Vec<(String, std::path::PathBuf)>>,
    pub clients: Vec<String>,
    pub capture_output: Mutex<String>,
    pub create_session_result: Mutex<Option<Result<()>>>,
    pub capture_pane_result: Mutex<Option<Result<String>>>,
    pub send_keys_result: Mutex<Option<Result<()>>>,
    pub pipe_pane_result: Mutex<Option<Result<()>>>,
}

impl TmuxProvider for MockTmuxProvider {
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
        name: &str,
        _dir: &Path,
        _split_command: Option<&str>,
    ) -> anyhow::Result<()> {
        self.created_sessions.lock().unwrap().push(name.to_string());
        self.create_session_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn capture_pane(&self, _session: &str, _lines: usize) -> anyhow::Result<String> {
        self.capture_pane_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Ok(self.capture_output.lock().unwrap().clone()))
    }

    fn send_keys(&self, session: &str, keys: &str) -> anyhow::Result<()> {
        self.sent_keys
            .lock()
            .unwrap()
            .push((session.to_string(), keys.to_string()));
        self.send_keys_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn pipe_pane(&self, session: &str, log_path: &Path) -> anyhow::Result<()> {
        self.piped_sessions
            .lock()
            .unwrap()
            .push((session.to_string(), log_path.to_path_buf()));
        self.pipe_pane_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn list_clients(&self, _session: &str) -> Vec<String> {
        self.clients.clone()
    }

    fn switch_to_session(&self, name: &str) {
        self.switched_sessions
            .lock()
            .unwrap()
            .push(name.to_string());
    }

    fn kill_session(&self, name: &str) {
        self.killed_sessions.lock().unwrap().push(name.to_string());
    }

    fn is_inside_tmux(&self) -> bool {
        self.inside_tmux
    }
}
