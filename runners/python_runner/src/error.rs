//! Errors produced while running a
//! [`PyActionRunner`](crate::PyActionRunner) operation.

use std::io;

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

/// Failure modes of [`PyActionRunner::run`](crate::PyActionRunner).
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum PyActionRunner {
    /// Reserved for a workflow-not-invoked condition; currently unused.
    #[error("Workflow Not Called")]
    WorkflowNotCalled,

    /// The shared engine's or logger's lock was poisoned by a panic in
    /// another thread while holding it.
    #[error("Get out! The lock has been poisoned: {0}")]
    PoisonedLock(String),

    /// The function-name detection regex failed to compile.
    #[error("Unable to compile regex: {0}")]
    RegexError(String),

    /// A required piece of data was missing.
    #[error("Not Found: {0}")]
    NotFound(String),

    /// Writing to the shared log file failed.
    #[error(transparent)]
    IoError {
        /// The underlying I/O error.
        #[from]
        source: io::Error,
    },

    /// The embedded Python interpreter raised an exception or otherwise
    /// failed while running the script.
    #[error("Python Error: {0}")]
    PythonError(String),
}

impl From<PyActionRunner> for ExecutionEngine {
    #[inline]
    fn from(value: PyActionRunner) -> Self {
        Self::Other {
            source: value.into(),
        }
    }
}

/// Shorthand for a [`Result`](std::result::Result) using [`PyActionRunner`]
/// as its error type.
pub type Result<T> = std::result::Result<T, PyActionRunner>;
