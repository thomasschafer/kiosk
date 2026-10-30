use std::{env, path::Path};

use anyhow::{Result, bail};
use kiosk::config::{Config, SOURCE};
use kiosk_docgen::ProductDocs;

const DOCS: ProductDocs<'static> = ProductDocs {
    sources: &[SOURCE],
    extension: "Config",
    order: &[
        "search_dirs",
        "include_non_git",
        "worktrees",
        "on_open",
        "agent",
        "theme",
        "keys",
    ],
    examples: &[(
        "on_open",
        "[on_open]\npanes = [\n  { command = \"hx\", direction = \"right\" },\n]",
    )],
    wording: &[
        (
            "OnOpenConfig::panes",
            "Pane definitions, created in order without moving focus from the first pane.\n\
             Commands run from the opened repository or worktree. They run only when kiosk\n\
             creates a session, not when it switches to an existing one.",
        ),
        (
            "OnOpenPaneConfig",
            "A command pane split off a new session's first pane.",
        ),
        (
            "OnOpenPaneConfig::command",
            "Shell command typed into the new pane's shell, which stays when the command ends. \
             The command must not be empty.",
        ),
        (
            "OnOpenPaneConfig::ratio",
            "Fraction of the split occupied by the new command pane. The value must be greater \
             than 0 and less than 1, and defaults to 0.5 when omitted.",
        ),
    ],
};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let Some(task) = args.next() else {
        bail!("usage: cargo xtask readme [--check]");
    };
    if task != "readme" {
        bail!("unknown xtask '{task}'; expected 'readme'");
    }
    let mut check = false;
    for argument in args {
        if argument == "--check" && !check {
            check = true;
        } else {
            bail!("unexpected argument '{argument}'; usage: cargo xtask readme [--check]");
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace parent");
    kiosk_docgen::generate::<Config>(&root.join("README.md"), &DOCS, check)
}
