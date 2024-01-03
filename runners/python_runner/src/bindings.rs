#![allow(clippy::std_instead_of_core)]

//!

extern crate alloc;
use alloc::sync::Arc;

use std::io::Write;
use std::sync::RwLock;
use std::thread;

use core::time::Duration;

use super::{constants, converters, File};
use pyo3::exceptions::{PyArithmeticError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use serde_json::Value;

///
#[pyclass]
pub struct TaskBinding {
    ///
    pub name: String,

    ///
    pub engine: Arc<RwLock<execution_engine::Engine>>,

    ///
    pub ctx: execution_engine::services::EngineInputContext,

    ///
    pub logger: Arc<RwLock<File>>,
}

#[pymethods]
impl TaskBinding {
    ///
    pub fn create(&self, id: String, params: &PyAny) -> Task {
        Task {
            id,
            params: params.into(),
            name: self.name.clone(),
            engine: Arc::<RwLock<execution_engine::Engine>>::clone(&self.engine),
            ctx: execution_engine::services::EngineInputContext::new(
                self.ctx.parent.clone(),
                self.ctx.execution_id.clone(),
                false,
            ),
            logger: Arc::<RwLock<File>>::clone(&self.logger),
        }
    }
}

///
#[pyclass]
pub struct Task {
    ///
    pub name: String,

    ///
    pub engine: Arc<RwLock<execution_engine::Engine>>,

    ///
    pub ctx: execution_engine::services::EngineInputContext,

    ///
    pub logger: Arc<RwLock<File>>,

    ///
    pub id: String,

    ///
    pub params: Py<PyAny>,
}

#[pymethods]
impl Task {
    ///
    #[pyo3(name = "continueAfter")]
    pub fn continue_after(&self, py: Python<'_>, delay: u64, unit: &str) -> PyResult<Py<PyAny>> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        {
            let mut logger = self
                .logger
                .write()
                .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

            logger
                .write_all(format!("{now} ({}) [TASK|WAIT] {}\n", self.name, self.id).as_bytes())?;
        };

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

        let engine = self
            .engine
            .read()
            .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

        let result = engine
            .run(&self.id, params, options, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Making API Call: {e}")))?;

        converters::from_value(py, result)
    }

    ///
    #[pyo3(name = "continueAfterUserInput")]
    pub fn continue_after_user_input(&self, py: Python<'_>, blocks: &PyAny) -> PyResult<Py<PyAny>> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        {
            let mut logger = self
                .logger
                .write()
                .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

            logger.write_all(
                format!("{now} ({}) [TASK|INPUT] {}\n", self.name, self.id).as_bytes(),
            )?;
        };

        let blocks = converters::from_py(blocks)?;

        let engine = self
            .engine
            .read()
            .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

        let result = engine
            .run("$input", blocks, Value::Null, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Collecting Input: {e}")))?;

        let mut params = converters::from_py(self.params.as_ref(py))?;

        if let &mut Value::Object(ref mut map) = &mut params {
            map.insert("input_results".into(), result);
        } else {
            // TODO: Verify this functionality
            return Err(PyValueError::new_err("Expected parameters to be an Object"));
        }

        let options = Value::Null;

        let result = engine
            .run(&self.id, params, options, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Making API Call: {e}")))?;

        converters::from_value(py, result)
    }
}

///
#[pyclass]
pub struct APIBindingWraper {
    ///
    pub name: String,

    ///
    pub engine: Arc<RwLock<execution_engine::Engine>>,

    ///
    pub ctx: execution_engine::services::EngineInputContext,

    ///
    pub logger: Arc<RwLock<File>>,
}

#[pymethods]
impl APIBindingWraper {
    ///
    pub fn run(
        &self,
        py: Python<'_>,
        id: &str,
        params: &PyAny,
        options: Option<&PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        {
            let mut logger = self
                .logger
                .write()
                .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

            logger.write_all(format!("{now} ({}) [API] {id}\n", self.name).as_bytes())?;
        };

        let params = converters::from_py(params)?;

        let options = options
            .map(converters::from_py)
            .transpose()?
            .unwrap_or(Value::Null);

        let engine = self
            .engine
            .read()
            .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

        let result = engine
            .run(id, params, options, &self.ctx)
            .map_err(|e| PyValueError::new_err(format!("Error Making API Call: {e}")))?;

        converters::from_value(py, result)
    }
}

///
#[pyclass]
pub struct Workflow {
    ///
    #[pyo3(get)]
    pub log: WorkflowLogger,
}

///
#[pyclass]
#[derive(Clone)]
pub struct WorkflowLogger {
    ///
    pub name: String,

    ///
    pub loggers: Arc<RwLock<File>>,

    ///
    pub output: Py<PyDict>,
}

#[pymethods]
impl WorkflowLogger {
    ///
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

    ///
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

    ///
    fn warn(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_WARN)?;
        Ok(py.None())
    }

    ///
    #[allow(non_snake_case)]
    fn status(&mut self, py: Python<'_>, display: &PyAny, groupId: &str) -> PyResult<Py<PyAny>> {
        self.print_display(display, &format!("{}={groupId}", constants::LOG_STATUS))?;
        Ok(py.None())
    }

    ///
    fn info(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_INFO)?;
        Ok(py.None())
    }

    ///
    fn print_display(&mut self, display: &PyAny, log_level: &str) -> PyResult<()> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        let mut logger = self
            .loggers
            .write()
            .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

        if display.is_instance_of::<PyDict>()? {
            let display = display.downcast::<PyDict>()?;
            let summary = display
                .get_item("summary")
                .and_then(|s| s.downcast::<PyString>().ok())
                .and_then(|s| s.to_str().ok())
                .ok_or_else(|| PyTypeError::new_err("Unable to find summary in display object"))?;

            logger.write_all(
                format!("{now} ({}) [workflow|{log_level}]: {summary}\n", self.name).as_bytes(),
            )?;
        } else if display.is_instance_of::<PyString>()? {
            let summary = display.downcast::<PyString>()?.to_str()?;

            logger.write_all(
                format!("{now} ({}) [workflow|{log_level}]: {summary}\n", self.name).as_bytes(),
            )?;
        } else {
            return Err(PyTypeError::new_err("Invalid type for display object"));
        }

        Ok(())
    }
}

