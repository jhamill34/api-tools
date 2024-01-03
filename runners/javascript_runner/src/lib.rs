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

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines,
    clippy::question_mark_used,
    clippy::needless_borrowed_reference,
    clippy::single_call_fn,
    clippy::absolute_paths,
)]

//!

// pub mod bindings;
mod constants;
mod converters;
pub mod error;

extern crate alloc;
use alloc::sync::Arc;
use mini_v8::MiniV8;

use std::{fs::File, io::Write, sync::RwLock};

use execution_engine::services::CodeRunner;

use lazy_static::lazy_static;
use regex::{Captures, Regex};

lazy_static! {
    static ref ARROW_FUNC: Option<Regex> = Regex::new(r"(?P<line>\(\s*\w*\s*\)\s*=>\s*)").ok();
    static ref REGULAR_FUNC: Option<Regex> =
        Regex::new(r"function\s*(?P<name>\w+)\s*\(\s*\w*\s*\)\s*").ok();
}

///
fn handle_arrow_func(source: &str, re: &Regex) -> String {
    let source = re.replace(source, "const __internal_arrow = $line");

    format!("(input, api) => {{\n\n{source}\n\n; return __internal_arrow(input);\n\n}}\n\n")
}

///
fn handle_regular_func(source: &str, captures: &Captures) -> error::Result<String> {
    if let Some(name) = captures.name("name") {
        let name = name.as_str();
        Ok(format!(
            "(input, api) => {{\n\n{source}\n\n; return {name}(input);\n\n}}\n\n"
        ))
    } else {
        Err(error::JsActionRunner::NoFunctionFound(
            "Arrow Function".into(),
        ))
    }
}

///
fn wrap_source_code(source: &str) -> error::Result<String> {
    if let Some(arrow_func) = ARROW_FUNC.as_ref() {
        if arrow_func.is_match(source) {
            return Ok(handle_arrow_func(source, arrow_func));
        }
    }

    if let Some(regular_func) = REGULAR_FUNC.as_ref() {
        if let Some(captures) = regular_func.captures(source) {
            return handle_regular_func(source, &captures);
        }
    }

    Err(error::JsActionRunner::NoFunctionFound(
        "No Regular or Arrow Function Found".into(),
    ))
}

///
pub struct JsActionRunner {
    ///
    logger: Arc<RwLock<File>>,

    ///
    engine: Arc<RwLock<execution_engine::Engine>>,
}

impl JsActionRunner {
    ///
    #[inline]
    pub fn new(engine: Arc<RwLock<execution_engine::Engine>>, logger: Arc<RwLock<File>>) -> Self {
        Self { logger, engine }
    }

    ///
    fn run_internal(
        &self,
        name: &str,
        _operation_name: &str,
        source_code: &str,
        params: serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        let mv8 = MiniV8::new();

        let logger = Arc::clone(&self.logger);
        let engine = Arc::clone(&self.engine);
        let name = name.to_owned();
        let execution_id = ctx.execution_id.clone();
        let api_binding = mv8.create_function(move |inv| -> mini_v8::Result<mini_v8::Value> {
            let (id, params, options): (String, mini_v8::Value, Option<mini_v8::Value>) =
                inv.args.into(&inv.mv8)?;

            let now = chrono::offset::Local::now();
            let now = now.format(constants::DATETIME_FORMAT).to_string();

            {
                let mut logger = logger.write().map_err(|err| {
                    mini_v8::Error::ExternalError(Box::new(error::JsActionRunner::PoisonedLock(
                        err.to_string(),
                    )))
                })?;

                logger
                    .write_all(format!("{now} ({}) [API] {id}\n", name.clone()).as_bytes())
                    .map_err(|err| mini_v8::Error::ExternalError(Box::new(err)))?;
            };

            let params = converters::from_v8(params)?;
            let options = if let Some(options) = options {
                converters::from_v8(options)?
            } else {
                serde_json::Value::Null
            };

            let engine = engine.read().map_err(|err| {
                mini_v8::Error::ExternalError(Box::new(error::JsActionRunner::PoisonedLock(
                    err.to_string(),
                )))
            })?;
            let context = execution_engine::services::EngineInputContext::new(
                Some(name.clone()),
                execution_id.clone(),
                false,
            );
            let result = engine
                .run(&id, params, options, &context)
                .map_err(|err| mini_v8::Error::ExternalError(Box::new(err)))?;

            let output = converters::from_value(&inv.mv8, result)?;

            Ok(output)
        });
        let api = mv8.create_object();
        api.set("run", api_binding)
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let source_code = wrap_source_code(source_code)?;

        let execute_internal: mini_v8::Function = mv8
            .eval(source_code)
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let inputs = converters::from_value(&mv8, params)
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let output = execute_internal
            .call((inputs, api))
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let result =
            converters::from_v8(output).map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        Ok(result)
    }
}

impl CodeRunner for JsActionRunner {
    #[inline]
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        source_code: &str,
        params: serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        let result = self.run_internal(name, operation_name, source_code, params, ctx)?;
        Ok(result)
    }
}
