use anyhow::Result;
use clap::Parser;
use kiosk_core::{
    config,
    git::{CliGitProvider, GitProvider},
    state::AppState,
    tmux::CliTmuxProvider,
};
use kiosk_tui::{OpenAction, Theme};
use std::sync::Arc;

#[derive(Parser)]
#[command(version, about = "Tmux session manager with worktree support")]
struct Cli {
    /// Path to config file (default: ~/.config/kiosk/config.toml)
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::load_config(cli.config.as_deref())?;
    let search_dirs = config.resolved_search_dirs();

    let git: Arc<dyn GitProvider> = Arc::new(CliGitProvider);
    let tmux = CliTmuxProvider;
    let repos = git.discover_repos(&search_dirs);
    let mut state = AppState::new(repos, config.session.split_command.clone());

    let theme = Theme::from_config(&config.theme);

    let mut terminal = ratatui::init();
    let result = kiosk_tui::run(&mut terminal, &mut state, &git, &tmux, &theme);
    ratatui::restore();

    match result? {
        Some(OpenAction::Open {
            path,
            session_name,
            split_command,
        }) => {
            use kiosk_core::tmux::TmuxProvider;

            if !tmux.session_exists(&session_name) {
                tmux.create_session(&session_name, &path, split_command.as_deref());
            }

            tmux.switch_to_session(&session_name);
        }
        Some(OpenAction::Quit) | None => {}
    }

    Ok(())
}
