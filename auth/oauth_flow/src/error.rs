use std::{env::VarError, io};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExecutableErr {
    #[error("")]
    EnvironmentVariableError {
        #[from]
        source: VarError,
    },

    #[error("")]
    Io {
        #[from]
        source: io::Error,
    },

    #[error("")]
    Json {
        #[from]
        source: serde_json::Error,
    },

    #[error("")]
    Protobuf {
        #[from]
        source: protobuf_json_mapping::PrintError,
    },

    #[error("")]
    RocketError {
        #[from]
        source: rocket::Error,
    },
}

pub type Result<T> = std::result::Result<T, ExecutableErr>;

#[derive(Responder)]
pub enum CallbackResponse {
    #[response(status = 400)]
    BadRequest(String),

    #[response(status = 500)]
    InternalError(String),
}

impl From<reqwest::Error> for CallbackResponse {
    fn from(value: reqwest::Error) -> Self {
        Self::InternalError(value.to_string())
    }
}

impl From<io::Error> for CallbackResponse {
    fn from(value: io::Error) -> Self {
        Self::InternalError(value.to_string())
    }
}
