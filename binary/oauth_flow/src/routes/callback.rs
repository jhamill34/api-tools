use base64::{engine::general_purpose, Engine as _};
use core_entities::service::{service_manifest_latest, service_manifest_latest::oauth_config};
use credential_entities::credentials::Authentication;
use rocket::{Shutdown, State};
use std::collections::HashMap;

use super::{or_default, require_non_empty};
use crate::{error::CallbackError, structs};

/// Exchanges the OAuth authorization `code` for an access token and saves
/// it to the shared credentials, then shuts down the embedded server.
///
/// # Errors
/// Returns [`CallbackError`] if the connector isn't set up for OAuth, is
/// missing a required config/credential field, the token exchange request
/// fails, or the response doesn't contain an access token at the
/// configured path.
#[get("/oauth/callback?<code>")]
pub async fn route(
    code: &str,
    shutdown: Shutdown,
    env: &State<structs::EnvironmentState>,
) -> Result<(), CallbackError> {
    let client = reqwest::Client::new();

    let v1 = env.service.v1();
    let service = v1.manifest_latest();

    let Some(service_manifest_latest::Value::Swagger(service)) = &service.value else {
        return Err(CallbackError::NotOauthConnector(
            "Service isn't a connector",
        ));
    };

    let oauth_config = service
        .auth
        .as_ref()
        .and_then(|auth| auth.oauth_config.as_ref())
        .ok_or(CallbackError::NotOauthConnector(
            "Connector doesn't use Oauth",
        ))?;

    let (client_id, client_secret) = {
        let creds = env.lock_creds();
        let creds = creds.as_oauth().ok_or(CallbackError::NotOauthConnector(
            "Connector doesn't use Oauth",
        ))?;
        require_non_empty(&creds.client_id, "Missing client_id/client_secret")?;
        require_non_empty(&creds.client_secret, "Missing client_id/client_secret")?;

        (creds.client_id.clone(), creds.client_secret.clone())
    };

    let access_token_uri =
        require_non_empty(&oauth_config.access_token_uri, "Missing access token uri")?;

    let mut response_builder = client
        .post(access_token_uri)
        .header("Accept", "application/json");

    let mut body = HashMap::new();
    body.insert("grant_type", "authorization_code");
    body.insert("code", code);
    body.insert("redirect_uri", &env.redirect_uri);

    match oauth_config.parameter_location {
        oauth_config::ParameterLocation::Query => {
            let mut basic_credentials = String::new();
            general_purpose::STANDARD.encode_string(
                format!("{client_id}:{client_secret}"),
                &mut basic_credentials,
            );
            response_builder =
                response_builder.header("Authorization", &format!("Basic {basic_credentials}"));
        }
        oauth_config::ParameterLocation::Body => {
            body.insert("client_id", &client_id);
            body.insert("client_secret", &client_secret);
        }
    }

    let response = response_builder.form(&body).send().await?;

    let response_body: serde_json::Value = response.json().await?;

    let access_token_path = or_default(&oauth_config.access_token_path, "access_token");

    let expression = jmespath::compile(access_token_path)
        .map_err(|source| CallbackError::InvalidAccessTokenPath { source })?;

    let access_token = expression
        .search(response_body)
        .map_err(|source| CallbackError::AccessTokenNotFound { source })?;

    {
        let mut creds = env.lock_creds();
        let Authentication::Oauth(creds) = &mut *creds else {
            return Err(CallbackError::NotOauthConnector(
                "Connector doesn't use Oauth",
            ));
        };
        creds.access_token = access_token.as_string().cloned();
    }

    shutdown.notify();

    Ok(())
}
