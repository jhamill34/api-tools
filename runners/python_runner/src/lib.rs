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
)]

//!

mod bindings;
mod constants;
mod converters;
pub mod error;
mod pyconf;

extern crate alloc;
use alloc::sync::Arc;

use std::fs::File;
use std::sync::RwLock;

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

///
fn run_python<F>(f: F) -> PyResult<Value>
where
    F: FnOnce() -> PyResult<Value>,
{
    f()
}

///
pub struct PyActionRunner {
    ///
    engine: Arc<RwLock<execution_engine::Engine>>,

    ///
    loggers: Arc<RwLock<File>>,
}

impl PyActionRunner {
    ///
    #[inline]
    #[must_use]
    pub fn new(loggers: Arc<RwLock<File>>, engine: Arc<RwLock<execution_engine::Engine>>) -> Self {
        Self { engine, loggers }
    }

    ///
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

        let config = pyconf::default_python_config();
        let interp = pyembed::MainPythonInterpreter::new(config)
            .map_err(|_e| error::PyActionRunner::NotFound("Interpretter".into()))?;

        interp.with_gil(|py| -> error::Result<Value> {
            let output = PyDict::new(py);

            let api = bindings::APIBindingWraper {
                name: format!("{name}.{operation_name}"),
                engine: Arc::clone(&self.engine),
                ctx: execution_engine::services::EngineInputContext::new(
                    Some(name.to_owned()),
                    ctx.execution_id.clone(),
                    false,
                ),
                logger: Arc::clone(&self.loggers),
            };

            let workflow = bindings::Workflow {
                log: bindings::WorkflowLogger {
                    name: format!("{name}.{operation_name}"),
                    output: output.into(),
                    loggers: Arc::clone(&self.loggers),
                },
            };

            let action = bindings::Action {
                log: bindings::ActionLogger {
                    name: format!("{name}.{operation_name}"),
                    logger: Arc::clone(&self.loggers),
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
                logger: Arc::clone(&self.loggers),
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
            .map_err(|e| error::PyActionRunner::PythonError(e.to_string()))
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
