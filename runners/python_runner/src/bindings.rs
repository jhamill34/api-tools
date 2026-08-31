//! `pyo3` classes installed into a running script's module namespace as the
//! `api`/`workflow`/`action`/`task` bindings, giving the script a way to
//! call back into the engine and to log activity.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use common_data_structures::log_writer::LogWriter;

use super::{constants, converters};
use pyo3::exceptions::{PyArithmeticError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use serde_json::Value;

/// The `task` binding: a factory a script uses to create [`Task`] handles.
#[pyclass]
pub struct TaskBinding {
    /// The `{service}.{operation}` identifier of the running script.
    pub name: String,

    /// The engine used to resolve calls made from created tasks.
    pub engine: Arc<dyn execution_engine::EngineService>,

    /// The execution context created tasks run under.
    pub ctx: execution_engine::services::EngineInputContext,

    /// Where created tasks log activity.
    pub logger: LogWriter,
}

#[pymethods]
impl TaskBinding {
    /// Creates a [`Task`] bound to operation `id` and `params`, sharing
    /// this binding's engine, logger, and parent context.
    pub fn create(&self, id: String, params: &PyAny) -> Task {
        Task {
            id,
            params: params.into(),
            name: self.name.clone(),
            engine: Arc::clone(&self.engine),
            ctx: execution_engine::services::EngineInputContext::new(
                self.ctx.parent.clone(),
                self.ctx.execution_id.clone(),
                false,
            ),
            logger: self.logger.clone(),
        }
    }
}

/// A pending call to operation `id` with `params`, created via
/// [`TaskBinding::create`], that a script resumes later — after a delay or
/// after collecting user input.
#[pyclass]
pub struct Task {
    /// The `{service}.{operation}` identifier of the script that created
    /// this task.
    pub name: String,

    /// The engine used to run this task's operation.
    pub engine: Arc<dyn execution_engine::EngineService>,

    /// The execution context this task runs under.
    pub ctx: execution_engine::services::EngineInputContext,

    /// Where this task logs activity.
    pub logger: LogWriter,

    /// The operation ID to invoke.
    pub id: String,

    /// The parameters to invoke it with.
    pub params: Py<PyAny>,
}

#[pymethods]
impl Task {
    /// Sleeps for `delay` `unit`s (`MINUTE`/`SECOND`/`MILLISECOND`/
    /// `NANOSECOND`; any other value is a no-op), then runs this task's
    /// operation and returns its result.
    #[pyo3(name = "continueAfter")]
    pub fn continue_after(&self, py: Python<'_>, delay: u64, unit: &str) -> PyResult<Py<PyAny>> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        self.logger
            .write_all(format!("{now} ({}) [TASK|WAIT] {}\n", self.name, self.id).as_bytes())?;

        match unit {
            "MINUTE" => {
                let minute_delay = delay.checked_mul(60).ok_or_else(|| {
                    PyArithmeticError::new_err("Overflow occurred calculating delay")
                })?;
                thread::sleep(Duration::from_secs(minute_delay));
            }
            "SECOND" => {
                thread::sleep(Duration::from_secs(delay));
            }
            "MILLISECOND" => {
                thread::sleep(Duration::from_millis(delay));
            }
            "NANOSECOND" => {
                thread::sleep(Duration::from_nanos(delay));
            }
            _ => {}
        }

        let params = converters::from_py(self.params.as_ref(py))?;

        let options = Value::Null;

        let result = self
            .engine
            .run(&self.id, params, options, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Making API Call: {e}")))?;

        converters::from_value(py, result)
    }

    /// Runs the built-in `$input` operation with `blocks` to collect user
    /// input, merges the result into this task's params under
    /// `input_results`, then runs this task's operation with the merged
    /// params and returns its result. Errors if this task's params aren't
    /// a JSON object, since there's nowhere to merge the input into.
    #[pyo3(name = "continueAfterUserInput")]
    pub fn continue_after_user_input(&self, py: Python<'_>, blocks: &PyAny) -> PyResult<Py<PyAny>> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        self.logger
            .write_all(format!("{now} ({}) [TASK|INPUT] {}\n", self.name, self.id).as_bytes())?;

        let blocks = converters::from_py(blocks)?;

        let result = self
            .engine
            .run("$input", blocks, Value::Null, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Collecting Input: {e}")))?;

        let mut params = converters::from_py(self.params.as_ref(py))?;

        if let Value::Object(map) = &mut params {
            map.insert("input_results".into(), result);
        } else {
            // TODO: Verify this functionality
            return Err(PyValueError::new_err("Expected parameters to be an Object"));
        }

        let options = Value::Null;

        let result = self
            .engine
            .run(&self.id, params, options, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Making API Call: {e}")))?;

        converters::from_value(py, result)
    }
}

/// The `api` binding: lets a script invoke another already-registered
/// operation synchronously and get its result back.
#[pyclass]
pub struct APIBindingWraper {
    /// The `{service}.{operation}` identifier of the running script.
    pub name: String,

    /// The engine used to resolve `run` calls.
    pub engine: Arc<dyn execution_engine::EngineService>,

    /// The execution context `run` calls run under.
    pub ctx: execution_engine::services::EngineInputContext,

    /// Where each call is logged.
    pub logger: LogWriter,
}

#[pymethods]
impl APIBindingWraper {
    /// Invokes operation `id` with `params` (and optional `options`, e.g.
    /// a result `limit`) and converts its result back to a Python object.
    pub fn run(
        &self,
        py: Python<'_>,
        id: &str,
        params: &PyAny,
        options: Option<&PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        self.logger
            .write_all(format!("{now} ({}) [API] {id}\n", self.name).as_bytes())?;

        let params = converters::from_py(params)?;

        let options = options
            .map(converters::from_py)
            .transpose()?
            .unwrap_or(Value::Null);

        let result = self
            .engine
            .run(id, params, options, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Making API Call: {e}")))?;

        converters::from_value(py, result)
    }
}

/// The `workflow` binding: exposes `workflow.log` for reporting the
/// script's overall outcome and progress.
#[pyclass]
pub struct Workflow {
    /// The workflow-level logger.
    #[pyo3(get)]
    pub log: WorkflowLogger,
}

/// Reports a running script's overall outcome (`done`/`fail`) and
/// progress (`info`/`warn`/`status`), writing each call to the shared log
/// and, for `done`/`fail`, recording the outcome in `output` so
/// [`PyActionRunner::run_internal`](crate::PyActionRunner) can return it
/// as the script's result instead of its literal return value.
#[pyclass]
#[derive(Clone)]
pub struct WorkflowLogger {
    /// The `{service}.{operation}` identifier of the running script.
    pub name: String,

    /// Where every call is logged.
    pub loggers: LogWriter,

    /// The output dict a `done`/`fail` call's outcome is recorded into.
    pub output: Py<PyDict>,
}

#[pymethods]
impl WorkflowLogger {
    /// Reports failure: logs `display` at [`constants::LOG_ERROR`], and
    /// records `standard_output_params`/`custom_output_params` (if given)
    /// under [`RESPONSE_ERROR_KEY`](constants::RESPONSE_ERROR_KEY) in
    /// `output`.
    fn fail(
        &mut self,
        py: Python<'_>,
        display: &PyAny,
        standard_output_params: Option<&PyAny>,
        custom_output_params: Option<&PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_ERROR)?;

        let output = PyDict::new(py);
        if let Some(standard_output_params) = standard_output_params {
            output.set_item(constants::RESPONSE_STANDARD_KEY, standard_output_params)?;
        }

        if let Some(custom_output_params) = custom_output_params {
            output.set_item(constants::RESPONSE_CUSTOM_KEY, custom_output_params)?;
        }

        self.output
            .as_ref(py)
            .set_item(constants::RESPONSE_ERROR_KEY, output)?;

        Ok(py.None())
    }

    /// Reports success: logs `display` (defaulting to `"done"`) at
    /// [`constants::LOG_SUCCESS`], and records
    /// `standard_output_params`/`custom_output_params` (if given) under
    /// [`RESPONSE_SUCCESS_KEY`](constants::RESPONSE_SUCCESS_KEY) in
    /// `output`.
    fn done(
        &mut self,
        py: Python<'_>,
        display: Option<&PyAny>,
        standard_output_params: Option<&PyAny>,
        custom_output_params: Option<&PyAny>,
    ) -> PyResult<Py<PyAny>> {
        if let Some(display) = display {
            self.print_display(display, constants::LOG_SUCCESS)?;
        } else {
            self.print_display(PyString::new(py, "done"), constants::LOG_SUCCESS)?;
        }

        let output = PyDict::new(py);
        if let Some(standard_output_params) = standard_output_params {
            output.set_item(constants::RESPONSE_STANDARD_KEY, standard_output_params)?;
        }

        if let Some(custom_output_params) = custom_output_params {
            output.set_item(constants::RESPONSE_CUSTOM_KEY, custom_output_params)?;
        }

        self.output
            .as_ref(py)
            .set_item(constants::RESPONSE_SUCCESS_KEY, output)?;

        Ok(py.None())
    }

    /// Logs `display` at [`constants::LOG_WARN`], without affecting the
    /// script's recorded outcome.
    fn warn(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_WARN)?;
        Ok(py.None())
    }

    /// Logs `display` at [`constants::LOG_STATUS`], tagged with
    /// `groupId`, without affecting the script's recorded outcome.
    #[allow(
        non_snake_case,
        reason = "groupId matches the script-facing binding's parameter name"
    )]
    fn status(&mut self, py: Python<'_>, display: &PyAny, groupId: &str) -> PyResult<Py<PyAny>> {
        self.print_display(display, &format!("{}={groupId}", constants::LOG_STATUS))?;
        Ok(py.None())
    }

    /// Logs `display` at [`constants::LOG_INFO`], without affecting the
    /// script's recorded outcome.
    fn info(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_INFO)?;
        Ok(py.None())
    }

    /// Writes a timestamped log line for `display` (either a string, or a
    /// dict with a string `summary` key) at `log_level`. Errors if
    /// `display` is neither shape.
    fn print_display(&mut self, display: &PyAny, log_level: &str) -> PyResult<()> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        if display.is_instance_of::<PyDict>()? {
            let display = display.downcast::<PyDict>()?;
            let summary = display
                .get_item("summary")
                .and_then(|s| s.downcast::<PyString>().ok())
                .and_then(|s| s.to_str().ok())
                .ok_or_else(|| PyTypeError::new_err("Unable to find summary in display object"))?;

            self.loggers.write_all(
                format!("{now} ({}) [workflow|{log_level}]: {summary}\n", self.name).as_bytes(),
            )?;
        } else if display.is_instance_of::<PyString>()? {
            let summary = display.downcast::<PyString>()?.to_str()?;

            self.loggers.write_all(
                format!("{now} ({}) [workflow|{log_level}]: {summary}\n", self.name).as_bytes(),
            )?;
        } else {
            return Err(PyTypeError::new_err("Invalid type for display object"));
        }

        Ok(())
    }
}

