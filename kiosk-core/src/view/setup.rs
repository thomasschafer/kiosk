//! What the first-run setup wizard shows at each step.

use serde::Serialize;

use super::{Hint, Query};
use crate::{
    display::sanitize,
    product::Product,
    setup::{FolderMode, SetupDir, SetupState, SetupStep},
};

/// How many path completions are offered at once.
pub const COMPLETIONS_SHOWN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "one view is built per frame and serialized straight away"
)]
pub enum SetupView {
    Welcome {
        title: &'static str,
        heading: String,
        intro: &'static str,
        explanation: &'static str,
        config_label: &'static str,
        config_path: String,
        hints: Vec<Hint>,
    },
    Directories {
        title: &'static str,
        instructions: &'static str,
        path: Field,
        /// Asked once a path is entered; the path field then shows that path.
        depth: Option<Field>,
        completions: Vec<String>,
        selected_completion: Option<usize>,
        added_title: &'static str,
        added: Vec<AddedDir>,
        nothing_added: &'static str,
        /// A problem with what was entered, shown in place of the hints.
        message: Option<String>,
        hints: Vec<Hint>,
    },
    FolderMode {
        title: &'static str,
        question: &'static str,
        directory_label: &'static str,
        directory: String,
        depth: String,
        options: Vec<FolderModeOption>,
        hints: Vec<Hint>,
    },
    Confirm {
        title: &'static str,
        heading: &'static str,
        dirs: Vec<AddedDir>,
        write_label: &'static str,
        config_path: String,
        hints: Vec<Hint>,
    },
}

/// A labelled text field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    pub label: &'static str,
    pub placeholder: &'static str,
    pub query: Query,
    /// Whether typing goes here.
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddedDir {
    pub path: String,
    /// Such as `depth 2`.
    pub depth: String,
    pub all_folders: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderModeOption {
    pub mode: FolderModeName,
    pub label: &'static str,
    pub selected: bool,
}

/// [`FolderMode`] as frontends name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderModeName {
    GitRepositoriesOnly,
    AllFolders,
}

impl From<FolderMode> for FolderModeName {
    fn from(mode: FolderMode) -> Self {
        match mode {
            FolderMode::GitRepositoriesOnly => Self::GitRepositoriesOnly,
            FolderMode::AllFolders => Self::AllFolders,
        }
    }
}

impl From<FolderModeName> for FolderMode {
    fn from(mode: FolderModeName) -> Self {
        match mode {
            FolderModeName::GitRepositoriesOnly => Self::GitRepositoriesOnly,
            FolderModeName::AllFolders => Self::AllFolders,
        }
    }
}

pub const FOLDER_MODES: [(FolderMode, &str); 2] = [
    (
        FolderMode::GitRepositoriesOnly,
        "Git repositories only (default)",
    ),
    (FolderMode::AllFolders, "All folders"),
];

pub fn depth_label(depth: u16) -> String {
    format!("depth {depth}")
}

pub const ALL_FOLDERS_SUFFIX: &str = "all folders";

/// `config_path` is where the config will be written, as the user should read it.
pub fn setup(state: &SetupState, product: &Product, config_path: &str) -> SetupView {
    let config_path = sanitize(config_path).into_owned();
    match &state.step {
        SetupStep::Welcome => SetupView::Welcome {
            title: "Welcome",
            heading: format!("Welcome to {}", product.name),
            intro: product.setup_intro,
            explanation: "This setup will choose the directories to scan.",
            config_label: "Config: ",
            config_path,
            hints: vec![Hint::new("enter", "continue"), Hint::new("esc", "quit")],
        },
        SetupStep::Directories | SetupStep::Depth { .. } => directories(state),
        SetupStep::FolderMode { path, depth } => SetupView::FolderMode {
            title: "Folder inclusion",
            question: "List only git repositories in this directory, or every folder?",
            directory_label: "Directory: ",
            directory: sanitize(path).into_owned(),
            depth: depth_label(*depth),
            options: FOLDER_MODES
                .into_iter()
                .map(|(mode, label)| FolderModeOption {
                    mode: mode.into(),
                    label,
                    selected: state.folder_mode == mode,
                })
                .collect(),
            hints: vec![
                Hint::new("Up/down", "select"),
                Hint::new("enter", "confirm"),
                Hint::new("esc", "back"),
            ],
        },
        SetupStep::Confirm => SetupView::Confirm {
            title: "Confirm setup",
            heading: "Ready to start",
            dirs: added_dirs(&state.dirs),
            write_label: "Write to: ",
            config_path,
            hints: vec![
                Hint::new("enter", "write config and open picker"),
                Hint::new("esc", "back"),
            ],
        },
    }
}

fn directories(state: &SetupState) -> SetupView {
    let pending_path = match &state.step {
        SetupStep::Depth { path } => Some(path.as_str()),
        _ => None,
    };
    let path = Field {
        label: "Directory path",
        placeholder: "~/Development",
        query: pending_path.map_or_else(
            || Query::new(&state.input),
            |path| Query::from_parts(path, path.len()),
        ),
        active: pending_path.is_none(),
    };
    let depth = pending_path.map(|_| Field {
        label: "Depth",
        placeholder: "1",
        query: Query::new(&state.input),
        active: true,
    });
    let (completions, selected_completion) = if pending_path.is_some() {
        (Vec::new(), None)
    } else {
        (
            state
                .completions
                .iter()
                .take(COMPLETIONS_SHOWN)
                .map(|completion| sanitize(completion).into_owned())
                .collect(),
            state
                .selected_completion
                .filter(|index| *index < COMPLETIONS_SHOWN),
        )
    };
    let hints = if pending_path.is_some() {
        vec![
            Hint::new("enter", "add directory"),
            Hint::new("esc", "edit path"),
        ]
    } else {
        vec![
            Hint::new("enter", "add/finish"),
            Hint::new("tab", "complete"),
            Hint::new("ctrl+x", "remove last"),
            Hint::new("esc", "quit"),
        ]
    };
    SetupView::Directories {
        title: "Search directories",
        instructions: "Add one or more directories. Submit an empty input when finished.",
        path,
        depth,
        completions,
        selected_completion,
        added_title: "Added",
        added: added_dirs(&state.dirs),
        nothing_added: "No directories added yet",
        message: state
            .message
            .as_deref()
            .map(|message| sanitize(message).into_owned()),
        hints,
    }
}

fn added_dirs(dirs: &[SetupDir]) -> Vec<AddedDir> {
    dirs.iter()
        .map(|dir| AddedDir {
            path: sanitize(&dir.path).into_owned(),
            depth: depth_label(dir.depth),
            all_folders: dir.include_non_git == Some(true),
        })
        .collect()
}

/// Shown when the config lists no directories to search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoSearchDirsView {
    pub title: &'static str,
    pub location: String,
    pub hint: &'static str,
}

pub fn no_search_dirs(config_path: Option<&std::path::Path>) -> NoSearchDirsView {
    NoSearchDirsView {
        title: "No search directories configured",
        location: config_path.map_or_else(
            || "No trusted config path could be resolved.".to_string(),
            |path| format!("Config path: {}", sanitize(&crate::path::display(path))),
        ),
        hint: "q / esc / ctrl+c to quit",
    }
}
