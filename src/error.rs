//! What can go wrong driving the app.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    /// `herdr-app` or `hc` refused, with its own code and message.
    Other { code: String, message: String },
    /// Kiosk would have passed the program something it can't take.
    InvalidArgument(String),
    /// The program could not run, or failed without saying why.
    Invocation(String),
    /// The program answered in a shape kiosk doesn't understand.
    InvalidResponse(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Other { code, message } => write!(formatter, "{code}: {message}"),
            Self::InvalidArgument(message) => write!(formatter, "invalid argument: {message}"),
            Self::Invocation(message) => write!(formatter, "invocation failed: {message}"),
            Self::InvalidResponse(message) => write!(formatter, "invalid response: {message}"),
        }
    }
}

impl std::error::Error for AppError {}

/// The pane `[on_open]` failures of this backend.
pub type PaneOpenError = kiosk_core::backend::on_open::PaneOpenError<AppError>;

/// An argument check's message as an error.
pub(crate) fn checked<T>(check: Result<T, String>) -> Result<T, AppError> {
    check.map_err(AppError::InvalidArgument)
}
