use base64::{engine::general_purpose, Engine as _};
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

    let creds = &env.creds;

    let service = &env.service;
    let service = service.v1().manifest.v2();

    if !service.has_swagger() {
        return Err(error::CallbackResponse::BadRequest(
            "Service isn't a connector".to_string(),
        ));
    }

    let service = &service.swagger().auth;
    if !service.has_oauthConfig() {
        return Err(error::CallbackResponse::BadRequest(
            "Connector doesn't use Oauth".to_string(),
        ));
    }
    let oauth_config = service.oauthConfig();

    let (client_id, client_secret) = {
        let creds = creds.lock().unwrap();
        if !creds.has_oauth() {
            return Err(error::CallbackResponse::BadRequest(
                "Connector doesn't use Oauth".to_string(),
            ));
        }

        let creds = creds.oauth();
        if creds.clientSecret.is_empty() || creds.clientId.is_empty() {
            return Err(error::CallbackResponse::InternalError(
                "Missing client_id/client_secret".to_string(),
            ));
        }

        (creds.clientId.clone(), creds.clientSecret.clone())
    };

    if oauth_config.accessTokenUri.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing access token uri".to_string(),
        ));
    }

    let mut response_builder = client
        .post(oauth_config.accessTokenUri.clone())
        .header("Accept", "application/json");

    let mut body = HashMap::new();
    body.insert("grant_type", "authorization_code");
    body.insert("code", code);
    body.insert("redirect_uri", &env.redirect_uri);

    let param_location = oauth_config.parameterLocation.enum_value_or_default();
    match param_location {
        core_entities::service::service_manifest_latest::oauth_config::ParameterLocation::QUERY => {
            let mut basic_credentials = String::new();
            general_purpose::STANDARD.encode_string(
                format!("{}:{}", client_id, client_secret),
                &mut basic_credentials,
            );
            response_builder =
                response_builder.header("Authorization", &format!("Basic {}", basic_credentials));
        }
        core_entities::service::service_manifest_latest::oauth_config::ParameterLocation::BODY => {
            body.insert("client_id", &client_id);
            body.insert("client_secret", &client_secret);
        }
    }

    let response = response_builder.form(&body).send().await?;

    let response_body: Result<serde_json::Value, _> = response.json().await;

    let response_body = response_body?;

    let access_token_path = if oauth_config.accessTokenPath.is_empty() {
        String::from("access_token")
    } else {
        oauth_config.accessTokenPath.clone()
    };

    let expression = jmespath::compile(&access_token_path).map_err(|_| {
        error::CallbackResponse::InternalError("Invalid access token path".to_string())
    })?;

    let access_token = expression.search(response_body).map_err(|_| {
        error::CallbackResponse::InternalError("Unable to find access token".to_string())
    })?;

    {
        let mut creds = creds.lock().unwrap();
        let creds = creds.mut_oauth();
        creds.accessToken = access_token.as_string().cloned();
    }

    shutdown.notify();

    Ok(())
}
