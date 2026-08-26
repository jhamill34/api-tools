#![allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "pagination limit/offset casts between i32/usize/u64 are unaudited; \
              tracked as a dedicated numeric-safety follow-up to issue #1, not \
              rushed into this lint-hygiene pass"
)]

//! A [`DataConnectionRunner`] adapter that resolves an operation's request
//! (method, endpoint, params, auth) and executes it over HTTP, handling
//! pagination across multiple requests when configured.

mod constants;
pub mod error;

use std::collections::HashMap;

use base64::Engine as _;
use common_data_structures::log_writer::LogWriter;
use core_entities::service::{pagination, Operation, Parameter, SwaggerService};
use credential_entities::credentials::Authentication;
use execution_engine::services::{
    AsyncDataConnectionRunner, DataConnectionRunner, DataConnectorBundle, EngineInputContext,
};
use http::{HeaderMap, HeaderName, HeaderValue};

/// Converts a scalar JSON value to its string form, for use as a header,
/// query, or path parameter. Errors on an array or object, which have no
/// unambiguous scalar representation.
fn simplify_value(value: &serde_json::Value) -> error::Result<String> {
    match value {
        serde_json::Value::String(val) => Ok(val.to_string()),
        serde_json::Value::Bool(val) => Ok(val.to_string()),
        serde_json::Value::Number(val) => Ok(val.to_string()),
        serde_json::Value::Null => Ok("null".to_owned()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Err(error::APICaller::SimpleValueAssertion)
        }
    }
}

/// Applies [`simplify_value`] across a map of JSON values, e.g. a set of
/// path or query parameters.
fn simplify_value_map<'item, I>(values: I) -> error::Result<HashMap<String, String>>
where
    I: Iterator<Item = (&'item String, &'item serde_json::Value)>,
{
    values
        .map(|(key, value)| Ok((key.to_string(), simplify_value(value)?)))
        .collect()
}

/// Extracts the paginated results from a raw response, by resolving the
/// configured pagination strategy's `resultsPath` (stripped of its
/// `$response.body#` runtime-expression prefix) as a JSON pointer into
/// `result`. Falls back to the whole `result` when there's no pagination
/// configured, the path resolves to the document root, or the strategy is
/// next-URL-based (which has no separate results path to extract).
fn find_results<'item>(
    result: &'item serde_json::Value,
    pagination_config: &Option<pagination::Value>,
) -> error::Result<&'item serde_json::Value> {
    let result = if let Some(pagination) = pagination_config {
        match pagination {
            core_entities::service::pagination::Value::PageOffset(page_offset) => {
                let path = page_offset.resultsPath.jmesPath();
                let path = path
                    .strip_prefix(constants::RESPONSE_BODY_PREFIX)
                    .unwrap_or(path);

                let path = if path.starts_with('/') {
                    path.to_owned()
                } else {
                    format!("/{path}")
                };

                if path == "/" {
                    result
                } else {
                    let path = path.parse::<jsonptr::Pointer>()?;
                    path.resolve(result)?
                }
            }
            core_entities::service::pagination::Value::MultiCursor(cursor) => {
                let path = cursor.resultsPath.jmesPath();
                let path = path
                    .strip_prefix(constants::RESPONSE_BODY_PREFIX)
                    .unwrap_or(path);

                let path = if path.starts_with('/') {
                    path.to_owned()
                } else {
                    format!("/{path}")
                };

                if path == "/" {
                    result
                } else {
                    let path = path.parse::<jsonptr::Pointer>()?;
                    path.resolve(result)?
                }
            }
            core_entities::service::pagination::Value::Offset(offset) => {
                let path = offset.resultsPath.jmesPath();
                let path = path
                    .strip_prefix(constants::RESPONSE_BODY_PREFIX)
                    .unwrap_or(path);

                let path = if path.starts_with('/') {
                    path.to_owned()
                } else {
                    format!("/{path}")
                };

                if path == "/" {
                    result
                } else {
                    let path = path.parse::<jsonptr::Pointer>()?;
                    path.resolve(result)?
                }
            }
            core_entities::service::pagination::Value::Unpaginated(unpaginated) => {
                let path = unpaginated.resultsPath.jmesPath();
                let path = path
                    .strip_prefix(constants::RESPONSE_BODY_PREFIX)
                    .unwrap_or(path);

                let path = if path.starts_with('/') {
                    path.to_owned()
                } else {
                    format!("/{path}")
                };

                if path == "/" {
                    result
                } else {
                    let path = path.parse::<jsonptr::Pointer>()?;
                    path.resolve(result)?
                }
            }
            core_entities::service::pagination::Value::NextUrl(_) | _ => result,
        }
    } else {
        result
    };
    Ok(result)
}

/// The in-progress state of a single HTTP request being built up from an
/// operation's manifest and the caller's runtime params, one page at a
/// time.
#[derive(Default)]
struct APICallState {
    /// The resolved HTTP method name (e.g. `"GET"`).
    method: String,

