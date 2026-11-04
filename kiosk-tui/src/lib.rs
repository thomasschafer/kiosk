//! The terminal UI shared by kiosk products, generic over the [`kiosk_core::backend::Backend`]
//! a product opens checkouts in. Products call [`launch::run`].

mod app;
pub mod components;
pub use kiosk_core::keymap;
pub mod launch;
pub mod theme;
mod wizard;
