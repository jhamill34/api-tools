use std::collections::HashMap;

use core_entities::service::service_manifest_latest;
use reqwest::Url;
use rocket::{response::Redirect, State};

use crate::{error, structs};

#[get("/oauth/authorize")]
pub fn route(env: &State<structs::EnvironmentState>) -> Result<Redirect, error::CallbackResponse> {
    let creds = &env.lock_creds();

    let v1 = env.service.v1();
    let service = v1.manifest_latest();

    let Some(service_manifest_latest::Value::Swagger(service)) = &service.value else {
        return Err(error::CallbackResponse::BadRequest(
            "Service isn't a connector".to_string(),
        ));
    };

    let bad_request =
        || error::CallbackResponse::BadRequest("Connector doesn't use Oauth".to_string());
    let oauth_config = service
        .auth
        .as_ref()
        .and_then(|auth| auth.oauth_config.as_ref())
        .ok_or_else(bad_request)?;
    let creds = creds.as_oauth().ok_or_else(bad_request)?;

    let mut params: HashMap<&str, String> = HashMap::new();
    params.insert("redirect_uri", env.redirect_uri.clone());

    if oauth_config.response_type.is_empty() {
        params.insert("response_type", "code".to_string());
    } else {
        params.insert("response_type", oauth_config.response_type.clone());
    }

    if creds.client_id.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing client id".to_string(),
        ));
    }
    params.insert("client_id", creds.client_id.clone());
    // params.insert("state", "UUID");

    if oauth_config.scope.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing scopes".to_string(),
        ));
    }
    params.insert("scope", oauth_config.scope.clone());

    if !oauth_config.access_type.is_empty() {
        params.insert("access_type", oauth_config.access_type.clone());
    }

    if !oauth_config.prompt.is_empty() {
        params.insert("prompt", oauth_config.prompt.clone());
    }

    if !oauth_config.audience.is_empty() {
        params.insert("audience", oauth_config.audience.clone());
    }

    if oauth_config.auth_uri.is_empty() {
        return Err(error::CallbackResponse::InternalError(
            "Missing auth uri".to_string(),
        ));
    }

    let url = Url::parse_with_params(&oauth_config.auth_uri, params);

    url.map(|u| Redirect::to(u.to_string()))
        .map_err(|e| error::CallbackResponse::InternalError(e.to_string()))
}
