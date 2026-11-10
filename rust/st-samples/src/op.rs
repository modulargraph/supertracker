//! Cancellation and progress reporting for long operations.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{Result, SampleError};

/// A flag one thread sets to stop work running on another. Clones share the flag.
///
/// Long operations poll it between blocks (a few thousand frames), so cancelling takes effect
/// within milliseconds and never leaves a partial output file behind.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that is not cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent; may be called from any thread.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether [`CancelToken::cancel`] was called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// `Err(SampleError::Cancelled)` once cancelled.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(SampleError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// How far a long operation has come, in frames of its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Frames handled so far.
    pub done: u64,
    /// Frames in total, when the file states its length.
    pub total: Option<u64>,
}

impl Progress {
    /// 0...1 when the total is known.
    pub fn fraction(&self) -> Option<f64> {
        self.total.map(|total| if total == 0 { 1.0 } else { (self.done as f64 / total as f64).min(1.0) })
    }
}

/// The controls of one long operation: an optional cancellation token and an optional progress
/// callback. `Operation::default()` runs to completion silently.
#[derive(Default)]
pub struct Operation<'a> {
    cancel: Option<&'a CancelToken>,
    progress: Option<&'a mut dyn FnMut(Progress)>,
}

impl<'a> Operation<'a> {
    /// No cancellation, no progress.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops with [`SampleError::Cancelled`] once `token` is cancelled.
    pub fn with_cancel(mut self, token: &'a CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Calls `callback` after each block, on the thread that runs the operation.
    pub fn with_progress(mut self, callback: &'a mut dyn FnMut(Progress)) -> Self {
        self.progress = Some(callback);
        self
    }

    /// `Err(Cancelled)` when the token was cancelled.
    pub fn check(&self) -> Result<()> {
        match self.cancel {
            Some(token) => token.check(),
            None => Ok(()),
        }
    }

    /// Reports progress to the callback, if any.
    pub fn report(&mut self, done: u64, total: Option<u64>) {
        if let Some(callback) = self.progress.as_mut() {
            callback(Progress { done, total });
        }
    }
}
