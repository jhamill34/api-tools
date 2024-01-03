#![allow(clippy::std_instead_of_core)]

//!

use std::io;

use thiserror::Error;

///
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServiceWriter {
    ///
    #[error("Invalid Type: {0}")]
    InvalidType(String),

    ///
    #[error("Not found: {0}")]
    NotFound(String),

    ///
    #[error("Unimplemented: {0}")]
    Unimplemented(String),

    ///
    #[error(transparent)]
    Protobuf {
        ///
        #[from]
        source: protobuf_json_mapping::PrintError,
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
    Json {
        ///
        #[from]
        source: serde_json::Error,
    },

    ///
    #[error(transparent)]
    Yaml {
        ///
        #[from]
        source: serde_yaml::Error,
    },
}

///
pub type Result<T> = std::result::Result<T, ServiceWriter>;
