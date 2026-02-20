use anyhow::Result;
use kiosk_core::{
    config,
    git::{CliGitProvider, GitProvider},
    state::AppState,
    tmux::CliTmuxProvider,
};
use kiosk_tui::OpenAction;

fn main() -> Result<()> {
    let config = config::load_config()?;
    let search_dirs = config.resolved_search_dirs();

    let git = CliGitProvider;
    let tmux = CliTmuxProvider;
    let repos = git.discover_repos(&search_dirs);
    let mut state = AppState::new(repos, config.session.split_command.clone());

    let mut terminal = ratatui::init();
    let result = kiosk_tui::run(&mut terminal, &mut state, &git, &tmux);
    ratatui::restore();

    match result? {
        Some(OpenAction::Open {
            path,
            split_command,
        }) => {
            use kiosk_core::tmux::TmuxProvider;
            let session_name = tmux.session_name_for(&path);

            if !tmux.session_exists(&session_name) {
                tmux.create_session(&session_name, &path, split_command.as_deref());
            }

            tmux.switch_to_session(&session_name);
        }
        Some(OpenAction::Quit) | None => {}
    }

    Ok(())
}
