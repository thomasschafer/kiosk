use anyhow::Result;
use clap::{Parser, Subcommand};
use kiosk_core::{
    config,
    constants::{GITDIR_FILE_PREFIX, GIT_DIR_ENTRY, WORKTREE_DIR_NAME},
    git::{CliGitProvider, GitProvider},
    state::AppState,
    tmux::CliTmuxProvider,
};
use kiosk_tui::{OpenAction, Theme};
use std::{fs, io, path::Path, process::Command, sync::Arc};

#[derive(Parser)]
#[command(version, about = "Tmux session manager with worktree support")]
struct Cli {
    /// Path to config file (default: ~/.config/kiosk/config.toml)
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Clean up orphaned worktree directories
    Clean {
        /// List orphaned worktrees without removing them
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::load_config(cli.config.as_deref())?;

    match cli.command {
        Some(Commands::Clean { dry_run }) => {
            let search_dirs = config.resolved_search_dirs();
            clean_orphaned_worktrees(&search_dirs, dry_run)?;
        }
        None => {
            run_tui(&config)?;
        }
    }

    Ok(())
}

fn run_tui(config: &config::Config) -> Result<()> {
    let search_dirs = config.resolved_search_dirs();

    let git: Arc<dyn GitProvider> = Arc::new(CliGitProvider);
    let tmux = CliTmuxProvider;
    let mut state =
        AppState::new_loading("Discovering repos...", config.session.split_command.clone());

    let theme = Theme::from_config(&config.theme);

    let mut terminal = ratatui::init();
    let result = kiosk_tui::run(
        &mut terminal,
        &mut state,
        &git,
        &tmux,
        &theme,
        &config.keys,
        search_dirs,
    );
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

fn clean_orphaned_worktrees(
    search_dirs: &[(std::path::PathBuf, u16)],
    dry_run: bool,
) -> Result<()> {
    let mut orphaned_worktrees = Vec::new();

    // Scan all search directories for .kiosk_worktrees directories
    for (search_dir, _) in search_dirs {
        let worktrees_dir = search_dir.join(WORKTREE_DIR_NAME);
        if !worktrees_dir.exists() {
            continue;
        }

        // Scan all potential worktree directories
        if let Ok(entries) = fs::read_dir(&worktrees_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && is_orphaned_worktree(&path) {
                    orphaned_worktrees.push(path);
                }
            }
        }
    }

    if orphaned_worktrees.is_empty() {
        println!("No orphaned worktrees found.");
        return Ok(());
    }

    println!("Found {} orphaned worktree(s):", orphaned_worktrees.len());
    for worktree in &orphaned_worktrees {
        println!("  {}", worktree.display());
    }

    if dry_run {
        println!("\n(Dry run - no changes made. Run without --dry-run to remove them.)");
        return Ok(());
    }

    // Prompt for confirmation
    print!("\nRemove these orphaned worktrees? (y/N): ");
    io::Write::flush(&mut io::stdout())?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if input.trim().to_lowercase() != "y" {
        println!("Cancelled.");
        return Ok(());
    }

    // Remove the worktrees
    for worktree in orphaned_worktrees {
        match remove_worktree(&worktree) {
            Ok(()) => println!("Removed: {}", worktree.display()),
            Err(e) => eprintln!("Failed to remove {}: {}", worktree.display(), e),
        }
    }

    Ok(())
}

fn is_orphaned_worktree(path: &Path) -> bool {
    let git_file = path.join(GIT_DIR_ENTRY);

    // If there's no .git file, it's definitely orphaned
    if !git_file.exists() {
        return true;
    }

    // Try to read the .git file to get the repository path
    let Ok(git_content) = fs::read_to_string(&git_file) else {
        return true; // Can't read .git file, treat as orphaned
    };

    // .git file should contain "gitdir: /path/to/repo/.git/worktrees/name"
    let Some(gitdir_line) = git_content
        .lines()
        .find(|line| line.starts_with(GITDIR_FILE_PREFIX))
    else {
        return true; // Malformed .git file
    };

    let gitdir_path = gitdir_line.strip_prefix(GITDIR_FILE_PREFIX).unwrap_or("");
    let gitdir = Path::new(gitdir_path);

    // Check if the gitdir path exists and is valid
    if !gitdir.exists() {
        return true; // Referenced git directory doesn't exist
    }

    // Check if there's a valid worktree entry in the main repo
    if let Some(main_git_dir) = gitdir.parent()
        && let Some(worktrees_dir) = main_git_dir.parent()
        && worktrees_dir.join("HEAD").exists()
    {
        return false; // This appears to be a valid worktree
    }

    true // Couldn't validate the worktree, treat as orphaned
}

fn remove_worktree(path: &Path) -> Result<()> {
    // First try to use git worktree remove if possible
    let output = Command::new("git")
        .args(["worktree", "remove", &path.to_string_lossy()])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            return Ok(()); // Successfully removed with git
        }
        _ => {
            // Fall back to directory removal
            fs::remove_dir_all(path)?;
        }
    }

    Ok(())
}
