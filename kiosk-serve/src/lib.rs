//! Runs a kiosk picker for a native frontend, such as a GUI app's plugin, that draws it
//! itself and talks to the picker over a loopback socket.
//!
//! A product's serve command calls [`serve`]. The frontend runs that command, reads the
//! [`launch::Handshake`] it prints, connects and speaks the [`protocol`]. The picker itself
//! is the same as in the terminal: kiosk-core's state machine, keymap and
//! [`kiosk_core::view`], so the frontend only draws what it is sent and reports input.

pub mod connection;
pub mod keystroke;
pub mod launch;
pub mod protocol;
pub mod session;

use std::panic::{AssertUnwindSafe, catch_unwind};

use anyhow::{Result, anyhow};
use kiosk_core::{backend::Backend, product::Product};

pub use connection::{Connection, Notifier};
pub use protocol::NoticeLevel;
pub use session::Start;

/// The serve command. In the process the frontend runs, starts the picker in the background
/// and prints its handshake. In the background, waits for the frontend, builds the session
/// with `prepare` (given a [`Notifier`] for the backend) and runs it. Failures after the
/// frontend has connected are sent to it as well as returned.
pub fn serve<B: Backend, E>(
    product: &'static Product,
    prepare: impl FnOnce(Notifier) -> Result<Start<B, E>>,
) -> Result<()> {
    match launch::stage() {
        launch::Stage::Foreground => {
            let handshake = launch::start_background(product.name)?;
            println!("{}", serde_json::to_string(&handshake)?);
            Ok(())
        }
        launch::Stage::Background => {
            let listening = launch::listen()?;
            listening.announce(&mut std::io::stdout().lock())?;
            let accepted = listening.accept(launch::ACCEPT_TIMEOUT)?;
            let connection = Connection::new(accepted.stream)?;
            let title = format!("{} stopped", product.name);
            let result = catch_unwind(AssertUnwindSafe(|| {
                protocol::check_client(accepted.protocol)?;
                prepare(connection.notifier())
                    .and_then(|start| session::run(&connection, product, start))
            }))
            .unwrap_or_else(|panic| Err(anyhow!("it crashed: {}", panic_message(&*panic))));
            match result {
                Ok(_) => {
                    launch::remove_log();
                    Ok(())
                }
                Err(error) => {
                    connection.fail(&title, &error);
                    Err(error)
                }
            }
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}