    /// The resolved request URL, before path-parameter substitution and
    /// query-string assembly.
    endpoint: String,

    /// Values to send as HTTP headers.
    header_params: HashMap<String, serde_json::Value>,

    /// Values to send as query-string parameters.
    query_params: HashMap<String, serde_json::Value>,

    /// Values to substitute into `{placeholder}` segments of the endpoint.
    path_params: HashMap<String, serde_json::Value>,

    /// The JSON request body, if any.
    body: Option<serde_json::Value>,
}

impl APICallState {
    /// Sends the built-up request over `client`, logging the request and
    /// response to `log`, and returns the parsed response body (or
    /// [`serde_json::Value::Null`] for an empty body, or the raw text
    /// wrapped in a JSON string if it isn't valid JSON).
    fn send(
        &self,
        id: &str,
        client: &reqwest::blocking::Client,
        log: &LogWriter,
    ) -> error::Result<serde_json::Value> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        let method = self.method.parse::<reqwest::Method>()?;
        let endpoint = self.resolve_endpoint()?;
        log.write_all(b"==============================\n")?;
        log.write_all(format!("ID = {id}\n").as_bytes())?;
        log.write_all(format!("Time = {now}\n").as_bytes())?;
        log.write_all(b"[REQUEST]\n")?;
        log.write_all(format!("{} {}\n", &self.method, &endpoint).as_bytes())?;

        let mut builder = client.request(method, endpoint);

        let headers: error::Result<HeaderMap> = self
            .header_params
            .iter()
            .map(|(key, val)| {
                let name = key.parse::<HeaderName>()?;
                let value = simplify_value(val).and_then(|value| {
                    value.parse::<HeaderValue>().map_err(error::APICaller::from)
                })?;

                Ok((name, value))
            })
            .collect();
        let headers = headers?;

        log.write_all(b"Headers = \n")?;
        for (key, value) in &headers {
            log.write_all(format!("  {}: {}\n", key.as_str(), value.to_str()?).as_bytes())?;
        }

        builder = builder.headers(headers);

        if let Some(body) = &self.body {
            log.write_all(format!("\n{}\n", serde_json::to_string_pretty(body)?).as_bytes())?;
            builder = builder.json(body);
        } else {
            log.write_all(b"\nNo Body\n")?;
        }

        log.write_all(b"\n")?;

        log.write_all(b"[RESPONSE]\n")?;
        let response = builder.send()?;

        log.write_all(format!("Status = {}\n", response.status()).as_bytes())?;

        log.write_all(b"Headers = \n")?;
        for (key, value) in response.headers() {
            log.write_all(format!("  {}: {}\n", key.as_str(), value.to_str()?).as_bytes())?;
        }

