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
    clippy::single_call_fn,
    clippy::absolute_paths,
    clippy::min_ident_chars
)]

//! A [`CodeRunner`] adapter that executes a Python operation body in an
//! embedded `CPython` interpreter, exposing `api`/`workflow`/`action`/`task`
//! bindings the script can use to interact with the engine.

mod bindings;
mod constants;
mod converters;
pub mod error;

extern crate alloc;
use alloc::sync::Arc;

use std::sync::RwLock;

use common_data_structures::log_writer::LogWriter;
use execution_engine::services::CodeRunner;
use lazy_static::lazy_static;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyModule, PyString};
use regex::Regex;
use serde_json::Value;

lazy_static! {
    static ref FUNCTION_REGEX: Option<Regex> =
        Regex::new(r"def\s*(?P<name>\w+)\s*\(\s*\w+\s*\)\s*:").ok();
}

/// Runs `func` inline. A thin seam separating the actual Python-calling
/// logic from its surrounding setup, kept as its own function for clarity.
fn run_python<F>(func: F) -> PyResult<Value>
where
    F: FnOnce() -> PyResult<Value>,
{
    func()
}

/// A [`CodeRunner`] that wraps and executes a Python operation body in an
/// embedded `CPython` interpreter, installing `api`/`workflow`/`action`/
/// `task` bindings into the script's module namespace so it can call back
/// into the shared [`execution_engine::Engine`].
pub struct PyActionRunner {
    /// The engine used to resolve calls made from the script's bindings.
    engine: Arc<RwLock<execution_engine::Engine>>,

    /// Where the script's bindings log activity.
    loggers: LogWriter,
}

impl PyActionRunner {
    /// Creates a [`PyActionRunner`] that dispatches binding calls through
    /// `engine` and logs them to `loggers`.
    #[inline]
    #[must_use]
    pub fn new(loggers: LogWriter, engine: Arc<RwLock<execution_engine::Engine>>) -> Self {
        Self { engine, loggers }
    }

    /// Detects `source_code`'s entry-point function name (falling back to
    /// [`constants::DEFAULT_FUNCTION_NAME`] if none is found), evaluates
    /// the script as a module with the `api`/`workflow`/`action`/`task`
    /// bindings installed, calls the entry point with `params`, and
    /// converts its return value — or, if the script called
    /// `workflow.log.done`/`workflow.log.fail`, that call's output — back
    /// to JSON.
    fn run_internal(
        &self,
        name: &str,
        operation_name: &str,
        source_code: &str,
        params: Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> error::Result<Value> {
        let function_name = FUNCTION_REGEX
            .as_ref()
            .ok_or_else(|| error::PyActionRunner::RegexError("Function Regex".into()))?
            .captures(source_code)
            .and_then(|cap| cap.name("name"))
            .map_or_else(
                || constants::DEFAULT_FUNCTION_NAME.to_owned(),
                |cap| cap.as_str().to_owned(),
            );

        pyo3::prepare_freethreaded_python();

        Python::with_gil(|py| -> error::Result<Value> {
            let output = PyDict::new(py);

            let api = bindings::APIBindingWraper {
                name: format!("{name}.{operation_name}"),
                engine: Arc::clone(&self.engine),
                ctx: execution_engine::services::EngineInputContext::new(
                    Some(name.to_owned()),
                    ctx.execution_id.clone(),
                    false,
                ),
                logger: self.loggers.clone(),
            };

            let workflow = bindings::Workflow {
                log: bindings::WorkflowLogger {
                    name: format!("{name}.{operation_name}"),
                    output: output.into(),
                    loggers: self.loggers.clone(),
                },
            };

            let action = bindings::Action {
                log: bindings::ActionLogger {
                    name: format!("{name}.{operation_name}"),
                    logger: self.loggers.clone(),
                },
            };

            let task = bindings::TaskBinding {
                name: format!("{name}.{operation_name}"),
                engine: Arc::clone(&self.engine),
                ctx: execution_engine::services::EngineInputContext::new(
                    Some(name.to_owned()),
                    ctx.execution_id.clone(),
                    true,
                ),
                logger: self.loggers.clone(),
            };

            run_python(|| {
                let input: Py<PyAny> = converters::from_value(py, params)?;

                let module = PyModule::from_code(py, source_code, name, operation_name)?;
                module.add(constants::BINDING_API_KEY, PyCell::new(py, api)?)?;
                module.add(constants::BINDING_WORKFLOW_KEY, PyCell::new(py, workflow)?)?;
                module.add(constants::BINDING_ACTION_KEY, PyCell::new(py, action)?)?;
                module.add(constants::BINDING_TASK_KEY, PyCell::new(py, task)?)?;

                let func_name = PyString::new(py, &function_name);
                let func = module.getattr(func_name)?;
                let returned = func.call1((input,))?;

                // If workflow.log.done or workflow.log.fail was called then return the custom outputs
                // otherwise return what the fuction returned.
                let result = if let Some(success) = output.get_item(constants::RESPONSE_SUCCESS_KEY)
                {
                    if let Ok(custom) = success.get_item(constants::RESPONSE_CUSTOM_KEY) {
                        converters::from_py(custom)
                    } else {
                        converters::from_py(success)
                    }
                } else if let Some(error) = output.get_item(constants::RESPONSE_ERROR_KEY) {
                    if let Ok(custom) = error.get_item(constants::RESPONSE_CUSTOM_KEY) {
                        converters::from_py(custom)
                    } else {
                        converters::from_py(error)
                    }
                } else {
                    converters::from_py(returned)
                };

                result
            })
            .map_err(|err| error::PyActionRunner::PythonError(err.to_string()))
        })
    }
}

impl CodeRunner for PyActionRunner {
    #[inline]
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        source_code: &str,
        params: Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> execution_engine::error::Result<Value> {
        let result = self.run_internal(name, operation_name, source_code, params, ctx)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyModule;
    use pyo3::Python;
    use serde_json::json;

    use crate::converters;

    // Exercises the same interpreter-init + module-from-source + call +
    // JSON round-trip pattern run_internal uses, without needing the full
    // execution_engine::Engine graph. Guards the pyembed -> plain pyo3
    // switch: this must keep working dynamically linked against whatever
    // libpython PYO3_PYTHON/LD_LIBRARY_PATH point at (see issue #35).
    #[test]
    fn runs_a_simple_python_function_and_round_trips_json() {
        pyo3::prepare_freethreaded_python();

        let result = Python::with_gil(|py| -> pyo3::PyResult<serde_json::Value> {
            let input = converters::from_value(py, json!({"name": "world"}))?;

            let module = PyModule::from_code(
                py,
                "def execute(input):\n    return {'greeting': 'hello ' + input['name']}\n",
                "test_module",
                "test_module",
            )?;

            let func = module.getattr("execute")?;
            let returned = func.call1((input,))?;

            converters::from_py(returned)
        })
        .expect("python execution should succeed");

        assert_eq!(result, json!({"greeting": "hello world"}));
    }
}
