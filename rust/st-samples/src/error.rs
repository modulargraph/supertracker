//! The crate's error type. Every message is written for the person using the app: the bridge
//! puts `to_string()` into a reply's `error` field unchanged.

use std::fmt;

/// What went wrong in a sample operation.
///
/// The variants let callers react differently (a cancelled decode is not a failure worth
/// reporting; an offline drive is not a broken file) while `Display` stays a readable sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleError {
    /// The request or the audio data is not acceptable (bad range, empty sample, unknown
    /// operation, unsupported setting). Nothing was changed.
    Invalid(String),
    /// The file does not exist.
    NotFound(String),
    /// The file exists but is not audio this crate can decode.
    Unsupported(String),
    /// The drive that holds the file is not mounted.
    Offline(String),
    /// The operation was cancelled through its [`crate::CancelToken`].
    Cancelled,
    /// Reading or writing a file failed. The text names the operation and the system's reason.
    Io(String),
}

impl SampleError {
    /// Shorthand for [`SampleError::Invalid`].
    pub fn invalid(message: impl Into<String>) -> Self {
        SampleError::Invalid(message.into())
    }

    /// An [`SampleError::Io`] that names what was being done, e.g. `"Write sample file"`.
    pub fn io(operation: &str, error: &std::io::Error) -> Self {
        SampleError::Io(format!("{operation}: {error}."))
    }

    /// True for [`SampleError::Cancelled`].
    pub fn is_cancelled(&self) -> bool {
        matches!(self, SampleError::Cancelled)
    }
}

impl fmt::Display for SampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SampleError::Invalid(message)
            | SampleError::NotFound(message)
            | SampleError::Unsupported(message)
            | SampleError::Offline(message)
            | SampleError::Io(message) => f.write_str(message),
            SampleError::Cancelled => f.write_str("The operation was cancelled."),
        }
    }
}

impl std::error::Error for SampleError {}

/// `Result` with [`SampleError`].
pub type Result<T> = std::result::Result<T, SampleError>;

/// The last path component, for messages ("kick.wav" rather than the whole path).
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(path)
}