        let response_body: String = response.text()?;
        if response_body.is_empty() {
            log.write_all(b"\nNo Content\n")?;
            Ok(serde_json::Value::Null)
        } else {
            let response = match serde_json::from_str(&response_body) {
                Ok(value) => value,
                Err(_) => serde_json::Value::String(response_body),
            };
            log.write_all(format!("\n{}\n", serde_json::to_string_pretty(&response)?).as_bytes())?;

            Ok(response)
        }
    }

    /// The async sibling of [`Self::send`] - identical request-building
    /// logic (endpoint/header/body resolution, logging), just built on
    /// `reqwest::Client`'s non-blocking `.send()`/`.text()` instead of the
    /// blocking client's, so it never occupies a thread while waiting on
    /// the network.
    async fn send_async(
        &self,
        id: &str,
        client: &reqwest::Client,
        log: &LogWriter,
    ) -> error::Result<serde_json::Value> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        let method = self.method.parse::<reqwest::Method>()?;
        let endpoint = self.resolve_endpoint()?;
        log.write_all(b"==============================\n")?;
        log.write_all(format!("ID = {id}\n").as_bytes())?;
        log.write_all(format!("Time = {now}\n").as_bytes())?;
        log.write_all(b"[REQUEST]\n")?;
        log.write_all(format!("{} {}\n", &self.method, &endpoint).as_bytes())?;

        let mut builder = client.request(method, endpoint);

        let headers: error::Result<HeaderMap> = self
            .header_params
            .iter()
            .map(|(key, val)| {
                let name = key.parse::<HeaderName>()?;
                let value = simplify_value(val).and_then(|value| {
                    value.parse::<HeaderValue>().map_err(error::APICaller::from)
                })?;

                Ok((name, value))
            })
            .collect();
        let headers = headers?;

        log.write_all(b"Headers = \n")?;
        for (key, value) in &headers {
            log.write_all(format!("  {}: {}\n", key.as_str(), value.to_str()?).as_bytes())?;
        }

        builder = builder.headers(headers);

        if let Some(body) = &self.body {
            log.write_all(format!("\n{}\n", serde_json::to_string_pretty(body)?).as_bytes())?;
            builder = builder.json(body);
        } else {
            log.write_all(b"\nNo Body\n")?;
        }

        log.write_all(b"\n")?;

        log.write_all(b"[RESPONSE]\n")?;
        let response = builder.send().await?;

        log.write_all(format!("Status = {}\n", response.status()).as_bytes())?;

        log.write_all(b"Headers = \n")?;
        for (key, value) in response.headers() {
            log.write_all(format!("  {}: {}\n", key.as_str(), value.to_str()?).as_bytes())?;
        }

        let response_body: String = response.text().await?;
        if response_body.is_empty() {
            log.write_all(b"\nNo Content\n")?;
            Ok(serde_json::Value::Null)
        } else {
            let response = match serde_json::from_str(&response_body) {
                Ok(value) => value,
                Err(_) => serde_json::Value::String(response_body),
            };
            log.write_all(format!("\n{}\n", serde_json::to_string_pretty(&response)?).as_bytes())?;

            Ok(response)
        }
    }

    /// Substitutes `path_params` into `endpoint`'s `{placeholder}` segments
    /// and appends `query_params` as a query string, producing the final
    /// request URL.
    fn resolve_endpoint(&self) -> error::Result<reqwest::Url> {
        let mut endpoint = self.endpoint.clone();

        let params = simplify_value_map(self.path_params.iter())?;

        for (key, value) in params {
            let key = ["{", &key, "}"].join("");
            let value = urlencoding::encode(&value);
            endpoint = endpoint.replace(&key, &value);
        }

        let query = simplify_value_map(self.query_params.iter())?;

        let url = match query.len() {
            0 => reqwest::Url::parse(&endpoint),
            _ => reqwest::Url::parse_with_params(&endpoint, query),
        }?;

        Ok(url)
    }

    /// Joins `base_url` and `path` (trimming the shared `/` between them)
    /// into `endpoint`.
    fn set_endpoint(&mut self, base_url: &str, path: &str) {
        let base_url = base_url.strip_suffix('/').unwrap_or(base_url);
        let path = path.strip_prefix('/').unwrap_or(path);

        self.endpoint = format!("{base_url}/{path}");
    }

    /// Resolves `operation.method` to its HTTP method name. Errors on the
    /// unset/default variant, since there's no sensible method to fall
    /// back to.
    fn set_method(&mut self, operation: &Operation) -> error::Result<()> {
        match operation.method.enum_value_or_default() {
            core_entities::service::operation::HttpMethodType::POST => {
                self.method = String::from("POST");
            }
            core_entities::service::operation::HttpMethodType::GET => {
                self.method = String::from("GET");
            }
            core_entities::service::operation::HttpMethodType::PUT => {
                self.method = String::from("PUT");
            }
            core_entities::service::operation::HttpMethodType::PATCH => {
                self.method = String::from("PATCH");
            }
            core_entities::service::operation::HttpMethodType::DELETE => {
                self.method = String::from("DELETE");
            }
            core_entities::service::operation::HttpMethodType::HEAD => {
                self.method = String::from("HEAD");
            }
            core_entities::service::operation::HttpMethodType::OPTIONS => {
                self.method = String::from("OPTIONS");
            }
            core_entities::service::operation::HttpMethodType::TRACE => {
                self.method = String::from("TRACE");
            }
            core_entities::service::operation::HttpMethodType::HTTP_METHOD_TYPE_NONE => {
                return Err(error::APICaller::InvalidMethod("NONE".into()));
            }
        };

        Ok(())
    }

    /// Sets the request body, adding a `Content-Type: application/json`
    /// header when `body` is present.
    fn set_body(&mut self, body: Option<serde_json::Value>) {
        if body.is_some() {
            self.header_params.insert(
                "Content-Type".to_owned(),
                serde_json::Value::String("application/json".into()),
            );
        }
        self.body = body;
    }

    /// Looks up each of `parameters` in `params` and routes its value to
    /// the matching query/header/path map. Errors if a `required`
    /// parameter is missing and `fail_on_required` is set, or if a
    /// parameter's location isn't a query/header/path (e.g. cookie, which
    /// this runner doesn't support). Always sets a default `User-Agent`
    /// header.
    fn collect_params(
        &mut self,
        params: &serde_json::Value,
        parameters: &[Parameter],
        fail_on_required: bool,
    ) -> error::Result<()> {
        for defined_param in parameters {
            let value = params.get(&defined_param.name);
            if defined_param.required && value.is_none() && fail_on_required {
                return Err(error::APICaller::MissingRequiredParameter(
                    defined_param.name.clone(),
                ));
            }

            if let Some(value) = value {
                match defined_param.in_.enum_value_or_default() {
                    core_entities::service::parameter::InType::QUERY => {
                        self.query_params
                            .insert(defined_param.name.clone(), value.clone());
                    }
                    core_entities::service::parameter::InType::HEADER => {
                        self.header_params
                            .insert(defined_param.name.clone(), value.clone());
                    }
                    core_entities::service::parameter::InType::PATH => {
                        self.path_params
                            .insert(defined_param.name.clone(), value.clone());
                    }
                    core_entities::service::parameter::InType::IN_TYPE_NONE
                    | core_entities::service::parameter::InType::COOKIE
                    | core_entities::service::parameter::InType::HEADERS => {
                        return Err(error::APICaller::Unimplemented(
                            "Http Method Unimplemented".into(),
                        ));
                    }
                }
            }
        }

        self.header_params
            .insert("User-Agent".to_owned(), "APICLI/1.0".into());

        Ok(())
    }

    /// Applies `manifest`'s configured auth strategy (header, query
    /// parameter, path parameter, HTTP basic, OAuth bearer, or
    /// multi-header) using `creds`, inserting the resulting values into the
    /// matching param map. The unset auth type is a deliberate no-op;
    /// any other value that doesn't map to a known auth type errors
    /// instead of silently skipping authentication.
    fn handle_auth(
        &mut self,
        manifest: &SwaggerService,
        creds: Option<&Authentication>,
    ) -> error::Result<()> {
        let defined_auth = &manifest.auth;
        let auth_type = defined_auth.type_.enum_value().map_err(|raw| {
            error::APICaller::Unimplemented(format!("Unrecognized auth type: {raw}"))
        })?;
        match auth_type {
            core_entities::service::swagger_service::service_auth::Type::HEADER => {
                let key = defined_auth
                    .params
                    .get("header")
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("header".into()))?
                    .string();

                let value = &creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .header()
                    .value;
                self.header_params
                    .insert(key.into(), serde_json::Value::String(value.clone()));
            }
            core_entities::service::swagger_service::service_auth::Type::PARAMETER => {
                let key = defined_auth
                    .params
                    .get("name")
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("name".into()))?
                    .string();

                let value = &creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .query()
                    .value;
                self.query_params
                    .insert(key.into(), serde_json::Value::String(value.clone()));
            }
            core_entities::service::swagger_service::service_auth::Type::PATH => {
                let key = defined_auth
                    .params
                    .get("path")
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("path".into()))?
                    .string();

                let value = &creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .path()
                    .value;
                self.path_params
                    .insert(key.into(), serde_json::Value::String(value.clone()));
            }
            core_entities::service::swagger_service::service_auth::Type::BASIC => {
                let value = creds.ok_or(error::APICaller::MissingCredentials)?.basic();
                let encoded_creds = base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{}", value.username, value.password));

                self.header_params.insert(
                    "Authorization".into(),
                    serde_json::Value::String(format!("Basic {encoded_creds}")),
                );
            }
            core_entities::service::swagger_service::service_auth::Type::OAUTH => {
                let header_name = defined_auth
                    .params
                    .get("header")
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("header".into()))?
                    .string();
                let token_type = defined_auth
                    .params
                    .get("type")
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("type".into()))?
                    .string();

                let value = creds.ok_or(error::APICaller::MissingCredentials)?.oauth();
                let access_token = value
                    .accessToken
                    .as_ref()
                    .ok_or(error::APICaller::MissingAccessToken)?;

                self.header_params.insert(
                    header_name.into(),
                    serde_json::Value::String(format!("{token_type} {access_token}")),
                );
            }
            core_entities::service::swagger_service::service_auth::Type::MULTIHEADER => {
                let headers = defined_auth
                    .params
                    .get("headers")
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("headers".into()))?
                    .multiHeaderAuth();

                let values = creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .multiHeader();
                let values = &values.values;

                for key in &headers.strings {
                    let value = values
                        .get(key)
                        .ok_or_else(|| error::APICaller::MissingRequiredParameter(key.clone()))?;

                    self.header_params
                        .insert(key.into(), serde_json::Value::String(value.clone()));
                }
            }
            core_entities::service::swagger_service::service_auth::Type::UNSET => {}
        }

        Ok(())
    }

    /// Prepares the next page's request: applies the configured pagination
    /// strategy's parameters (page number, offset, limit, or a cursor read
    /// from `previous_response`) as runtime expressions, and returns the
    /// page size that was requested (`0` if unpaginated).
    fn handle_pagination(
        &mut self,
        pagination_config: &Option<pagination::Value>,
        previous_response: Option<&serde_json::Value>,
        current_page: i32,
        parameters: &[Parameter],
    ) -> error::Result<i32> {
        let requested = if let Some(pagination) = pagination_config {
            match pagination {
                core_entities::service::pagination::Value::PageOffset(page_offset) => {
                    let current_page = page_offset
                        .startPage
                        .value
                        .checked_add(current_page)
                        .ok_or(error::APICaller::PagingOverflow)?;
                    let max_limit = page_offset.maxLimit.value;

                    self.apply_runtime_expression(
                        &page_offset.pageOffsetParam,
                        serde_json::Value::Number(current_page.into()),
                        parameters,
                    )?;
                    self.apply_runtime_expression(
                        &page_offset.limitParam,
                        serde_json::Value::Number(max_limit.into()),
                        parameters,
                    )?;

                    max_limit
                }
                core_entities::service::pagination::Value::MultiCursor(cursor) => {
                    let max_limit = cursor.maxLimit.value;
                    self.apply_runtime_expression(
                        &cursor.limitParam,
                        serde_json::Value::Number(max_limit.into()),
                        parameters,
                    )?;

                    if let Some(previous_response) = previous_response {
                        let cursor_path = cursor
                            .cursorsPath
                            .first()
                            .ok_or_else(|| error::APICaller::NotFound("Cursor Path".into()))?
                            .jmesPath();

                        let cursor_path = cursor_path
                            .strip_prefix(constants::RESPONSE_BODY_PREFIX)
                            .unwrap_or(cursor_path);

                        let cursor_path = cursor_path.parse::<jsonptr::Pointer>()?;
                        let next_cursor = cursor_path.resolve(previous_response)?;

                        let cursor_param = cursor
                            .cursorsParam
                            .first()
                            .ok_or_else(|| error::APICaller::NotFound("Cursor Param".into()))?;
                        self.apply_runtime_expression(
                            cursor_param,
                            next_cursor.clone(),
                            parameters,
                        )?;
                    }

                    max_limit
                }
                core_entities::service::pagination::Value::Offset(offset) => {
                    let max_limit = offset.maxLimit.value;

                    self.apply_runtime_expression(
                        &offset.offsetParam,
                        serde_json::Value::Number(current_page.into()),
                        parameters,
                    )?;
                    self.apply_runtime_expression(
                        &offset.limitParam,
                        serde_json::Value::Number(max_limit.into()),
                        parameters,
                    )?;

                    max_limit
                }
                pagination::Value::NextUrl(_) | pagination::Value::Unpaginated(_) | _ => 0_i32,
            }
        } else {
            0_i32
        };

        Ok(requested)
    }

    /// Applies `value` at the location named by a runtime `expression`
    /// (`$request.query.*`, `$request.path.*`, `$request.header.*`, or
    /// `$request.body#{pointer}`), or — for a plain parameter name rather
    /// than a `$request.` expression — resolves it against `parameter`'s
    /// manifest definitions via [`collect_params`](APICallState::collect_params).
    fn apply_runtime_expression(
        &mut self,
        expression: &str,
        value: serde_json::Value,
        parameter: &[Parameter],
    ) -> error::Result<()> {
        // we can only apply to the request
        if expression.starts_with("$request.") {
            let expression = expression
                .strip_prefix("$request.")
                .ok_or_else(|| error::APICaller::InvalidRuntimeExpression(expression.into()))?;

            if expression.starts_with("query.") {
                let key = expression
                    .strip_prefix("query.")
                    .ok_or_else(|| error::APICaller::InvalidRuntimeExpression(expression.into()))?;
                self.query_params.insert(key.to_owned(), value);
            } else if expression.starts_with("path.") {
                let key = expression
                    .strip_prefix("path.")
                    .ok_or_else(|| error::APICaller::InvalidRuntimeExpression(expression.into()))?;
                self.path_params.insert(key.to_owned(), value);
            } else if expression.starts_with("header.") {
                let key = expression
                    .strip_prefix("header.")
                    .ok_or_else(|| error::APICaller::InvalidRuntimeExpression(expression.into()))?;
                self.header_params.insert(key.to_owned(), value);
            } else if expression.starts_with("body#") && self.body.is_some() {
                let path = expression
                    .strip_prefix("body#")
                    .ok_or_else(|| error::APICaller::InvalidRuntimeExpression(expression.into()))?
                    .parse::<jsonptr::Pointer>()?;

                let body = self
                    .body
                    .as_mut()
                    .ok_or_else(|| error::APICaller::InvalidRuntimeExpression(expression.into()))?;

                path.assign(body, value)?;
            } else {
                return Err(error::APICaller::InvalidRuntimeExpression(
                    expression.into(),
                ));
            }
        } else {
            let mut params = serde_json::Map::new();
            params.insert(expression.to_owned(), value);
            let params = serde_json::Value::Object(params);
            self.collect_params(&params, parameter, false)?;
        }

        Ok(())
    }
}

