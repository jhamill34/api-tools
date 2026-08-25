#![allow(clippy::std_instead_of_core)]

//! Errors produced while making an API call.

use std::{io, num::TryFromIntError};

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

/// Failure modes of [`APICaller::run`](crate::APICaller).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum APICaller {
    /// The requested operation wasn't defined on the connector bundle.
    #[error("Operation not found: {0}")]
    OperationNotFound(String),

    /// A required piece of pagination configuration was missing.
    #[error("Not Found: {0}")]
    NotFound(String),

    /// A parameter marked `required` in the manifest wasn't supplied.
    #[error("Missing required parameter: {0}")]
    MissingRequiredParameter(String),

    /// The manifest's auth configuration was missing a parameter this auth
    /// type requires (e.g. the header/query/path key to use).
    #[error("Expected to find defined auth parameter {0}")]
    InvalidAuthParameter(String),

    /// The operation requires credentials but none were supplied.
    #[error("Expected credentials")]
    MissingCredentials,

    /// OAuth credentials were supplied but had no access token.
    #[error("Missing Access Token")]
    MissingAccessToken,

    /// The operation's HTTP method wasn't one this runner recognizes.
    #[error("Invalid method: {0}")]
    InvalidMethod(String),

    /// A pagination runtime expression (e.g. `$request.query.page`) wasn't
    /// one of the recognized shapes.
    #[error("Invalid Runtime Expression: {0}")]
    InvalidRuntimeExpression(String),

    /// A `resultsPath`/cursor path wasn't a well-formed JSON pointer.
    #[error("Json Pointer Parser Error")]
    JsonPointerParseError {
        #[from]
        source: jsonptr::MalformedPointerError,
    },

    /// Resolving a JSON pointer against a response body failed.
    #[error("Json Pointer Index Error")]
    JsonPointerIndexError {
        #[from]
        source: jsonptr::Error,
    },

    /// A parameter value was an array or object where a scalar was
    /// expected (e.g. as a header or query value).
    #[error("Unable to simplify value")]
    SimpleValueAssertion,

    /// The shared log file's lock was poisoned by a panic in another
    /// thread while holding it.
    #[error("Get out! Lock has been poisoned: {0}")]
    PoisonedLock(String),

    /// The manifest used a feature this runner doesn't (yet) support, such
    /// as an unrecognized parameter location or auth type.
    #[error("Unimplemented: {0}")]
    Unimplemented(String),

    /// A pagination counter (page number, item total) overflowed `i32`.
    #[error("Paging strategy encountered an integer overflow")]
    PagingOverflow,

    /// The resolved HTTP method string wasn't a valid method.
    #[error(transparent)]
    HttpMethodParsingError {
        /// The underlying parse error.
        #[from]
        source: http::method::InvalidMethod,
    },

    /// The underlying HTTP request failed.
    #[error(transparent)]
    ReqwestError {
        /// The underlying `reqwest` error.
        #[from]
        source: reqwest::Error,
    },

    /// Serializing or deserializing a value as JSON failed.
    #[error(transparent)]
    Json {
        /// The underlying JSON error.
        #[from]
        source: serde_json::Error,
    },

    /// Writing to the shared log file failed.
    #[error(transparent)]
    Io {
        /// The underlying I/O error.
        #[from]
        source: io::Error,
    },

    /// Building the request URL failed.
    #[error(transparent)]
    UrlParsingError {
        /// The underlying URL parse error.
        #[from]
        source: url::ParseError,
    },

    /// A header parameter's name wasn't a valid HTTP header name.
    #[error(transparent)]
    InvalidHeaderName {
        /// The underlying header-name error.
        #[from]
        source: reqwest::header::InvalidHeaderName,
    },

    /// A header parameter's value wasn't a valid HTTP header value.
    #[error(transparent)]
    InvalidHeaderValue {
        /// The underlying header-value error.
        #[from]
        source: reqwest::header::InvalidHeaderValue,
    },

    /// A response header's value wasn't valid UTF-8 for logging.
    #[error(transparent)]
    HeaderValueToStringError {
        /// The underlying conversion error.
        #[from]
        source: reqwest::header::ToStrError,
    },

    /// A numeric conversion (e.g. array length to `i32`, `i32` to `usize`)
    /// overflowed its target type.
    #[error(transparent)]
    IntegerConversion {
        /// The underlying conversion error.
        #[from]
        source: TryFromIntError,
    },
}

impl From<APICaller> for ExecutionEngine {
    #[inline]
    fn from(value: APICaller) -> Self {
        Self::Other {
            source: value.into(),
        }
    }
}

/// Shorthand for a [`Result`](std::result::Result) using [`APICaller`] as
/// its error type.
pub type Result<T> = std::result::Result<T, APICaller>;
