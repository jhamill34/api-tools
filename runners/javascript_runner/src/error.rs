#![allow(clippy::std_instead_of_core)]

//!

use std::io;

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

///
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum JsActionRunner {
    ///
    #[error("Workflow Not Called")]
    WorkflowNotCalled,

    ///
    #[error("Unable to select which function to call: {0}")]
    NoFunctionFound(String),

    ///
    #[error(transparent)]
    IoError {
        ///
        #[from]
        source: io::Error,
    },

    ///
    #[error("Get out! The lock has been poisoned: {0}")]
    PoisonedLock(String),

    ///
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

///
pub type Result<T> = std::result::Result<T, JsActionRunner>;