/// The `action` binding: exposes `action.log` for reporting a single
/// action-level log entry (as opposed to the workflow-level outcome
/// reported via [`Workflow`]).
#[pyclass]
pub struct Action {
    /// The action-level logger.
    #[pyo3(get)]
    pub log: ActionLogger,
}

#[pymethods]
impl Action {
    /// Logs `display` at [`constants::LOG_SUCCESS`].
    fn post(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.log.print_display(display, constants::LOG_SUCCESS)?;
        Ok(py.None())
    }
}

/// Writes action-level log entries to the shared log file.
#[pyclass]
#[derive(Clone)]
pub struct ActionLogger {
    /// The `{service}.{operation}` identifier of the running script.
    pub name: String,

    /// Where every call is logged.
    pub logger: LogWriter,
}

#[pymethods]
impl ActionLogger {
    /// Logs `display` at [`constants::LOG_ERROR`].
    fn error(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_ERROR)?;
        Ok(py.None())
    }

    /// Logs `display` at [`constants::LOG_WARN`].
    fn warn(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_WARN)?;
        Ok(py.None())
    }

    /// Logs `display` at [`constants::LOG_INFO`].
    fn info(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_INFO)?;
        Ok(py.None())
    }

    /// Writes a timestamped log line for `display` (either a string, or a
    /// dict with a string `summary` key) at `log_level`. Errors if
    /// `display` is neither shape.
    fn print_display(&mut self, display: &PyAny, log_level: &str) -> PyResult<()> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        if display.is_instance_of::<PyDict>()? {
            let display = display.downcast::<PyDict>()?;
            let summary = display
                .get_item("summary")
                .and_then(|s| s.downcast::<PyString>().ok())
                .and_then(|s| s.to_str().ok())
                .ok_or_else(|| PyTypeError::new_err("Unable to find summary in display object"))?;

            self.logger.write_all(
                format!("{now} ({}) [action|{log_level}]: {summary}\n", self.name).as_bytes(),
            )?;
        } else if display.is_instance_of::<PyString>()? {
            let summary = display.downcast::<PyString>()?.to_str()?;

            self.logger.write_all(
                format!("{now} ({}) [action|{log_level}]: {summary}\n", self.name).as_bytes(),
            )?;
        } else {
            return Err(PyTypeError::new_err("Invalid type for display object"));
        }

        Ok(())
    }
}
