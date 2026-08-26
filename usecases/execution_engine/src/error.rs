#![allow(clippy::std_instead_of_core, clippy::absolute_paths)]

//! Errors produced while resolving and running an operation identifier.

use std::io;

use thiserror::Error;

/// Failure modes of [`Engine::run`](crate::Engine::run).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionEngine {
    /// The requested service, operation, or adapter wasn't found or
    /// registered.
    #[error("Not found: {0}")]
    NotFound(String),

    /// The manifest used a feature this engine doesn't (yet) support.
    #[error("Unimplemented: {0}")]
    Unimplemented(String),

    /// An operation identifier wasn't in the expected `service.operation`
    /// shape.
    #[error("Invalid Identifier: {0}")]
    InvalidIdentifier(String),

    /// Writing to the shared log file failed.
    #[error(transparent)]
    Io {
        /// The underlying I/O error.
        #[from]
        source: io::Error,
    },

    /// TODO: Rename to OutputPort
    #[error(transparent)]
    Other {
        /// The wrapped error from an output port implementation.
        source: anyhow::Error,
    },
}

/// Shorthand for a [`Result`](core::result::Result) using
/// [`ExecutionEngine`] as its error type.
pub type Result<T> = core::result::Result<T, ExecutionEngine>;
