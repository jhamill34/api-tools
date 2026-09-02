//! A [`FilteredRunner`] adapter that calls another already-registered
//! operation through an [`EngineService`] and narrows its result down to
//! a specific set of selected output fields.

pub mod error;

use std::{
    rc::Rc,
    sync::{Arc, LazyLock},
};

use common_data_structures::log_writer::LogWriter;

use core_entities::ports::engine::{self, EngineInputContext, EngineService, FilteredRunner};
use core_json_compat::{from_json, to_json};
use regex::Regex;

static OPERATION_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new("(?P<group>.*)/(?P<app>.*):(?P<version>.*)").ok());

/// Resolves an [`APIWrappedService`](core_entities::service::APIWrappedService)
/// manifest by invoking the wrapped operation on the shared
/// [`EngineService`] and picking out the manifest's selected
/// output fields.
pub struct APIWrapper {
    /// Currently unused; kept for parity with the other runners' constructors.
    _log: LogWriter,

    /// The engine used to invoke the wrapped operation.
    engine: Arc<dyn EngineService>,
}

impl APIWrapper {
    /// Creates an [`APIWrapper`] that dispatches wrapped calls through
    /// `engine`.
    #[must_use]
    #[inline]
    pub fn new(log: LogWriter, engine: Arc<dyn EngineService>) -> Self {
        Self { _log: log, engine }
    }

    /// Builds the wrapped operation's input from `manifest.inputs` and
    /// `params`, runs it through [`engine`](APIWrapper::new), then extracts
    /// `manifest.output_selectors` (`JMESPath` expressions) from the result
    /// into the returned object.
    #[inline]
    fn run_internal(
        &self,
        name: &str,
        _operation_name: &str,
        manifest: &core_entities::service::APIWrappedService,
        params: &serde_json::Value,
        ctx: &EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        let app = extract_connector_id(&manifest.connector_id)?;
        let operation = manifest.connector_operation.as_str();

        let id = format!("{app}.{operation}");

        let mut input = serde_json::Value::Object(serde_json::Map::new());
        for input_param in &manifest.inputs {
            if let Some(param) = &input_param.param {
                let param = &param.name;
                if let Some(param) = params.get(param) {
                    let path: Vec<_> = input_param.api_param_name.split('.').collect();
                    traverse_map(&mut input, &path, param.clone())?;
                }
            }
        }

        let context =
            EngineInputContext::new(Some(name.to_owned()), ctx.execution_id.clone(), true);

        let result = self.engine.run(
            &id,
            from_json(input),
            from_json(serde_json::Value::Null),
            &context,
        )?;

        let result = Rc::new(to_json(result));

        let mut output = serde_json::Map::new();

        for output_param in &manifest.output_selectors {
            let expr = jmespath::compile(&output_param.jmes_path_selector)?;
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
        params: engine::RuntimeValue,
        ctx: &EngineInputContext,
    ) -> engine::error::Result<engine::RuntimeValue> {
        let params = to_json(params);
        let result = self.run_internal(name, operation_name, manifest, &params, ctx)?;

        Ok(from_json(result))
    }
}

/// Parses the `app` component out of a connector ID formatted as
/// `"group/app:version"`.
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

/// Writes `value` into `current` at the dot-separated path `parts`,
/// creating intermediate JSON objects along the way. Errors if an
/// intermediate path segment lands on a non-object value.
fn traverse_map(
    current: &mut serde_json::Value,
    parts: &[&str],
    value: serde_json::Value,
) -> error::Result<()> {
    if let Some(next) = parts.first() {
        if let serde_json::Value::Object(current) = current {
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
