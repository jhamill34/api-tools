use base64::{engine::general_purpose, Engine as _};
use core_entities::service::{service_manifest_latest, service_manifest_latest::oauth_config};
use credential_entities::credentials::Authentication;
use rocket::{Shutdown, State};
use std::collections::HashMap;

use crate::{error, structs};

#[get("/oauth/callback?<code>")]
pub async fn route(
    code: &str,
    shutdown: Shutdown,
    env: &State<structs::EnvironmentState>,
) -> Result<(), error::CallbackResponse> {
    let client = reqwest::Client::new();

    let v1 = env.service.v1();
    let service = v1.manifest_latest();

    let Some(service_manifest_latest::Value::Swagger(service)) = &service.value else {
        return Err(error::CallbackResponse::BadRequest(
            "Service isn't a connector".to_string(),
        ));
    };

    let bad_request = || {
        error::CallbackResponse::BadRequest("Connector doesn't use Oauth".to_string())
    };
    let oauth_config = service
        .auth
        .as_ref()
        .and_then(|auth| auth.oauth_config.as_ref())
        .ok_or_else(bad_request)?;

    let (client_id, client_secret) = {
        let creds = env.lock_creds();
        let creds = creds.as_oauth().ok_or_else(bad_request)?;
        if creds.client_secret.is_empty() || creds.client_id.is_empty() {
            return Err(error::CallbackResponse::InternalError(
                "Missing client_id/client_secret".to_string(),
            ));
        }

        (creds.client_id.clone(), creds.client_secret.clone())
    };

    if oauth_config.access_token_uri.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing access token uri".to_string(),
        ));
    }

    let mut response_builder = client
        .post(oauth_config.access_token_uri.clone())
        .header("Accept", "application/json");

    let mut body = HashMap::new();
    body.insert("grant_type", "authorization_code");
    body.insert("code", code);
    body.insert("redirect_uri", &env.redirect_uri);

    match oauth_config.parameter_location {
        oauth_config::ParameterLocation::Query => {
            let mut basic_credentials = String::new();
            general_purpose::STANDARD.encode_string(
                format!("{}:{}", client_id, client_secret),
                &mut basic_credentials,
            );
            response_builder =
                response_builder.header("Authorization", &format!("Basic {}", basic_credentials));
        }
        oauth_config::ParameterLocation::Body => {
            body.insert("client_id", &client_id);
            body.insert("client_secret", &client_secret);
        }
    }

    let response = response_builder.form(&body).send().await?;

    let response_body: Result<serde_json::Value, _> = response.json().await;

    let response_body = response_body?;

    let access_token_path = if oauth_config.access_token_path.is_empty() {
        String::from("access_token")
    } else {
        oauth_config.access_token_path.clone()
    };

    let expression = jmespath::compile(&access_token_path).map_err(|err| {
        error::CallbackResponse::InternalError(format!("Invalid access token path: {err}"))
    })?;

    let access_token = expression.search(response_body).map_err(|err| {
        error::CallbackResponse::InternalError(format!("Unable to find access token: {err}"))
    })?;

    {
        let mut creds = env.lock_creds();
        let Authentication::Oauth(creds) = &mut *creds else {
            return Err(error::CallbackResponse::BadRequest(
                "Connector doesn't use Oauth".to_string(),
            ));
        };
        creds.access_token = access_token.as_string().cloned();
    }

    shutdown.notify();

    Ok(())
}
