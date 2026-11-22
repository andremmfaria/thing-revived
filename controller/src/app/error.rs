//! Typed application error: a kind that tells the router which wire reply
//! shape fits, plus the user-facing message. Hand-written on purpose (no
//! `thiserror`/`anyhow`): three kinds and a message are all this crate needs.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The backend does not exist on this system (for example BlueZ is absent).
    Unavailable,
    /// The request itself was rejected (out of range, unknown name).
    Invalid,
    /// The backend exists but the operation failed.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> AppError {
        AppError { kind, message: message.into() }
    }

    pub fn unavailable(message: impl Into<String>) -> AppError {
        AppError::new(ErrorKind::Unavailable, message)
    }

    pub fn invalid(message: impl Into<String>) -> AppError {
        AppError::new(ErrorKind::Invalid, message)
    }

    /// Wraps any displayable backend error as `Failed`. Usable directly in
    /// `map_err(AppError::failed)`.
    pub fn failed(message: impl fmt::Display) -> AppError {
        AppError::new(ErrorKind::Failed, message.to_string())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

/// Lets legacy `Result<_, String>` call sites keep compiling with `?`.
impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_the_kind_and_display_is_the_message() {
        assert_eq!(AppError::unavailable("no bluez").kind, ErrorKind::Unavailable);
        assert_eq!(AppError::invalid("bad").kind, ErrorKind::Invalid);
        let e = AppError::failed(std::io::Error::new(std::io::ErrorKind::Other, "boom"));
        assert_eq!(e.kind, ErrorKind::Failed);
        assert_eq!(e.to_string(), "boom");
    }

    #[test]
    fn converts_to_the_legacy_string_form() {
        let s: String = AppError::invalid("brightness 9 out of range").into();
        assert_eq!(s, "brightness 9 out of range");
    }
}
