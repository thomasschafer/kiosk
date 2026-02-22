mod cli;

use anyhow::Result;
use clap::{Parser, Subcommand};
use kiosk_core::{
    config,
    constants::{GIT_DIR_ENTRY, GITDIR_FILE_PREFIX, WORKTREE_DIR_NAME},
    git::{CliGitProvider, GitProvider},
    pending_delete::load_pending_worktree_deletes,
    state::AppState,
    tmux::{CliTmuxProvider, TmuxProvider},
};
use kiosk_tui::{OpenAction, Theme};
use std::{fs, io, path::Path, process::Command, process::ExitCode, sync::Arc};

#[derive(Parser)]
#[command(version, about = "Tmux session manager with worktree support")]
struct Cli {
    /// Override path to config file
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
    /// List discovered repositories
    List {
        #[arg(long)]
        json: bool,
    },
    /// List branches for a repository
    Branches {
        repo: String,
        #[arg(long)]
        json: bool,
    },
    /// Open or create a worktree and tmux session
    Open {
        repo: String,
        branch: Option<String>,
        #[arg(long)]
        new_branch: Option<String>,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        no_switch: bool,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        log: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show status for a session
    Status {
        repo: String,
        branch: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        lines: Option<usize>,
    },
    /// List active kiosk sessions
    Sessions {
        #[arg(long)]
        json: bool,
    },
    /// Delete a worktree and session
    Delete {
        repo: String,
        branch: String,
        #[arg(long)]
        force: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_errors = command_wants_json(cli.command.as_ref());
    let config = match config::load_config(cli.config.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            let cli_error = crate::cli::CliError::system(error.to_string());
            crate::cli::print_error(&cli_error, json_errors);
            return ExitCode::from(2);
        }
    };

    let git: Arc<dyn GitProvider> = Arc::new(CliGitProvider);
    let tmux: Arc<dyn TmuxProvider> = Arc::new(CliTmuxProvider);

    let result = match cli.command {
        Some(Commands::Clean { dry_run }) => {
            let search_dirs = config.resolved_search_dirs();
            clean_orphaned_worktrees(&search_dirs, dry_run).map_err(crate::cli::CliError::from)
        }
        Some(Commands::List { json }) => crate::cli::cmd_list(&config, git.as_ref(), json),
        Some(Commands::Branches { repo, json }) => {
            crate::cli::cmd_branches(&config, git.as_ref(), tmux.as_ref(), &repo, json)
        }
        Some(Commands::Open {
            repo,
            branch,
            new_branch,
            base,
            no_switch,
            run,
            log,
            json,
        }) => {
            let args = crate::cli::OpenArgs {
                repo,
                branch,
                new_branch,
                base,
                no_switch,
                run,
                log,
                json,
            };
            crate::cli::cmd_open(&config, git.as_ref(), tmux.as_ref(), &args)
        }
        Some(Commands::Status {
            repo,
            branch,
            json,
            lines,
        }) => {
            let args = crate::cli::StatusArgs {
                repo,
                branch,
                json,
                lines,
            };
            crate::cli::cmd_status(&config, git.as_ref(), tmux.as_ref(), &args)
        }
        Some(Commands::Sessions { json }) => {
            crate::cli::cmd_sessions(&config, git.as_ref(), tmux.as_ref(), json)
        }
        Some(Commands::Delete {
            repo,
            branch,
            force,
        }) => {
            let args = crate::cli::DeleteArgs {
                repo,
                branch,
                force,
            };
            crate::cli::cmd_delete(&config, git.as_ref(), tmux.as_ref(), &args)
        }
        None => run_tui(&config).map_err(crate::cli::CliError::from),
    };

    match result {
        Ok(()) => ExitCode::from(0),
        Err(error) => {
            crate::cli::print_error(&error, json_errors);
            let code: u8 = match error.code() {
                1 => 1,
                _ => 2,
            };
            ExitCode::from(code)
        }
    }
}

fn run_tui(config: &config::Config) -> Result<()> {
    let search_dirs = config.resolved_search_dirs();

    let git: Arc<dyn GitProvider> = Arc::new(CliGitProvider);
    let tmux: Arc<dyn TmuxProvider> = Arc::new(CliTmuxProvider);

    // Detect CWD repo/worktree for instant display and ordering.
    // cwd_worktree_path: the toplevel of whatever git tree the user is in (main repo or worktree)
    // current_repo_path: the main repo root (resolved through worktree .git pointers)
    let cwd_worktree_path = git
        .resolve_repo_from_cwd()
        .and_then(|p| dunce::canonicalize(&p).ok());
    let current_repo_path = cwd_worktree_path
        .as_ref()
        .and_then(|p| resolve_main_repo_root(p))
        .and_then(|main_root| {
            let canonical = dunce::canonicalize(&main_root).unwrap_or(main_root);
            is_within_search_dirs(&canonical, &search_dirs).then_some(canonical)
        });
    let initial_repo = current_repo_path.as_ref().and_then(|repo_path| {
        let name = repo_path.file_name()?.to_string_lossy().to_string();
        let worktrees = git.list_worktrees(repo_path);
        Some(kiosk_core::git::Repo {
            session_name: name.clone(),
            name,
            path: repo_path.clone(),
            worktrees,
        })
    });

    let mut state = if let Some(repo) = initial_repo {
        let mut s = AppState::new(vec![repo], config.session.split_command.clone());
        s.loading_repos = true;
        s.current_repo_path = current_repo_path;
        s.cwd_worktree_path = cwd_worktree_path;
        s
    } else {
        let mut s =
            AppState::new_loading("Discovering repos...", config.session.split_command.clone());
        s.current_repo_path = current_repo_path;
        s.cwd_worktree_path = cwd_worktree_path;
        s
    };
    state.pending_worktree_deletes = load_pending_worktree_deletes();

    let theme = Theme::from_config(&config.theme);

    let mut terminal = if should_disable_alt_screen() {
        // Inline viewport keeps drawing in the primary screen buffer, which makes
        // tmux capture-pane output usable for automation/debugging.
        ratatui::init_with_options(ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(30),
        })
    } else {
        ratatui::init()
    };
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
            if !tmux.session_exists(&session_name) {
                tmux.create_session(&session_name, &path, split_command.as_deref())?;
            }

