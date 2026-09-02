//! Errors produced while loading a service, its credentials, or its
//! override configuration.

use std::io;

use thiserror::Error;

/// Failure modes of [`ServiceLoader::load`](crate::ServiceLoader::load) and
/// the `OpenAPI` loader it delegates to.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServiceLoader {
    /// Applying override configuration to a loaded manifest failed.
    #[error("Override Error: {0}")]
    OverrideError(String),

    /// A required piece of the input, such as a manifest section, was
    /// missing.
    #[error("Not found: {0}")]
    NotFound(String),

    /// A `$ref` chain in the `OpenAPI` document referenced itself.
    #[error("Cyclical Reference: {0}")]
    CyclicalReference(String),

    /// An `OpenAPI` schema's `type` wasn't one this loader recognizes.
    #[error("Unknown Schema Type")]
    UnknownSchemaType,

    /// A required field was absent from the input document.
    #[error("Missing Required Field: {0}")]
    MissingRequiredField(String),

    /// A `$ref` string wasn't a well-formed JSON pointer.
    #[error("Json Pointer Parser Error: {source}")]
    JsonPointerParseError {
        #[from]
        source: jsonptr::MalformedPointerError,
    },

    /// Resolving a JSON pointer against the document failed.
    #[error("Json Pointer Index Error: {source}")]
    JsonPointerIndexError {
        #[from]
        source: jsonptr::Error,
    },

    /// A value was a different shape than expected.
    #[error("Wrong Type (field={field}, expected={expected})")]
    WrongType {
        /// The field that had the wrong type.
        field: String,

        /// The type that was expected.
        expected: String,
    },

    /// Reading from the source [`Fetcher`](crate::Fetcher) failed.
    #[error(transparent)]
    Io {
        /// The underlying I/O error.
        #[from]
        source: io::Error,
    },

    /// Parsing or serializing a value as JSON failed.
    #[error(transparent)]
    Json {
        /// The underlying JSON error.
        #[from]
        source: serde_json::Error,
    },

    /// Parsing the `OpenAPI` document as YAML failed.
    #[error("Unable to load YAML spec: {source}")]
    Yaml {
        /// The underlying YAML error.
        #[from]
        source: serde_yaml::Error,
    },

    /// Some other, adapter-specific failure.
    #[error(transparent)]
    Other {
        /// The wrapped error.
        source: anyhow::Error,
    },
}

/// Shorthand for a [`Result`](std::result::Result) using [`ServiceLoader`]
/// as its error type.
pub type Result<T> = std::result::Result<T, ServiceLoader>;
