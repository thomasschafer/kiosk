//! herdr-app-kiosk: kiosk's repository and branch picker for the Herdr desktop app. The
//! app's `kiosk` mod runs `herdr-app-kiosk serve`, which serves the picker through
//! kiosk-serve for the mod to draw with the app's own UI. The picker itself is
//! kiosk-core's; this crate is the app backend that opens what is picked, the config it
//! adds, and how the mod starts it.

pub mod app;
pub mod background;
pub mod config;
pub mod error;
pub mod launch;
pub mod migrate;
pub mod notices;
#[cfg(all(test, unix))]
mod test_support;

use kiosk_core::product::Product;

pub const PRODUCT: Product = Product {
    name: "herdr-app-kiosk",
    config_dir_var: None,
    state_dir_var: None,
    backend_name: "Herdr",
    no_backend_message: "not running in the Herdr app",
    setup_intro: "Pick repositories and branches without leaving Herdr.",
};
