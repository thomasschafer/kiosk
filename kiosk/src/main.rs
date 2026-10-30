use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use kiosk::{PRODUCT, commands};

#[derive(Parser)]
#[command(
    version,
    about = "Fuzzy-find Git repositories and branches, and open them as tmux sessions. Without a command, runs the picker."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Switch to the least recently active other session whose agent is waiting, done,
    /// idle or in an unknown state
    Next {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// List the agents in tmux panes and what they are doing
    Agents {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Read one pane's output for OSC 7501 status; tmux runs this for kiosk
    #[command(hide = true)]
    Collect {
        /// The tmux server's socket
        #[arg(long)]
        socket: PathBuf,
        /// The pane whose output this is
        #[arg(long)]
        pane: String,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("{}: {error:#}", PRODUCT.name);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        None => commands::picker(),
        Some(Command::Next { json }) => commands::next(json),
        Some(Command::Agents { json }) => commands::agents(json),
        Some(Command::Collect { socket, pane }) => commands::collect(socket, &pane),
    }
}
