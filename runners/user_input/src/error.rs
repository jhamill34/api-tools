//! Errors produced while waiting on a [`UserInput`](crate::UserInput)
//! prompt.

use std::sync::mpsc::RecvTimeoutError;

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

/// Failure modes of [`UserInput::run_internal_with_timeout`](crate::UserInput).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UserInput {
    /// The shared [`Signals`](crate::Signals) mutex was poisoned by a panic
    /// in another thread while holding the lock.
    #[error("Get out! Unable to obtain lock: {0}")]
    PoisonedLock(String),

    /// No answer arrived before the prompt's timeout elapsed.
    #[error(transparent)]
    Recieve {
        /// The underlying channel timeout.
        #[from]
        source: RecvTimeoutError,
    },
}

impl From<UserInput> for ExecutionEngine {
    #[inline]
    fn from(value: UserInput) -> Self {
        ExecutionEngine::Other {
            source: value.into(),
        }
    }
}

/// Shorthand for a [`Result`](std::result::Result) using [`UserInput`] as
/// its error type.
pub type Result<T> = std::result::Result<T, UserInput>;
