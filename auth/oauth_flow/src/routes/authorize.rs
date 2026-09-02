use std::collections::HashMap;

use core_entities::service::service_manifest_latest;
use reqwest::Url;
use rocket::{response::Redirect, State};

use super::{or_default, require_non_empty};
use crate::{error::CallbackError, structs};

#[get("/oauth/authorize")]
pub fn route(env: &State<structs::EnvironmentState>) -> Result<Redirect, CallbackError> {
    let creds = &env.lock_creds();

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
    let creds = creds.as_oauth().ok_or(CallbackError::NotOauthConnector(
        "Connector doesn't use Oauth",
    ))?;

    let mut params: HashMap<&str, &str> = HashMap::new();
    params.insert("redirect_uri", &env.redirect_uri);

    let response_type = or_default(&oauth_config.response_type, "code");
    params.insert("response_type", response_type);

    let client_id = require_non_empty(&creds.client_id, "Missing client id")?;
    params.insert("client_id", client_id);
    // params.insert("state", "UUID");

    let scope = require_non_empty(&oauth_config.scope, "Missing scopes")?;
    params.insert("scope", scope);

    insert_if_present(&mut params, "access_type", &oauth_config.access_type);
    insert_if_present(&mut params, "prompt", &oauth_config.prompt);
    insert_if_present(&mut params, "audience", &oauth_config.audience);

    let auth_uri = require_non_empty(&oauth_config.auth_uri, "Missing auth uri")?;

    let url = Url::parse_with_params(auth_uri, params)?;

    Ok(Redirect::to(url.to_string()))
}

/// Inserts `key`/`value` into `params` only if `value` is non-empty - for
/// query parameters that are omitted entirely rather than defaulted when
/// unset.
fn insert_if_present<'value>(
    params: &mut HashMap<&'value str, &'value str>,
    key: &'value str,
    value: &'value str,
) {
    if !value.is_empty() {
        params.insert(key, value);
    }
}
