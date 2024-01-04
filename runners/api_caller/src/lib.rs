#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,

    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::match_ref_pats,
    clippy::separated_literal_suffix,

    clippy::as_conversions,
    clippy::cast_possible_truncation,

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines,
    clippy::needless_borrowed_reference,
    clippy::separated_literal_suffix,
    clippy::question_mark_used,
    clippy::absolute_paths,
    clippy::ref_patterns,
)]

//!

mod constants;
pub mod error;

extern crate alloc;
use alloc::sync::Arc;

use std::{collections::HashMap, fs::File, io::Write, sync::RwLock};

use base64::Engine as _;
use core_entities::service::{pagination, Operation, Parameter, SwaggerService};
use credential_entities::credentials::Authentication;
use execution_engine::services::{DataConnectionRunner, DataConnectorBundle, EngineInputContext};
use http::{HeaderMap, HeaderName, HeaderValue};

///
fn simplify_value(value: &serde_json::Value) -> error::Result<String> {
    match value {
        &serde_json::Value::String(ref val) => Ok(val.to_string()),
        &serde_json::Value::Bool(val) => Ok(val.to_string()),
        &serde_json::Value::Number(ref val) => Ok(val.to_string()),
        &serde_json::Value::Null => Ok("null".to_owned()),
        &serde_json::Value::Array(_) | &serde_json::Value::Object(_) => {
            Err(error::APICaller::SimpleValueAssertion)
        }
    }
}

///
fn simplify_value_map<'item, I>(values: I) -> error::Result<HashMap<String, String>>
where
    I: Iterator<Item = (&'item String, &'item serde_json::Value)>,
{
    values
        .map(|(key, value)| Ok((key.to_string(), simplify_value(value)?)))
        .collect()
}

///
fn find_results<'item>(
    result: &'item serde_json::Value,
    pagination_config: &Option<pagination::Value>,
) -> error::Result<&'item serde_json::Value> {
    let result = if let &Some(ref pagination) = pagination_config {
        match pagination {
            &core_entities::service::pagination::Value::PageOffset(ref page_offset) => {
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
            &core_entities::service::pagination::Value::MultiCursor(ref cursor) => {
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
            &core_entities::service::pagination::Value::Offset(ref offset) => {
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
            &core_entities::service::pagination::Value::Unpaginated(ref unpaginated) => {
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
            &core_entities::service::pagination::Value::NextUrl(_) | &_ => result,
        }
    } else {
        result
    };
    Ok(result)
}

///
#[derive(Default)]
struct APICallState {
    ///
    method: String,

    ///
    endpoint: String,

    ///
    header_params: HashMap<String, serde_json::Value>,

    ///
    query_params: HashMap<String, serde_json::Value>,

    ///
    path_params: HashMap<String, serde_json::Value>,

    ///
    body: Option<serde_json::Value>,
}

impl APICallState {
    ///
    fn send(
        &self,
        id: &str,
        client: &reqwest::blocking::Client,
        log: &Arc<RwLock<File>>,
    ) -> error::Result<serde_json::Value> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        let mut log = log
            .write()
            .map_err(|err| error::APICaller::PoisonedLock(err.to_string()))?;

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

        if let &Some(ref body) = &self.body {
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

    ///
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

    ///
    fn set_endpoint(&mut self, base_url: &str, path: &str) {
        let base_url = base_url.strip_suffix('/').unwrap_or(base_url);
        let path = path.strip_prefix('/').unwrap_or(path);

        self.endpoint = format!("{base_url}/{path}");
    }

    ///
    fn set_method(&mut self, operation: &Operation) -> error::Result<()> {
        match operation.method.unwrap() {
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

    ///
    fn set_body(&mut self, body: Option<serde_json::Value>) {
        if body.is_some() {
            self.header_params.insert(
                "Content-Type".to_owned(),
                serde_json::Value::String("application/json".into()),
            );
        }
        self.body = body;
    }

    ///
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
                match defined_param.in_.unwrap() {
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

    ///
    fn handle_auth(
        &mut self,
        manifest: &SwaggerService,
        creds: Option<&Authentication>,
    ) -> error::Result<()> {
        let defined_auth = &manifest.auth;
        match defined_auth.type_.unwrap() {
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

    ///
    fn handle_pagination(
        &mut self,
        pagination_config: &Option<pagination::Value>,
        previous_response: Option<&serde_json::Value>,
        current_page: i32,
        parameters: &[Parameter],
    ) -> error::Result<i32> {
        let requested = if let &Some(ref pagination) = pagination_config {
            match pagination {
                &core_entities::service::pagination::Value::PageOffset(ref page_offset) => {
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
                &core_entities::service::pagination::Value::MultiCursor(ref cursor) => {
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
                &core_entities::service::pagination::Value::Offset(ref offset) => {
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
                &pagination::Value::NextUrl(_) | &pagination::Value::Unpaginated(_) | &_ => 0_i32,
            }
        } else {
            0_i32
        };

        Ok(requested)
    }

    ///
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

///
pub struct APICaller {
    ///
    log: Arc<RwLock<File>>,
}

impl APICaller {
    ///
    #[must_use]
    #[inline]
    pub fn new(log: Arc<RwLock<File>>) -> Self {
        Self { log }
    }

    ///
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
                &serde_json::Value::Number(ref n) if n.is_f64() => n.as_f64().map(|n| n as i32),
                &serde_json::Value::Number(ref n) if n.is_i64() => n.as_i64().map(|n| n as i32),
                &serde_json::Value::Number(ref n) if n.is_u64() => n.as_u64().map(|n| n as i32),
                &serde_json::Value::Null
                | &serde_json::Value::Bool(_)
                | &serde_json::Value::Number(_)
                | &serde_json::Value::String(_)
                | &serde_json::Value::Array(_)
                | &serde_json::Value::Object(_) => None,
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
            let client = reqwest::blocking::Client::new();
            let result = call_state.send(
                format!("{name}.{operation_name}").as_str(),
                &client,
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
            let current_size = if let &serde_json::Value::Array(ref arr) = actual_result {
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
