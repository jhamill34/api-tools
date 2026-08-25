//! Errors produced while running a
//! [`LuaActionRunner`](crate::LuaActionRunner) operation.

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

/// Failure modes of [`LuaActionRunner::run`](crate::LuaActionRunner).
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum LuaActionRunner {
    /// The embedded Lua interpreter raised an error while loading or
    /// running the script.
    #[error("Lua error: {0}")]
    LuaError(String),

    /// The script's return value couldn't be converted to JSON.
    #[error("Unable to convert Lua return value to JSON: {0}")]
    ConversionError(String),
}

impl From<mlua::Error> for LuaActionRunner {
    #[inline]
    fn from(value: mlua::Error) -> Self {
        Self::LuaError(value.to_string())
    }
}

impl From<LuaActionRunner> for ExecutionEngine {
    #[inline]
    fn from(value: LuaActionRunner) -> Self {
        Self::Other {
            source: value.into(),
        }
    }
}

/// Shorthand for a [`Result`](core::result::Result) using
/// [`LuaActionRunner`] as its error type.
pub type Result<T> = core::result::Result<T, LuaActionRunner>;
