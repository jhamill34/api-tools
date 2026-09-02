use std::{env::VarError, io};

use rocket::{
    http::Status,
    response::{self, Responder},
    Request,
};
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
    RocketError {
        #[from]
        source: rocket::Error,
    },
}

pub type Result<T> = std::result::Result<T, ExecutableErr>;

/// Everything a `/oauth/*` route handler ([`crate::routes::authorize`],
/// [`crate::routes::callback`]) can fail with. Every conversion here goes
/// through `thiserror`'s `#[from]`/`#[source]`, so an underlying error's
/// chain ([`std::error::Error::source`]) survives instead of being
/// flattened into a bare `.to_string()` at the point of conversion - unlike
/// the hand-rolled `CallbackResponse` this replaced, which only ever held a
/// `String`. [`Responder`] is implemented below, with [`CallbackError::status`]
/// deciding the HTTP status.
#[derive(Error, Debug)]
pub enum CallbackError {
    /// The configured service isn't set up for OAuth at all: not a Swagger
    /// connector, its manifest has no `oauth_config`, or its saved
    /// credentials aren't OAuth credentials. Reported as a 400 - the
    /// caller asked to authenticate a service that doesn't support it.
    #[error("{0}")]
    NotOauthConnector(&'static str),

    /// A field the connector's manifest or saved credentials was expected
    /// to set is empty. Reported as a 500 - it's a misconfiguration of the
    /// connector, not a bad request from the OAuth callback caller.
    #[error("{0}")]
    MissingConfig(&'static str),

    #[error(transparent)]
    InvalidRedirectUrl(#[from] url::ParseError),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("invalid access token path: {source}")]
    InvalidAccessTokenPath {
        #[source]
        source: jmespath::JmespathError,
    },

    #[error("access token not found in the OAuth response: {source}")]
    AccessTokenNotFound {
        #[source]
        source: jmespath::JmespathError,
    },
}

impl CallbackError {
    /// The HTTP status this error should be reported to the OAuth callback
    /// caller as.
    fn status(&self) -> Status {
        match self {
            Self::NotOauthConnector(_) => Status::BadRequest,
            Self::MissingConfig(_)
            | Self::InvalidRedirectUrl(_)
            | Self::Http(_)
            | Self::InvalidAccessTokenPath { .. }
            | Self::AccessTokenNotFound { .. } => Status::InternalServerError,
        }
    }
}

impl<'r, 'o: 'r> Responder<'r, 'o> for CallbackError {
    fn respond_to(self, request: &'r Request<'_>) -> response::Result<'o> {
        (self.status(), self.to_string()).respond_to(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_io_error_keeps_its_own_message_instead_of_being_empty() {
        let source = io::Error::other("boom");
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

    #[test]
    fn not_oauth_connector_is_a_bad_request() {
        let err = CallbackError::NotOauthConnector("Service isn't a connector");

        assert_eq!(err.status(), Status::BadRequest);
        assert_eq!(err.to_string(), "Service isn't a connector");
    }

    #[test]
    fn missing_config_is_an_internal_error() {
        let err = CallbackError::MissingConfig("Missing client id");

        assert_eq!(err.status(), Status::InternalServerError);
        assert_eq!(err.to_string(), "Missing client id");
    }

    #[test]
    fn access_token_not_found_keeps_the_source_error_chain() {
        use std::error::Error as _;

        let source = jmespath::compile("(").expect_err("expected a compile error to wrap");
        let err = CallbackError::AccessTokenNotFound { source };

        assert_eq!(err.status(), Status::InternalServerError);
        assert!(err.source().is_some());
    }
}
