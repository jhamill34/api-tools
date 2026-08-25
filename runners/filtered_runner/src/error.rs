#![allow(clippy::std_instead_of_core)]

//! Errors produced while resolving an [`APIWrapper`](crate::APIWrapper)
//! call.

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

/// Failure modes of [`APIWrapper::run`](crate::APIWrapper).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FilteredRunner {
    /// The manifest's `connectorId` didn't match the expected
    /// `"group/app:version"` shape.
    #[error("Unknown Connector: {0}")]
    UnknownConnectorId(String),

    /// An output selector's path pointed through a non-object value.
    #[error("Unable to traverse path: {0}")]
    PathTraversal(String),

    /// The shared engine's lock was poisoned by a panic in another thread
    /// while holding it.
    #[error("Poisoned Lock: {0}")]
    PoisonedLock(String),

    /// An output selector's `JMESPath` expression failed to compile or
    /// evaluate.
    #[error(transparent)]
    JmesPath {
        /// The underlying `JMESPath` error.
        #[from]
        source: jmespath::JmespathError,
    },

    /// Serializing or deserializing a selected output value failed.
    #[error(transparent)]
    Json {
        /// The underlying JSON error.
        #[from]
        source: serde_json::Error,
    },

    /// The wrapped operation itself failed.
    #[error(transparent)]
    Engine {
        /// The underlying engine error.
        #[from]
        source: ExecutionEngine,
    },
}

impl From<FilteredRunner> for ExecutionEngine {
    #[inline]
    fn from(value: FilteredRunner) -> Self {
        Self::Other {
            source: value.into(),
        }
    }
}

/// Shorthand for a [`Result`](std::result::Result) using [`FilteredRunner`]
/// as its error type.
pub type Result<T> = std::result::Result<T, FilteredRunner>;
