#![allow(clippy::std_instead_of_core)]

//! Errors produced while running a [`JsActionRunner`](crate::JsActionRunner)
//! operation.

use std::io;

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

/// Failure modes of [`JsActionRunner::run`](crate::JsActionRunner).
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum JsActionRunner {
    /// Reserved for a workflow-not-invoked condition; currently unused.
    #[error("Workflow Not Called")]
    WorkflowNotCalled,

    /// The source body's top-level function wasn't recognized as either an
    /// arrow function or a named `function` declaration.
    #[error("Unable to select which function to call: {0}")]
    NoFunctionFound(String),

    /// Writing to the shared log file failed.
    #[error(transparent)]
    IoError {
        /// The underlying I/O error.
        #[from]
        source: io::Error,
    },

    /// The shared engine's or logger's lock was poisoned by a panic in
    /// another thread while holding it.
    #[error("Get out! The lock has been poisoned: {0}")]
    PoisonedLock(String),

    /// The `MiniV8` interpreter reported an error while evaluating or
    /// calling the wrapped script.
    #[error("V8 Error: {0}")]
    V8(String),
}

impl From<JsActionRunner> for ExecutionEngine {
    #[inline]
    fn from(value: JsActionRunner) -> Self {
        Self::Other {
            source: value.into(),
        }
    }
}

/// Shorthand for a [`Result`](std::result::Result) using [`JsActionRunner`]
/// as its error type.
pub type Result<T> = std::result::Result<T, JsActionRunner>;
