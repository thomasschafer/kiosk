mod config;
mod git;
mod tmux;
mod ui;

use anyhow::Result;
use ui::{App, OpenAction};

fn main() -> Result<()> {
    let config = config::load_config()?;
    let search_dirs = config.resolved_search_dirs();
    let repos = git::discover_repos(&search_dirs);

    let mut terminal = ratatui::init();
    let result = App::new(repos, config.session.split_command.clone()).run(&mut terminal);
    ratatui::restore();

    match result? {
        Some(OpenAction::Open {
            path,
            split_command,
        }) => {
            let session_name = tmux::session_name_for(&path);

            if !tmux::session_exists(&session_name) {
                tmux::create_session(&session_name, &path, split_command.as_deref());
            }

            tmux::switch_to_session(&session_name);
        }
        Some(OpenAction::Quit) | None => {}
    }

    Ok(())
}