///
#[pyclass]
pub struct Action {
    ///
    #[pyo3(get)]
    pub log: ActionLogger,
}

#[pymethods]
impl Action {
    ///
    fn post(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.log.print_display(display, constants::LOG_SUCCESS)?;
        Ok(py.None())
    }
}

///
#[pyclass]
#[derive(Clone)]
pub struct ActionLogger {
    ///
    pub name: String,

    ///
    pub logger: Arc<RwLock<File>>,
}

#[pymethods]
impl ActionLogger {
    ///
    fn error(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_ERROR)?;
        Ok(py.None())
    }

    ///
    fn warn(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_WARN)?;
        Ok(py.None())
    }

    ///
    fn info(&mut self, py: Python<'_>, display: &PyAny) -> PyResult<Py<PyAny>> {
        self.print_display(display, constants::LOG_INFO)?;
        Ok(py.None())
    }

    ///
    fn print_display(&mut self, display: &PyAny, log_level: &str) -> PyResult<()> {
        let now = chrono::offset::Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        let mut logger = self
            .logger
            .write()
            .map_err(|e| PyValueError::new_err(format!("Locking Error: {e}")))?;

        if display.is_instance_of::<PyDict>()? {
            let display = display.downcast::<PyDict>()?;
            let summary = display
                .get_item("summary")
                .and_then(|s| s.downcast::<PyString>().ok())
                .and_then(|s| s.to_str().ok())
                .ok_or_else(|| PyTypeError::new_err("Unable to find summary in display object"))?;

            logger.write_all(
                format!(
                    "{now} ({}) [action|{log_level}]: {summary}\n",
                    self.name
                )
                .as_bytes(),
            )?;
        } else if display.is_instance_of::<PyString>()? {
            let summary = display.downcast::<PyString>()?.to_str()?;

            logger.write_all(
                format!(
                    "{now} ({}) [action|{log_level}]: {summary}\n",
                    self.name
                )
                .as_bytes(),
            )?;
        } else {
            return Err(PyTypeError::new_err("Invalid type for display object"));
        }

        Ok(())
    }
}
