#![allow(clippy::std_instead_of_core)]

//! Errors produced by an [`OperationRepos`](crate::OperationRepos)
//! repository.

use service_loader::error::ServiceLoader;
use thiserror::Error;

/// Failure modes of a [`Repository`](crate::repo::Repository) operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OperationRepo {
    /// A repository's lock was poisoned by a panic in another thread while
    /// holding it.
    #[error("Get out! Lock has been poisened: {0}")]
    LockingError(String),

    /// No entry was found for the requested ID.
    #[error("Operation not found: {0}")]
    OperationNotFound(String),

    /// Serializing a value to its protobuf JSON representation failed.
    #[error(transparent)]
    ProtobufSerialize {
        /// The underlying protobuf serialization error.
        #[from]
        source: protobuf_json_mapping::PrintError,
    },

    /// Parsing a value from its protobuf JSON representation failed.
    #[error(transparent)]
    ProtobufParse {
        /// The underlying protobuf parse error.
        #[from]
        source: protobuf_json_mapping::ParseError,
    },

    /// Serializing or deserializing a value as plain JSON failed.
    #[error(transparent)]
    Json {
        /// The underlying JSON error.
        #[from]
        source: serde_json::Error,
    },
}

impl From<OperationRepo> for ServiceLoader {
    #[inline]
    fn from(val: OperationRepo) -> Self {
        ServiceLoader::Other { source: val.into() }
    }
}

/// Shorthand for a [`Result`](std::result::Result) using [`OperationRepo`]
/// as its error type.
pub type Result<T> = std::result::Result<T, OperationRepo>;