/// A [`DataConnectionRunner`] that executes an operation as one or more
/// HTTP requests, reusing a single [`reqwest::blocking::Client`] (and its
/// connection pool) across every call it makes.
pub struct APICaller {
    /// Where each request/response is logged.
    log: LogWriter,

    /// The shared HTTP client every request is sent through.
    client: reqwest::blocking::Client,
}

impl APICaller {
    /// Creates an [`APICaller`] that logs to `log`, building its
    /// [`reqwest::blocking::Client`] once up front.
    #[must_use]
    #[inline]
    pub fn new(log: LogWriter) -> Self {
        Self {
            log,
            client: reqwest::blocking::Client::new(),
        }
    }

    /// Builds and sends `operation_name`'s request(s) against `bundle`,
    /// paging through results (per the operation's pagination strategy)
    /// until a page comes back short, the configured `limit` is reached, or
    /// pagination isn't configured — in which case exactly one request is
    /// sent. Returns the raw first response unchanged if `ctx.raw_response`
    /// is set; otherwise returns the accumulated, limit-truncated results
    /// as a JSON array.
    fn run_internal(
        &self,
        name: &str,
        operation_name: &str,
        bundle: &DataConnectorBundle,
        params: &serde_json::Value,
        options: &serde_json::Value,
        ctx: &EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        let operation = bundle
            .api
            .operations
            .get(operation_name)
            .ok_or_else(|| error::APICaller::OperationNotFound(operation_name.into()))?;

        let total_limit = options.get("limit");

        let total_limit: i32 = total_limit
            .and_then(|value| match value {
                serde_json::Value::Number(n) if n.is_f64() => n.as_f64().map(|n| n as i32),
                serde_json::Value::Number(n) if n.is_i64() => n.as_i64().map(|n| n as i32),
                serde_json::Value::Number(n) if n.is_u64() => n.as_u64().map(|n| n as i32),
                serde_json::Value::Null
                | serde_json::Value::Bool(_)
                | serde_json::Value::Number(_)
                | serde_json::Value::String(_)
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => None,
            })
            .unwrap_or(constants::DEFAULT_LIMIT);

        let mut total: i32 = 0;
        let mut current_page: i32 = 0;

        let mut page_responses: Vec<serde_json::Value> = Vec::new();

        loop {
            // Create a request payload
            let mut call_state = APICallState::default();
            call_state.set_body(params.get("$body").cloned());
            call_state.collect_params(params, &operation.parameter, true)?;
            call_state.handle_auth(bundle.manifest, bundle.creds)?;
            call_state.set_method(operation)?;
            call_state.set_endpoint(bundle.api.basePath(), &operation.path);

            let request_size = call_state.handle_pagination(
                &operation.pagination.value,
                page_responses.last(),
                current_page,
                &operation.parameter,
            )?;

            // Send the request
            let result = call_state.send(
                format!("{name}.{operation_name}").as_str(),
                &self.client,
                &self.log,
            )?;

            // Unless the provided context told us to paginate,
            // we're going to bail early and just return the first raw response
            if ctx.raw_response {
                return Ok(result);
            }

            // Peek at what the results path is
            let actual_result = find_results(&result, &operation.pagination.value)?;

            // Determine how many items we got in a request
            let current_size = if let serde_json::Value::Array(arr) = actual_result {
                i32::try_from(arr.len())?
            } else {
                1_i32
            };

            // Push the raw response onto the vector for us to reference in the next iteration
            page_responses.push(result);

            current_page = current_page
                .checked_add(1)
                .ok_or(error::APICaller::PagingOverflow)?;
            total = total
                .checked_add(current_size)
                .ok_or(error::APICaller::PagingOverflow)?;

            // Figure out if we're done or not
            if request_size == 0_i32
                || total_limit == 0_i32
                || current_size < request_size
                || total >= total_limit
            {
                break;
            }
        }

        let result: error::Result<Vec<serde_json::Value>> = page_responses
            .into_iter()
            .map(|response| {
                let result = find_results(&response, &operation.pagination.value)?.clone();
                Ok(result)
            })
            .collect();

        let result: Vec<serde_json::Value> = result?
            .into_iter()
            .flat_map(|response| {
                if let serde_json::Value::Array(arr) = response {
                    arr
                } else {
                    vec![response]
                }
            })
            .collect();

        let total_limit: usize = total_limit.try_into()?;
        let result = if total_limit > 0 {
            result.get(..total_limit).unwrap_or(&result).to_vec()
        } else {
            result
        };

        Ok(serde_json::Value::Array(result))
    }
}

