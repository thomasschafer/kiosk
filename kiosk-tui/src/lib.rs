//! The terminal UI shared by kiosk products, generic over the [`kiosk_core::backend::Backend`]
//! a product opens checkouts in. Products call [`launch::run`].

mod app;
pub mod components;
pub mod keymap;
pub mod launch;
pub mod theme;
mod wizard;
