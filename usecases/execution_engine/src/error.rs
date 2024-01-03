#![allow(clippy::std_instead_of_core, clippy::absolute_paths)]

//!

use std::io;

use thiserror::Error;

///
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionEngine {
    ///
    #[error("Not found: {0}")]
    NotFound(String),

    ///
    #[error("Unimplemented: {0}")]
    Unimplemented(String),

    ///
    #[error("Invalid Identifier: {0}")]
    InvalidIdentifier(String),

    ///
    #[error(transparent)]
    Io {
        ///
        #[from]
        source: io::Error,
    },

    ///
    #[error("Get out of here! The Lock is poisoned: {0}")]
    PoisonedLock(String),

    /// TODO: Rename to OutputPort
    #[error(transparent)]
    Other {
        ///
        source: anyhow::Error,
    },
}

///
pub type Result<T> = core::result::Result<T, ExecutionEngine>;
