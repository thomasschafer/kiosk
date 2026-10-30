//! kiosk: a repository and branch picker for tmux. The picker comes from `kiosk-core` and
//! `kiosk-tui`; this crate holds the tmux backend, agent status for panes, and the config
//! it adds.

pub mod agent;
pub mod commands;
pub mod config;
pub mod tmux;

use kiosk_core::product::Product;

pub const PRODUCT: Product = Product {
    name: "kiosk",
    config_dir_var: Some("KIOSK_CONFIG_DIR"),
    state_dir_var: Some("KIOSK_STATE_DIR"),
    backend_name: "tmux",
    no_backend_message: "tmux is not available",
    setup_intro: "Pick repositories and branches, and open them in tmux sessions.",
};
