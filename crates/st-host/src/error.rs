//! The error type of command handlers.

use std::fmt;

/// Why a command failed, in words the person using the application can read: the bridge sends the
/// text as the reply's `error`, and the interface shows it.
///
/// Any `std::error::Error` converts with `?` (its `Display` text becomes the message), so a handler
/// can return another crate's error unchanged when that error is already written for users. Add the
/// missing context where it is not: [`Error::context`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    /// An error with this message.
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }

    /// "`<what>`: `<cause>`", for causes that do not say what was being done.
    pub fn context(what: impl fmt::Display, cause: impl fmt::Display) -> Self {
        Error(format!("{what}: {cause}"))
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

// `Error` deliberately does not implement `std::error::Error`: that is what allows this blanket
// conversion (the same arrangement as `anyhow::Error`).
impl<E: std::error::Error> From<E> for Error {
    fn from(error: E) -> Self {
        Error(error.to_string())
    }
}

/// `Result` with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Returns early with an [`Error`] built like `format!`.
///
/// ```
/// fn tempo(bpm: f64) -> st_host::Result<f64> {
///     if !(20.0..=400.0).contains(&bpm) {
///         st_host::fail!("Project tempo must be between 20 and 400 BPM (got {bpm}).");
///     }
///     Ok(bpm)
/// }
/// assert_eq!(tempo(500.0).unwrap_err().message(), "Project tempo must be between 20 and 400 BPM (got 500).");
/// ```
#[macro_export]
macro_rules! fail {
    ($($message:tt)*) => {
        return ::std::result::Result::Err($crate::Error::new(format!($($message)*)).into())
    };
}
