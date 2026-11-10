//! Errors. Every message is written for the person using the application: the bridge sends
//! `to_string()` as the reply's `error`.

use std::fmt;
use std::path::Path;

/// What went wrong, in words a user can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A request, a file or its content is not acceptable.
    Invalid(String),
    /// A file could not be read or written. The message names the file and the system's reason.
    Io(String),
    /// The operation was cancelled.
    Cancelled,
}

impl Error {
    /// An [`Error::Invalid`] with this message.
    pub fn invalid(message: impl Into<String>) -> Self {
        Error::Invalid(message.into())
    }

    /// An [`Error::Io`] for `path`, worded "Could not `action` `file name`: `reason`."
    pub fn io(action: &str, path: &Path, error: &std::io::Error) -> Self {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        Error::Io(format!("Could not {action} {name}: {}.", io_reason(error)))
    }

    /// The message, without the variant.
    pub fn message(&self) -> &str {
        match self {
            Error::Invalid(message) | Error::Io(message) => message,
            Error::Cancelled => "The operation was cancelled.",
        }
    }
}

/// The system's reason without Rust's "(os error N)" suffix, starting in lower case.
pub(crate) fn io_reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    let text = match text.find(" (os error") {
        Some(index) => text[..index].to_string(),
        None => text,
    };
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => "unknown error".to_string(),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for Error {}

impl From<st_project::Error> for Error {
    fn from(error: st_project::Error) -> Self {
        match error {
            st_project::Error::Invalid(message) => Error::Invalid(message),
            st_project::Error::Io(message) => Error::Io(message),
            st_project::Error::Cancelled => Error::Cancelled,
        }
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
