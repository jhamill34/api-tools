#![allow(clippy::std_instead_of_core)]

//!

use std::{io, num::TryFromIntError};

use execution_engine::error::ExecutionEngine;
use thiserror::Error;

///
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum APICaller {
    ///
    #[error("Operation not found: {0}")]
    OperationNotFound(String),

    ///
    #[error("Not Found: {0}")]
    NotFound(String),

    ///
    #[error("Missing required parameter: {0}")]
    MissingRequiredParameter(String),

    ///
    #[error("Expected to find defined auth parameter {0}")]
    InvalidAuthParameter(String),

    ///
    #[error("Expected credentials")]
    MissingCredentials,

    ///
    #[error("Missing Access Token")]
    MissingAccessToken,

    ///
    #[error("Invalid method: {0}")]
    InvalidMethod(String),

    ///
    #[error("Invalid Runtime Expression: {0}")]
    InvalidRuntimeExpression(String),

    ///
    #[error("Json Pointer Parser Error")]
    JsonPointerParseError {
        #[from]
        source: jsonptr::MalformedPointerError,
    },

    ///
    #[error("Json Pointer Index Error")]
    JsonPointerIndexError {
        #[from]
        source: jsonptr::Error,
    },

    ///
    #[error("Unable to simplify value")]
    SimpleValueAssertion,

    ///
    #[error("Get out! Lock has been poisoned: {0}")]
    PoisonedLock(String),

    ///
    #[error("Unimplemented: {0}")]
    Unimplemented(String),

    ///
    #[error("Paging strategy encountered an integer overflow")]
    PagingOverflow,

    ///
    #[error(transparent)]
    HttpMethodParsingError {
        ///
        #[from]
        source: http::method::InvalidMethod,
    },

    ///
    #[error(transparent)]
    ReqwestError {
        ///
        #[from]
        source: reqwest::Error,
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
    Io {
        ///
        #[from]
        source: io::Error,
    },

    ///
    #[error(transparent)]
    UrlParsingError {
        ///
        #[from]
        source: url::ParseError,
    },

    ///
    #[error(transparent)]
    InvalidHeaderName {
        ///
        #[from]
        source: reqwest::header::InvalidHeaderName,
    },

    ///
    #[error(transparent)]
    InvalidHeaderValue {
        ///
        #[from]
        source: reqwest::header::InvalidHeaderValue,
    },

    ///
    #[error(transparent)]
    HeaderValueToStringError {
        ///
        #[from]
        source: reqwest::header::ToStrError,
    },

    ///
    #[error(transparent)]
    IntegerConversion {
        ///
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

///
pub type Result<T> = std::result::Result<T, APICaller>;
