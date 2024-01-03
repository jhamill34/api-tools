use std::collections::HashMap;

use reqwest::Url;
use rocket::{response::Redirect, State};

use crate::{error, structs};

#[get("/oauth/authorize")]
pub fn route(env: &State<structs::EnvironmentState>) -> Result<Redirect, error::CallbackResponse> {
    let creds = &env.creds.lock().unwrap();

    let service = &env.service;
    let service = service.v1().manifest.v2();

    if !service.has_swagger() {
        return Err(error::CallbackResponse::BadRequest(
            "Service isn't a connector".to_string(),
        ));
    }

    let service = &service.swagger().auth;

    if !service.has_oauthConfig() || !creds.has_oauth() {
        return Err(error::CallbackResponse::BadRequest(
            "Connector doesn't use Oauth".to_string(),
        ));
    }

    let oauth_config = service.oauthConfig();
    let creds = creds.oauth();

    let mut params: HashMap<&str, String> = HashMap::new();
    params.insert("redirect_uri", env.redirect_uri.clone());

    if oauth_config.responseType.is_empty() {
        params.insert("response_type", "code".to_string());
    } else {
        params.insert("response_type", oauth_config.responseType.clone());
    }

    if creds.clientId.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing client id".to_string(),
        ));
    }
    params.insert("client_id", creds.clientId.clone());
    // params.insert("state", "UUID");

    if oauth_config.scope.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing scopes".to_string(),
        ));
    }
    params.insert("scope", oauth_config.scope.clone());

    if !oauth_config.accessType.is_empty() {
        params.insert("access_type", oauth_config.accessType.clone());
    }

    if !oauth_config.prompt.is_empty() {
        params.insert("prompt", oauth_config.prompt.clone());
    }

    if !oauth_config.audience.is_empty() {
        params.insert("audience", oauth_config.audience.clone());
    }

    if oauth_config.authUri.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing auth uri".to_string(),
        ));
    }

    let url = Url::parse_with_params(&oauth_config.authUri, params);

    url.map(|u| Redirect::to(u.to_string()))
        .map_err(|e| error::CallbackResponse::InternalError(e.to_string()))
}
