//! Errors produced while writing a service manifest or its credentials.

use std::io;

use thiserror::Error;

/// Failure modes of [`ServiceWriter`](crate::ServiceWriter).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServiceWriter {
    /// A JSON value was a different shape than expected (e.g. not an
    /// object where one was required).
    #[error("Invalid Type: {0}")]
    InvalidType(String),

    /// A required piece of the input, such as the service manifest, was
    /// missing.
    #[error("Not found: {0}")]
    NotFound(String),

    /// The input used a feature this writer doesn't (yet) support, such as
    /// an unrecognized HTTP verb or parameter location.
    #[error("Unimplemented: {0}")]
    Unimplemented(String),

    /// Writing to the destination [`Storage`](crate::Storage) failed.
    #[error(transparent)]
    Io {
        /// The underlying I/O error.
        #[from]
        source: io::Error,
    },

    /// Serializing or deserializing a value as JSON failed.
    #[error(transparent)]
    Json {
        /// The underlying JSON error.
        #[from]
        source: serde_json::Error,
    },

    /// Serializing the reconstructed `OpenAPI` document to YAML failed.
    #[error(transparent)]
    Yaml {
        /// The underlying YAML error.
        #[from]
        source: serde_yaml::Error,
    },
}

/// Shorthand for a [`Result`](std::result::Result) using [`ServiceWriter`]
/// as its error type.
pub type Result<T> = std::result::Result<T, ServiceWriter>;
