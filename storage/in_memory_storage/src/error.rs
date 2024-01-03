#![allow(clippy::std_instead_of_core)]

//!

use service_loader::error::ServiceLoader;
use thiserror::Error;

///
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OperationRepo {
    ///
    #[error("Get out! Lock has been poisened: {0}")]
    LockingError(String),

    ///
    #[error("Operation not found: {0}")]
    OperationNotFound(String),

    ///
    #[error(transparent)]
    ProtobufSerialize {
        ///
        #[from]
        source: protobuf_json_mapping::PrintError,
    },

    ///
    #[error(transparent)]
    ProtobufParse {
        ///
        #[from]
        source: protobuf_json_mapping::ParseError,
    },

    ///
    #[error(transparent)]
    Json {
        ///
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

///
pub type Result<T> = std::result::Result<T, OperationRepo>;