impl DataConnectionRunner for APICaller {
    #[inline]
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        bundle: &DataConnectorBundle,
        params: serde_json::Value,
        options: serde_json::Value,
        ctx: &EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        let result = self.run_internal(name, operation_name, bundle, &params, &options, ctx)?;
        Ok(result)
    }
}

/// The async sibling of [`APICaller`] - same manifest-driven request
/// building, auth, and pagination logic (all shared via [`APICallState`]),
/// just executed over a pooled [`reqwest::Client`] instead of
/// [`reqwest::blocking::Client`], so a call never occupies a thread while
/// waiting on the network. See [`AsyncDataConnectionRunner`]'s docs for why
/// this exists as a genuinely separate dispatch path rather than being
/// reached through [`Engine::run`](execution_engine::Engine::run).
pub struct AsyncAPICaller {
    /// Where each request/response is logged.
    log: LogWriter,

    /// The shared async HTTP client every request is sent through.
    client: reqwest::Client,
}

impl AsyncAPICaller {
    /// Creates an [`AsyncAPICaller`] that logs to `log`, building its
    /// [`reqwest::Client`] once up front.
    #[must_use]
    #[inline]
    pub fn new(log: LogWriter) -> Self {
        Self {
            log,
            client: reqwest::Client::new(),
        }
    }

