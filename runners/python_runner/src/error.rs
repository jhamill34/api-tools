#![allow(clippy::std_instead_of_core)]

//!

use std::io;

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

///
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum PyActionRunner {
    ///
    #[error("Workflow Not Called")]
    WorkflowNotCalled,

    ///
    #[error("Get out! The lock has been poisoned: {0}")]
    PoisonedLock(String),

    ///
    #[error("Unable to compile regex: {0}")]
    RegexError(String),

    ///
    #[error("Not Found: {0}")]
    NotFound(String),

    ///
    #[error(transparent)]
    IoError {
        ///
        #[from]
        source: io::Error,
    },

    ///
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

///
pub type Result<T> = std::result::Result<T, PyActionRunner>;
