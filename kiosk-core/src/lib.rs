pub mod action;
pub mod config;
pub mod constants;
pub mod event;
pub mod git;
pub mod keyboard;
pub mod state;
pub mod tmux;

// Re-export commonly used types at crate root
pub use action::Action;
pub use config::Config;
pub use event::AppEvent;
pub use git::{GitProvider, Repo, Worktree};
pub use keyboard::KeyEvent;
pub use state::{AppState, BranchEntry, Mode};
pub use tmux::TmuxProvider;