    /// The async sibling of [`APICaller::run_internal`] - identical
    /// pagination-loop shape, just `.await`s [`APICallState::send_async`]
    /// instead of calling the blocking [`APICallState::send`].
    async fn run_internal(
        &self,
        name: &str,
        operation_name: &str,
        bundle: &DataConnectorBundle<'_>,
        params: &serde_json::Value,
        options: &serde_json::Value,
        ctx: &EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        let operation = bundle
            .api
            .operations
            .get(operation_name)
            .ok_or_else(|| error::APICaller::OperationNotFound(operation_name.into()))?;

        let total_limit = options.get("limit");

        let total_limit: i32 = total_limit
            .and_then(|value| match value {
                serde_json::Value::Number(n) if n.is_f64() => n.as_f64().map(|n| n as i32),
                serde_json::Value::Number(n) if n.is_i64() => n.as_i64().map(|n| n as i32),
                serde_json::Value::Number(n) if n.is_u64() => n.as_u64().map(|n| n as i32),
                serde_json::Value::Null
                | serde_json::Value::Bool(_)
                | serde_json::Value::Number(_)
                | serde_json::Value::String(_)
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => None,
            })
            .unwrap_or(constants::DEFAULT_LIMIT);

        let mut total: i32 = 0;
        let mut current_page: i32 = 0;

        let mut page_responses: Vec<serde_json::Value> = Vec::new();

        loop {
            // Create a request payload
            let mut call_state = APICallState::default();
            call_state.set_body(params.get("$body").cloned());
            call_state.collect_params(params, &operation.parameter, true)?;
            call_state.handle_auth(bundle.manifest, bundle.creds)?;
            call_state.set_method(operation)?;
            call_state.set_endpoint(bundle.api.basePath(), &operation.path);

            let request_size = call_state.handle_pagination(
                &operation.pagination.value,
                page_responses.last(),
                current_page,
                &operation.parameter,
            )?;

            // Send the request
            let result = call_state
                .send_async(
                    format!("{name}.{operation_name}").as_str(),
                    &self.client,
                    &self.log,
                )
                .await?;

            // Unless the provided context told us to paginate,
            // we're going to bail early and just return the first raw response
            if ctx.raw_response {
                return Ok(result);
            }

            // Peek at what the results path is
            let actual_result = find_results(&result, &operation.pagination.value)?;

            // Determine how many items we got in a request
            let current_size = if let serde_json::Value::Array(arr) = actual_result {
                i32::try_from(arr.len())?
            } else {
                1_i32
            };

            // Push the raw response onto the vector for us to reference in the next iteration
            page_responses.push(result);

            current_page = current_page
                .checked_add(1)
                .ok_or(error::APICaller::PagingOverflow)?;
            total = total
                .checked_add(current_size)
                .ok_or(error::APICaller::PagingOverflow)?;

            // Figure out if we're done or not
            if request_size == 0_i32
                || total_limit == 0_i32
                || current_size < request_size
                || total >= total_limit
            {
                break;
            }
        }

        let result: error::Result<Vec<serde_json::Value>> = page_responses
            .into_iter()
            .map(|response| {
                let result = find_results(&response, &operation.pagination.value)?.clone();
                Ok(result)
            })
            .collect();

        let result: Vec<serde_json::Value> = result?
            .into_iter()
            .flat_map(|response| {
                if let serde_json::Value::Array(arr) = response {
                    arr
                } else {
                    vec![response]
                }
            })
            .collect();

        let total_limit: usize = total_limit.try_into()?;
        let result = if total_limit > 0 {
            result.get(..total_limit).unwrap_or(&result).to_vec()
        } else {
            result
        };

        Ok(serde_json::Value::Array(result))
    }
}

