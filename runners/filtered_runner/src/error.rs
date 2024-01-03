#![allow(clippy::std_instead_of_core)]

//!

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

///
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FilteredRunner {
    ///
    #[error("Unknown Connector: {0}")]
    UnknownConnectorId(String),

    ///
    #[error("Unable to traverse path: {0}")]
    PathTraversal(String),

    ///
    #[error("Poisoned Lock: {0}")]
    PoisonedLock(String),

    ///
    #[error(transparent)]
    JmesPath {
        ///
        #[from]
        source: jmespath::JmespathError,
    },

    ///
    #[error(transparent)]
    Json {
        ///
        #[from]
        source: serde_json::Error,
    },

    ///
    #[error(transparent)]
    Engine {
        ///
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

///
pub type Result<T> = std::result::Result<T, FilteredRunner>;
