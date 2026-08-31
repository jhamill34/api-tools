//! A [`DataConnectionRunner`] adapter that resolves an operation's request
//! (method, endpoint, params, auth) and executes it over HTTP, handling
//! pagination across multiple requests when configured.

mod constants;
pub mod error;

use std::collections::HashMap;

use base64::Engine as _;
use common_data_structures::log_writer::LogWriter;
use core_entities::service::{Operation, Pagination, Parameter, SwaggerService};
use core_entities::ports::engine::{
    self, AsyncDataConnectionRunner, DataConnectionRunner, DataConnectorBundle,
    EngineInputContext,
};
use credential_entities::credentials::Authentication;
use http::{HeaderMap, HeaderName, HeaderValue};

/// Converts a scalar JSON value to its string form, for use as a header,
/// query, or path parameter. Errors on an array or object, which have no
/// unambiguous scalar representation.
fn simplify_value(value: &serde_json::Value) -> error::Result<String> {
    match value {
        serde_json::Value::String(val) => Ok(val.clone()),
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
        .map(|(key, value)| Ok((key.clone(), simplify_value(value)?)))
        .collect()
}

/// Resolves the `options.limit` pagination cap from JSON to an `i32`,
/// falling back to [`constants::DEFAULT_LIMIT`] when absent, non-numeric, or
/// out of `i32`'s range (rather than silently wrapping to an unrelated
/// value).
fn resolve_total_limit(options: &serde_json::Value) -> i32 {
    options
        .get("limit")
        .and_then(|value| match value {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "float-to-int `as` casts saturate rather than wrap (defined \
                          behavior since Rust 1.45): an out-of-range or NaN limit clamps \
                          to i32::MAX/i32::MIN/0, and a fractional limit truncates toward \
                          zero — both are the intended behavior for a pagination limit"
            )]
            serde_json::Value::Number(n) if n.is_f64() => n.as_f64().map(|n| n as i32),
            serde_json::Value::Number(n) if n.is_i64() => {
                n.as_i64().and_then(|n| i32::try_from(n).ok())
            }
            serde_json::Value::Number(n) if n.is_u64() => {
                n.as_u64().and_then(|n| i32::try_from(n).ok())
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => None,
        })
        .unwrap_or(constants::DEFAULT_LIMIT)
}

/// Extracts the paginated results from a raw response, by resolving the
/// configured pagination strategy's `resultsPath` (stripped of its
/// `$response.body#` runtime-expression prefix) as a JSON pointer into
/// `result`. Falls back to the whole `result` when there's no pagination
/// configured, the path resolves to the document root, or the strategy is
/// next-URL-based (which has no separate results path to extract).
fn find_results<'item>(
    result: &'item serde_json::Value,
    pagination_config: Option<&Pagination>,
) -> error::Result<&'item serde_json::Value> {
    let results_path = match pagination_config {
        Some(Pagination::PageOffset(page_offset)) => {
            page_offset.results_path.as_ref().map(core_entities::service::pagination::ExtendedPath::jmes_path)
        }
        Some(Pagination::MultiCursor(cursor)) => {
            cursor.results_path.as_ref().map(core_entities::service::pagination::ExtendedPath::jmes_path)
        }
        Some(Pagination::Offset(offset)) => offset.results_path.as_ref().map(core_entities::service::pagination::ExtendedPath::jmes_path),
        Some(Pagination::Unpaginated(unpaginated)) => {
            unpaginated.results_path.as_ref().map(core_entities::service::pagination::ExtendedPath::jmes_path)
        }
        Some(Pagination::NextUrl(_)) | None => None,
    };

    let Some(path) = results_path else {
        return Ok(result);
    };

    let path = path
        .strip_prefix(constants::RESPONSE_BODY_PREFIX)
        .unwrap_or(path);

    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };

    if path == "/" {
        Ok(result)
    } else {
        let path = path.parse::<jsonptr::Pointer>()?;
        Ok(path.resolve(result)?)
    }
}