#[async_trait::async_trait]
impl AsyncDataConnectionRunner for AsyncAPICaller {
    #[inline]
    async fn run(
        &self,
        name: &str,
        operation_name: &str,
        bundle: &DataConnectorBundle,
        params: serde_json::Value,
        options: serde_json::Value,
        ctx: &EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        let result = self
            .run_internal(name, operation_name, bundle, &params, &options, ctx)
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        thread,
    };

    use core_entities::service::{swagger_service::ServiceAuth, Operation, SwaggerService};
    use protobuf::{EnumOrUnknown, MessageField};

    use super::*;

    // Minimal HTTP/1.1 keep-alive test server: accepts connections, serves
    // as many requests as arrive on each one, and counts distinct accepted
    // connections so the test can prove connection reuse.
    fn start_test_server() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut writer = stream;
                    loop {
                        let mut request_line = String::new();
                        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                            break;
                        }
                        loop {
                            let mut header_line = String::new();
                            let n = reader.read_line(&mut header_line).unwrap_or(0);
                            if n == 0 || header_line == "\r\n" {
                                break;
                            }
                        }
                        let body = b"{}";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                            body.len()
                        );
                        if writer.write_all(response.as_bytes()).is_err()
                            || writer.write_all(body).is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        (format!("http://{addr}"), accepted)
    }

    #[test]
    fn reuses_the_same_http_client_across_calls() {
        let (base_url, accepted) = start_test_server();
        let (log, _log_handle) = LogWriter::spawn(tempfile::tempfile().unwrap());
        let caller = APICaller::new(log);

        // The response body must be fully drained before the connection is
        // returned to reqwest's pool — leaving it unread races the second
        // request against that release and flakes the assertion below.
        caller
            .client
            .get(format!("{base_url}/ping"))
            .send()
            .unwrap()
            .bytes()
            .unwrap();
        caller
            .client
            .get(format!("{base_url}/ping"))
            .send()
            .unwrap()
            .bytes()
            .unwrap();

        assert_eq!(
            accepted.load(Ordering::SeqCst),
            1,
            "expected the second request to reuse the pooled connection from the first"
        );
    }

    #[tokio::test]
    async fn async_api_caller_reuses_the_same_http_client_across_calls() {
        let (base_url, accepted) = start_test_server();
        let (log, _log_handle) = LogWriter::spawn(tempfile::tempfile().unwrap());
        let caller = AsyncAPICaller::new(log);

        // See the sync test above: draining the body is what guarantees the
        // connection is idle-pooled before the next request is sent.
        caller
            .client
            .get(format!("{base_url}/ping"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        caller
            .client
            .get(format!("{base_url}/ping"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();

        assert_eq!(
            accepted.load(Ordering::SeqCst),
            1,
            "expected the second request to reuse the pooled connection from the first"
        );
    }

    #[tokio::test]
    async fn async_api_caller_run_dispatches_a_real_http_request() {
        let (base_url, _accepted) = start_test_server();
        let (log, _log_handle) = LogWriter::spawn(tempfile::tempfile().unwrap());
        let caller = AsyncAPICaller::new(log);

        let mut operation = Operation::new();
        operation.path = "/ping".to_owned();
        operation.method =
            EnumOrUnknown::new(core_entities::service::operation::HttpMethodType::GET);

        let mut api = core_entities::service::CommonApi::new();
        api.set_basePath(base_url);
        api.operations.insert("execute".to_owned(), operation);

        let manifest = SwaggerService::default();
        let bundle = DataConnectorBundle::new(&manifest, &api, None);
        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        let result = caller
            .run(
                "svc",
                "execute",
                &bundle,
                serde_json::Value::Null,
                serde_json::Value::Null,
                &ctx,
            )
            .await
            .expect("async api call should succeed");

        assert_eq!(result, serde_json::json!([{}]));
    }

    #[test]
    fn set_method_does_not_panic_on_an_unrecognized_method() {
        let operation = Operation {
            method: EnumOrUnknown::from_i32(999),
            ..Default::default()
        };

        let mut call_state = APICallState::default();
        let result = call_state.set_method(&operation);

        assert!(
            matches!(result, Err(error::APICaller::InvalidMethod(_))),
            "expected InvalidMethod, got {result:?}"
        );
    }

    #[test]
    fn collect_params_does_not_panic_on_an_unrecognized_parameter_location() {
        let defined_param = Parameter {
            name: "x".to_owned(),
            in_: EnumOrUnknown::from_i32(999),
            ..Default::default()
        };
        let params = serde_json::json!({ "x": "value" });

        let mut call_state = APICallState::default();
        let result = call_state.collect_params(&params, &[defined_param], true);

        assert!(
            matches!(result, Err(error::APICaller::Unimplemented(_))),
            "expected Unimplemented, got {result:?}"
        );
    }

    #[test]
    fn handle_auth_does_not_panic_on_an_unrecognized_auth_type() {
        let manifest = SwaggerService {
            auth: MessageField::some(ServiceAuth {
                type_: EnumOrUnknown::from_i32(999),
                ..Default::default()
            }),
            ..Default::default()
        };

        let mut call_state = APICallState::default();
        let result = call_state.handle_auth(&manifest, None);

        assert!(
            result.is_err(),
            "expected an unrecognized auth type to error instead of silently skipping auth, got {result:?}"
        );
    }
}
