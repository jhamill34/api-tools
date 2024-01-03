#![allow(clippy::std_instead_of_core)]

//!

use std::sync::mpsc::RecvTimeoutError;

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

///
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UserInput {
    ///
    #[error("Get out! Unable to obtain lock: {0}")]
    PoisonedLock(String),

    ///
    #[error(transparent)]
    Recieve {
        ///
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

///
pub type Result<T> = std::result::Result<T, UserInput>;
