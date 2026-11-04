use std::path::Path;

use crate::{
    product::TEST_PRODUCT,
    setup::{FolderMode, SetupDir, SetupState, SetupStep},
    view::{
        Hint, Query,
        setup::{
            AddedDir, FolderModeName, FolderModeOption, NoSearchDirsView, SetupView,
            no_search_dirs, setup,
        },
    },
};

fn dirs() -> Vec<SetupDir> {
    vec![
        SetupDir {
            path: "~/GitOnly".into(),
            depth: 1,
            include_non_git: None,
        },
        SetupDir {
            path: "~/Every\nthing".into(),
            depth: 2,
            include_non_git: Some(true),
        },
    ]
}

fn added() -> Vec<AddedDir> {
    vec![
        AddedDir {
            path: "~/GitOnly".into(),
            depth: "depth 1".into(),
            all_folders: false,
        },
        AddedDir {
            path: "~/Every\\nthing".into(),
            depth: "depth 2".into(),
            all_folders: true,
        },
    ]
}

#[test]
fn welcome_names_the_product_and_where_the_config_goes() {
    assert_eq!(
        setup(&SetupState::default(), &TEST_PRODUCT, "/config\t.toml"),
        SetupView::Welcome {
            title: "Welcome",
            heading: "Welcome to kiosk-test".into(),
            intro: TEST_PRODUCT.setup_intro,
            explanation: "This setup will choose the directories to scan.",
            config_label: "Config: ",
            config_path: "/config\\t.toml".into(),
            hints: vec![Hint::new("enter", "continue"), Hint::new("esc", "quit")],
        }
    );
}

#[test]
fn directories_offer_completions_and_list_what_was_added() {
    let mut state = SetupState {
        step: SetupStep::Directories,
        dirs: dirs(),
        ..SetupState::default()
    };
    state.input.text = "~/De".into();
    state.input.cursor = 2;
    state.completions = (0..7).map(|index| format!("~/Dev{index}")).collect();
    state.selected_completion = Some(1);
    let SetupView::Directories {
        path,
        depth,
        completions,
        selected_completion,
        added: listed,
        message,
        hints,
        ..
    } = setup(&state, &TEST_PRODUCT, "/config.toml")
    else {
        panic!("expected the directories step");
    };
    assert_eq!(
        path.query,
        Query {
            before: "~/".into(),
            after: "De".into(),
        }
    );
    assert!(path.active);
    assert_eq!(depth, None);
    assert_eq!(completions.len(), 5);
    assert_eq!(selected_completion, Some(1));
    assert_eq!(listed, added());
    assert_eq!(message, None);
    assert_eq!(
        hints,
        [
            Hint::new("enter", "add/finish"),
            Hint::new("tab", "complete"),
            Hint::new("ctrl+x", "remove last"),
            Hint::new("esc", "quit"),
        ]
    );

    state.selected_completion = Some(6);
    state.message = Some("Add at least one search directory".into());
    let SetupView::Directories {
        selected_completion,
        message,
        ..
    } = setup(&state, &TEST_PRODUCT, "/config.toml")
    else {
        unreachable!()
    };
    assert_eq!(selected_completion, None);
    assert_eq!(
        message.as_deref(),
        Some("Add at least one search directory")
    );
}

#[test]
fn depth_is_asked_for_the_entered_path_without_completions() {
    let mut state = SetupState {
        step: SetupStep::Directories,
        ..SetupState::default()
    };
    state.input.text = "/repo".into();
    state.completions = vec!["/repos".into()];
    state.begin_depth().unwrap();
    state.completions = vec!["/repos".into()];
    let SetupView::Directories {
        path,
        depth,
        completions,
        hints,
        ..
    } = setup(&state, &TEST_PRODUCT, "/config.toml")
    else {
        panic!("expected the directories step");
    };
    assert!(!path.active);
    assert_eq!(
        path.query,
        Query {
            before: "/repo".into(),
            after: String::new(),
        }
    );
    let depth = depth.expect("depth is asked for");
    assert!(depth.active);
    assert_eq!(depth.label, "Depth");
    assert_eq!(
        depth.query,
        Query {
            before: "1".into(),
            after: String::new(),
        }
    );
    assert!(completions.is_empty());
    assert_eq!(
        hints,
        [
            Hint::new("enter", "add directory"),
            Hint::new("esc", "edit path")
        ]
    );
}

#[test]
fn folder_mode_offers_both_modes_with_the_chosen_one_selected() {
    let state = SetupState {
        step: SetupStep::FolderMode {
            path: "/repo".into(),
            depth: 3,
        },
        folder_mode: FolderMode::AllFolders,
        ..SetupState::default()
    };
    let SetupView::FolderMode {
        directory,
        depth,
        options,
        ..
    } = setup(&state, &TEST_PRODUCT, "/config.toml")
    else {
        panic!("expected the folder mode step");
    };
    assert_eq!(directory, "/repo");
    assert_eq!(depth, "depth 3");
    assert_eq!(
        options,
        [
            FolderModeOption {
                mode: FolderModeName::GitRepositoriesOnly,
                label: "Git repositories only (default)",
                selected: false,
            },
            FolderModeOption {
                mode: FolderModeName::AllFolders,
                label: "All folders",
                selected: true,
            },
        ]
    );
    for mode in [FolderMode::GitRepositoriesOnly, FolderMode::AllFolders] {
        assert_eq!(FolderMode::from(FolderModeName::from(mode)), mode);
    }
}

#[test]
fn confirm_lists_the_directories_and_the_config_path() {
    let state = SetupState {
        step: SetupStep::Confirm,
        dirs: dirs(),
        ..SetupState::default()
    };
    assert_eq!(
        setup(&state, &TEST_PRODUCT, "/config.toml"),
        SetupView::Confirm {
            title: "Confirm setup",
            heading: "Ready to start",
            dirs: added(),
            write_label: "Write to: ",
            config_path: "/config.toml".into(),
            hints: vec![
                Hint::new("enter", "write config and open picker"),
                Hint::new("esc", "back"),
            ],
        }
    );
}

#[test]
fn no_search_dirs_says_where_the_config_is_when_it_is_known() {
    assert_eq!(
        no_search_dirs(Some(Path::new("/config.toml"))),
        NoSearchDirsView {
            title: "No search directories configured",
            location: "Config path: /config.toml".into(),
            hint: "q / esc / ctrl+c to quit",
        }
    );
    assert_eq!(
        no_search_dirs(None).location,
        "No trusted config path could be resolved."
    );
}