/// Looks up `key` in `defined_auth`'s configured params as a plain string,
/// erroring if it's absent - the shared body of every [`APICallState::handle_auth`]
/// branch that needs one static auth parameter (as opposed to
/// [`Authentication`] itself, which comes from `creds` instead).
fn required_auth_param<'auth>(
    defined_auth: &'auth core_entities::service::swagger_service::ServiceAuth,
    key: &str,
) -> error::Result<&'auth str> {
    Ok(defined_auth
        .params
        .get(key)
        .ok_or_else(|| error::APICaller::InvalidAuthParameter(key.into()))?
        .as_str())
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
        use core_entities::service::operation::HttpMethodType;

        self.method = match operation.method {
            HttpMethodType::Post => String::from("POST"),
            HttpMethodType::Get => String::from("GET"),
            HttpMethodType::Put => String::from("PUT"),
            HttpMethodType::Patch => String::from("PATCH"),
            HttpMethodType::Delete => String::from("DELETE"),
            HttpMethodType::Head => String::from("HEAD"),
            HttpMethodType::Options => String::from("OPTIONS"),
            HttpMethodType::Trace => String::from("TRACE"),
            HttpMethodType::None => return Err(error::APICaller::InvalidMethod("NONE".into())),
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
                use core_entities::service::parameter::InType;

                match defined_param.r#in {
                    InType::Query => {
                        self.query_params
                            .insert(defined_param.name.clone(), value.clone());
                    }
                    InType::Header => {
                        self.header_params
                            .insert(defined_param.name.clone(), value.clone());
                    }
                    InType::Path => {
                        self.path_params
                            .insert(defined_param.name.clone(), value.clone());
                    }
                    InType::None | InType::Cookie | InType::Headers => {
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
        use core_entities::service::swagger_service::service_auth::Type;

        let Some(defined_auth) = &manifest.auth else {
            return Ok(());
        };

        match defined_auth.r#type {
            Type::Header => {
                let key = required_auth_param(defined_auth, "header")?;
                let value = &creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .as_header()
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("header credentials".into()))?
                    .value;
                self.header_params
                    .insert(key.into(), serde_json::Value::String(value.clone()));
            }
            Type::Parameter => {
                let key = required_auth_param(defined_auth, "name")?;
                let value = &creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .as_query()
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("query credentials".into()))?
                    .value;
                self.query_params
                    .insert(key.into(), serde_json::Value::String(value.clone()));
            }
            Type::Path => {
                let key = required_auth_param(defined_auth, "path")?;
                let value = &creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .as_path()
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("path credentials".into()))?
                    .value;
                self.path_params
                    .insert(key.into(), serde_json::Value::String(value.clone()));
            }
            Type::Basic => {
                let value = creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .as_basic()
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("basic credentials".into()))?;
                let encoded_creds = base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{}", value.username, value.password));

                self.header_params.insert(
                    "Authorization".into(),
                    serde_json::Value::String(format!("Basic {encoded_creds}")),
                );
            }
            Type::Oauth => {
                let header_name = required_auth_param(defined_auth, "header")?;
                let token_type = required_auth_param(defined_auth, "type")?;

                let value = creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .as_oauth()
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("oauth credentials".into()))?;
                let access_token = value
                    .access_token
                    .as_ref()
                    .ok_or(error::APICaller::MissingAccessToken)?;

                self.header_params.insert(
                    header_name.into(),
                    serde_json::Value::String(format!("{token_type} {access_token}")),
                );
            }
            Type::MultiHeader => {
                let headers = defined_auth
                    .params
                    .get("headers")
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("headers".into()))?
                    .as_multi_header_auth()
                    .ok_or_else(|| error::APICaller::InvalidAuthParameter("headers".into()))?;

                let values = creds
                    .ok_or(error::APICaller::MissingCredentials)?
                    .as_multi_header()
                    .ok_or_else(|| {
                        error::APICaller::InvalidAuthParameter("multi-header credentials".into())
                    })?;
                let values = &values.values;

                for key in &headers.strings {
                    let value = values
                        .get(key)
                        .ok_or_else(|| error::APICaller::MissingRequiredParameter(key.clone()))?;

                    self.header_params
                        .insert(key.into(), serde_json::Value::String(value.clone()));
                }
            }
            Type::Unset => {}
        }

        Ok(())
    }

    /// Prepares the next page's request: applies the configured pagination
    /// strategy's parameters (page number, offset, limit, or a cursor read
    /// from `previous_response`) as runtime expressions, and returns the
    /// page size that was requested (`0` if unpaginated).
    fn handle_pagination(
        &mut self,
        pagination_config: Option<&Pagination>,
        previous_response: Option<&serde_json::Value>,
        current_page: i32,
        parameters: &[Parameter],
    ) -> error::Result<i32> {
        let requested = if let Some(pagination) = pagination_config {
            match pagination {
                Pagination::PageOffset(page_offset) => {
                    let current_page = page_offset
                        .start_page
                        .unwrap_or(0)
                        .checked_add(current_page)
                        .ok_or(error::APICaller::PagingOverflow)?;
                    let max_limit = page_offset.max_limit.unwrap_or(0);

                    self.apply_runtime_expression(
                        &page_offset.page_offset_param,
                        serde_json::Value::Number(current_page.into()),
                        parameters,
                    )?;
                    self.apply_runtime_expression(
                        &page_offset.limit_param,
                        serde_json::Value::Number(max_limit.into()),
                        parameters,
                    )?;

                    max_limit
                }
                Pagination::MultiCursor(cursor) => {
                    let max_limit = cursor.max_limit.unwrap_or(0);
                    self.apply_runtime_expression(
                        &cursor.limit_param,
                        serde_json::Value::Number(max_limit.into()),
                        parameters,
                    )?;

                    if let Some(previous_response) = previous_response {
                        let cursor_path = cursor
                            .cursors_path
                            .first()
                            .ok_or_else(|| error::APICaller::NotFound("Cursor Path".into()))?
                            .jmes_path();

                        let cursor_path = cursor_path
                            .strip_prefix(constants::RESPONSE_BODY_PREFIX)
                            .unwrap_or(cursor_path);

                        let cursor_path = cursor_path.parse::<jsonptr::Pointer>()?;
                        let next_cursor = cursor_path.resolve(previous_response)?;

                        let cursor_param = cursor
                            .cursors_param
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
                Pagination::Offset(offset) => {
                    let max_limit = offset.max_limit.unwrap_or(0);

                    self.apply_runtime_expression(
                        &offset.offset_param,
                        serde_json::Value::Number(current_page.into()),
                        parameters,
                    )?;
                    self.apply_runtime_expression(
                        &offset.limit_param,
                        serde_json::Value::Number(max_limit.into()),
                        parameters,
                    )?;

                    max_limit
                }
                Pagination::NextUrl(_) | Pagination::Unpaginated(_) => 0_i32,
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

        let total_limit: i32 = resolve_total_limit(options);

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
            call_state.set_endpoint(bundle.api.base_path.as_deref().unwrap_or(""), &operation.path);

            let request_size = call_state.handle_pagination(
                operation.pagination.as_ref(),
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
            let actual_result = find_results(&result, operation.pagination.as_ref())?;

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
                let result = find_results(&response, operation.pagination.as_ref())?.clone();
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
    ) -> engine::error::Result<serde_json::Value> {
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
/// reached through `EngineService::run`.
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

        let total_limit: i32 = resolve_total_limit(options);

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
            call_state.set_endpoint(bundle.api.base_path.as_deref().unwrap_or(""), &operation.path);

            let request_size = call_state.handle_pagination(
                operation.pagination.as_ref(),
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
            let actual_result = find_results(&result, operation.pagination.as_ref())?;

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
                let result = find_results(&response, operation.pagination.as_ref())?.clone();
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
    ) -> engine::error::Result<serde_json::Value> {
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

    use core_entities::service::{
        operation::HttpMethodType, parameter::InType, swagger_service::service_auth::Type,
        swagger_service::ServiceAuth, Operation, SwaggerService,
    };

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

        let operation = Operation {
            path: "/ping".to_owned(),
            method: HttpMethodType::Get,
            ..Default::default()
        };

        let mut api = core_entities::service::CommonApi {
            base_path: Some(base_url),
            ..Default::default()
        };
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
    fn set_method_does_not_panic_on_an_unset_method() {
        let operation = Operation {
            method: HttpMethodType::None,
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
    fn collect_params_does_not_panic_on_an_unset_parameter_location() {
        let defined_param = Parameter {
            name: "x".to_owned(),
            r#in: InType::None,
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
    fn handle_auth_does_not_panic_on_a_missing_auth_parameter() {
        let manifest = SwaggerService {
            auth: Some(ServiceAuth {
                r#type: Type::Header,
                ..Default::default()
            }),
            ..Default::default()
        };

        let mut call_state = APICallState::default();
        let result = call_state.handle_auth(&manifest, None);

        assert!(
            matches!(result, Err(error::APICaller::InvalidAuthParameter(_))),
            "expected a Header auth type with no configured \"header\" param to error instead \
             of silently skipping auth, got {result:?}"
        );
    }

    #[test]
    fn resolve_total_limit_passes_through_in_range_numbers() {
        assert_eq!(resolve_total_limit(&serde_json::json!({ "limit": 42 })), 42);
        assert_eq!(
            resolve_total_limit(&serde_json::json!({ "limit": 42.9 })),
            42
        );
        assert_eq!(
            resolve_total_limit(&serde_json::json!({ "limit": 1_000_000_000_u64 })),
            1_000_000_000
        );
    }

    #[test]
    fn resolve_total_limit_falls_back_to_default_when_absent_or_non_numeric() {
        assert_eq!(
            resolve_total_limit(&serde_json::json!({})),
            constants::DEFAULT_LIMIT
        );
        assert_eq!(
            resolve_total_limit(&serde_json::json!({ "limit": "not a number" })),
            constants::DEFAULT_LIMIT
        );
    }

    #[test]
    fn resolve_total_limit_falls_back_to_default_instead_of_wrapping_an_out_of_range_i64() {
        // i64::from(i32::MAX) + 1 wraps to i32::MIN under `as i32`, which
        // would corrupt the pagination limit into a large negative number
        // instead of safely falling back to the default.
        let oversized = i64::from(i32::MAX) + 1;
        assert_eq!(
            resolve_total_limit(&serde_json::json!({ "limit": oversized })),
            constants::DEFAULT_LIMIT
        );
    }

    #[test]
    fn resolve_total_limit_falls_back_to_default_instead_of_wrapping_an_out_of_range_u64() {
        // 2^32 + 1 wraps to 1 under `as i32`, which would be silently
        // misread as a valid (tiny) limit instead of falling back to the
        // configured default.
        let oversized = u64::from(u32::MAX) + 2;
        assert_eq!(
            resolve_total_limit(&serde_json::json!({ "limit": oversized })),
            constants::DEFAULT_LIMIT
        );
    }
}
