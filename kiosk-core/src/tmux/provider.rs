use std::path::Path;

pub trait TmuxProvider {
    fn list_sessions(&self) -> Vec<String>;
    fn session_exists(&self, name: &str) -> bool;
    fn create_session(&self, name: &str, dir: &Path, split_command: Option<&str>);
    fn switch_to_session(&self, name: &str);
    fn is_inside_tmux(&self) -> bool;
    fn session_name_for(&self, path: &Path) -> String;
}
