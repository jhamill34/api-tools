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

    clippy::absolute_paths,
    clippy::single_call_fn,
    clippy::ref_patterns,
)]

//!

pub mod error;

extern crate alloc;
use alloc::{rc::Rc, sync::Arc};

use lazy_static::lazy_static;
use std::{fs::File, sync::RwLock};

use execution_engine::services::FilteredRunner;
use regex::Regex;

lazy_static! {
    static ref OPERATION_REGEX: Option<Regex> =
        Regex::new("(?P<group>.*)/(?P<app>.*):(?P<version>.*)").ok();
}

///
pub struct APIWrapper {
    ///
    _log: Arc<RwLock<File>>,

    ///
    engine: Arc<RwLock<execution_engine::Engine>>,
}

impl APIWrapper {
    ///
    #[must_use]
    #[inline]
    pub fn new(log: Arc<RwLock<File>>, engine: Arc<RwLock<execution_engine::Engine>>) -> Self {
        Self { _log: log, engine }
    }

    ///
    #[inline]
    fn run_internal(
        &self,
        name: &str,
        _operation_name: &str,
        manifest: &core_entities::service::APIWrappedService,
        params: &serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        let app = extract_connector_id(&manifest.connectorId)?;
        let operation = manifest.connectorOperation.as_str();

        let id = format!("{app}.{operation}");

        let mut input = serde_json::Value::Object(serde_json::Map::new());
        for input_param in &manifest.inputs {
            if let &Some(ref param) = &input_param.param.0 {
                let param = &param.name;
                if let Some(param) = params.get(param) {
                    let path: Vec<_> = input_param.apiParamName.split('.').collect();
                    traverse_map(&mut input, &path, param.clone())?;
                }
            }
        }

        let context = execution_engine::services::EngineInputContext::new(
            Some(name.to_owned()),
            ctx.execution_id.clone(),
            true,
        );

        let engine = self
            .engine
            .read()
            .map_err(|err| error::FilteredRunner::PoisonedLock(err.to_string()))?;
        let result = engine.run(&id, input, serde_json::Value::Null, &context)?;

        let result = Rc::new(result);

        let mut output = serde_json::Map::new();

        for output_param in &manifest.outputSelectors {
            let expr = jmespath::compile(&output_param.jmesPathSelector)?;
            let value = expr.search(Rc::clone(&result))?;
            let value = serde_json::to_string(&value)?;
            let value = serde_json::from_str(&value)?;
            output.insert(output_param.name.clone(), value);
        }

        Ok(output.into())
    }
}

impl FilteredRunner for APIWrapper {
    #[inline]
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        manifest: &core_entities::service::APIWrappedService,
        params: serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        let result = self.run_internal(name, operation_name, manifest, &params, ctx)?;

        Ok(result)
    }
}

///
fn extract_connector_id(id: &str) -> error::Result<&str> {
    let op_regex = OPERATION_REGEX
        .as_ref()
        .ok_or_else(|| error::FilteredRunner::UnknownConnectorId(id.to_owned()))?;
    let captures = op_regex
        .captures(id)
        .ok_or_else(|| error::FilteredRunner::UnknownConnectorId(id.to_owned()))?;

    let app = captures
        .name("app")
        .ok_or_else(|| error::FilteredRunner::UnknownConnectorId(id.to_owned()))?;
    let app = app.as_str();

    Ok(app)
}

///
fn traverse_map(
    current: &mut serde_json::Value,
    parts: &[&str],
    value: serde_json::Value,
) -> error::Result<()> {
    if let Some(next) = parts.first() {
        if let &mut serde_json::Value::Object(ref mut current) = current {
            let key = (*next).to_owned();
            let child = current
                .entry(key)
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

            let remainder = parts.get(1..).unwrap_or_default();

            traverse_map(child, remainder, value)
        } else {
            Err(error::FilteredRunner::PathTraversal(parts.join(".")))
        }
    } else {
        *current = value;
        Ok(())
    }
}
