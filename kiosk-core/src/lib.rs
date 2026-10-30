//! The parts of a kiosk picker that do not depend on the terminal: finding repositories,
//! git, configuration, the picker's state machine, and the [`backend::Backend`] contract
//! a multiplexer implements to open what the user picks.

pub mod backend;
pub mod config;
pub mod display;
pub mod event;
pub mod git;
pub mod keyboard;
pub mod path;
pub mod pending_delete;
pub mod picker;
pub mod product;
pub mod setup;
pub mod spawn;
pub mod state;
