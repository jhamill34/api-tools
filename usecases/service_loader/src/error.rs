#![allow(clippy::std_instead_of_core)]

//!

use std::io;

use thiserror::Error;

///
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServiceLoader {
    ///
    #[error("Override Error: {0}")]
    OverrideError(String),

    ///
    #[error("Not found: {0}")]
    NotFound(String),

    ///
    #[error("Cyclical Reference: {0}")]
    CyclicalReference(String),

    ///
    #[error("Unknown Schema Type")]
    UnknownSchemaType,

    ///
    #[error("Missing Required Field: {0}")]
    MissingRequiredField(String),

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
    #[error("Wrong Type (field={field}, expected={expected})")]
    WrongType {
        ///
        field: String,

        ///
        expected: String,
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
    #[error("Unable to load YAML spec")]
    Yaml {
        ///
        #[from]
        source: serde_yaml::Error,
    },

    ///
    #[error(transparent)]
    ProtobufParse {
        ///
        #[from]
        source: protobuf_json_mapping::ParseError,
    },

    /// TODO: Rename to OutputPortError
    #[error(transparent)]
    Other {
        ///
        source: anyhow::Error,
    },
}

///
pub type Result<T> = std::result::Result<T, ServiceLoader>;
