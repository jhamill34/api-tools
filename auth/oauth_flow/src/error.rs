use std::{env::VarError, io};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExecutableErr {
    #[error(transparent)]
    EnvironmentVariableError {
        #[from]
        source: VarError,
    },

    #[error(transparent)]
    Io {
        #[from]
        source: io::Error,
    },

    #[error(transparent)]
    Json {
        #[from]
        source: serde_json::Error,
    },

    #[error(transparent)]
    Protobuf {
        #[from]
        source: protobuf_json_mapping::PrintError,
    },

    #[error(transparent)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_io_error_keeps_its_own_message_instead_of_being_empty() {
        let source = io::Error::new(io::ErrorKind::Other, "boom");
        let err = ExecutableErr::from(source);

        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn wrapped_env_var_error_keeps_its_own_message_instead_of_being_empty() {
        let source = std::env::var("CLAUDE_DEFINITELY_UNSET_VAR_FOR_TEST").unwrap_err();
        let expected = source.to_string();
        let err = ExecutableErr::from(source);

        assert_eq!(err.to_string(), expected);
        assert!(!err.to_string().is_empty());
    }
}