            tmux.switch_to_session(&session_name);
        }
        Some(OpenAction::Quit) | None => {}
    }

    Ok(())
}

fn is_within_search_dirs(path: &Path, search_dirs: &[(std::path::PathBuf, u16)]) -> bool {
    search_dirs.iter().any(|(dir, _)| {
        let dir_canonical = dunce::canonicalize(dir).unwrap_or_else(|_| dir.clone());
        path.starts_with(&dir_canonical)
    })
}

/// If `path` is a secondary git worktree root, resolve to the main repository root.
/// Returns the path unchanged if it's already a main repository root.
fn resolve_main_repo_root(path: &Path) -> Option<std::path::PathBuf> {
    let git_entry = path.join(GIT_DIR_ENTRY);
    if git_entry.is_file() {
        // Secondary worktree: .git is a file containing "gitdir: /path/to/main/.git/worktrees/name"
        let content = fs::read_to_string(&git_entry).ok()?;
        let gitdir_str = content
            .lines()
            .find(|l| l.starts_with(GITDIR_FILE_PREFIX))?
            .strip_prefix(GITDIR_FILE_PREFIX)?
            .trim();
        let gitdir_raw = Path::new(gitdir_str);
        // Resolve relative gitdir paths against the worktree root
        let gitdir = if gitdir_raw.is_relative() {
            path.join(gitdir_raw)
        } else {
            gitdir_raw.to_path_buf()
        };
        // .git/worktrees/<name> → .git/worktrees → .git → repo root
        gitdir.parent()?.parent()?.parent().map(Path::to_path_buf)
    } else if git_entry.is_dir() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn command_wants_json(command: Option<&Commands>) -> bool {
    match command {
        Some(
            Commands::List { json }
            | Commands::Branches { json, .. }
            | Commands::Sessions { json }
            | Commands::Open { json, .. }
            | Commands::Status { json, .. },
        ) => *json,
        Some(Commands::Clean { .. } | Commands::Delete { .. }) | None => false,
    }
}

fn should_disable_alt_screen() -> bool {
    match std::env::var("KIOSK_NO_ALT_SCREEN") {
        Ok(value) => {
            let value = value.trim().to_ascii_lowercase();
            !matches!(value.as_str(), "" | "0" | "false" | "no" | "off")
        }
        Err(_) => false,
    }
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

    // First check: basic validation of the worktree structure
    let is_structurally_valid = if let Some(worktrees_dir) = gitdir.parent()
        && let Some(git_dir) = worktrees_dir.parent()
        && git_dir.join("HEAD").exists()
    {
        true // This appears to be a valid worktree
    } else {
        false
    };

    if !is_structurally_valid {
        return true; // Structurally invalid, definitely orphaned
    }

    // Second check: cross-reference with git worktree list output when possible.
    // If git fails (binary missing, non-zero exit), fall through to structural validation
    // rather than incorrectly classifying valid worktrees as orphaned.
    if let Some(main_repo_path) = find_main_repo_path(gitdir)
        && let Some(known) = is_worktree_known_to_git(&main_repo_path, path)
    {
        return !known;
    }

    // Fallback: if we can't determine via git, trust the structural validation
    false
}

/// Extract the main repository path from the gitdir path
/// e.g. "/repo/.git/worktrees/branch" -> "/repo"
fn find_main_repo_path(gitdir: &Path) -> Option<std::path::PathBuf> {
    gitdir
        .parent()? // /repo/.git/worktrees/branch-name -> /repo/.git/worktrees
        .parent()? // /repo/.git/worktrees -> /repo/.git
        .parent() // /repo/.git -> /repo
        .map(std::path::Path::to_path_buf)
}

/// Check if a worktree path is known to git in the main repository.
/// Returns `Some(true)` if found, `Some(false)` if not found, `None` if git failed.
fn is_worktree_known_to_git(main_repo_path: &Path, worktree_path: &Path) -> Option<bool> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(main_repo_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Canonicalize our path so we match git's absolute paths even through symlinks.
    // dunce avoids Windows UNC prefix (\\?\) that git's output won't contain.
    let canonical =
        dunce::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.to_path_buf());

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse porcelain output to find worktree paths
    // Format: "worktree /path/to/worktree\nHEAD <sha>\nbranch <branch>\n\n"
    for line in stdout.lines() {
        if let Some(listed_path) = line.strip_prefix("worktree ")
            && Path::new(listed_path) == canonical
        {
            return Some(true);
        }
    }

    Some(false)
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
