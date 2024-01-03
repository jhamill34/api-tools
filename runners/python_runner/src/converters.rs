//!

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use serde_json::Value;

// TODO: Use the serde feature....

///
pub fn from_value(py: Python, item: Value) -> PyResult<Py<PyAny>> {
    let result = match item {
        Value::Null => py.None(),
        Value::Bool(val) => PyBool::new(py, val).into(),
        Value::Number(val) if val.is_i64() => {
            if let Some(val) = val.as_i64() {
                val.into_py(py)
            } else {
                py.None()
            }
        }
        Value::Number(val) => {
            if let Some(val) = val.as_f64() {
                PyFloat::new(py, val).into()
            } else {
                py.None()
            }
        }
        Value::String(val) => PyString::new(py, &val).into(),
        Value::Array(val) => {
            let items: PyResult<Vec<_>> = val
                .into_iter()
                .map(|v| {
                    let v = from_value(py, v)?;
                    Ok(v)
                })
                .collect();

            PyList::new(py, items?).into()
        }
        Value::Object(val) => {
            let dict = PyDict::new(py);

            for (key, item) in &val {
                let value = from_value(py, item.clone())?;
                dict.set_item(key, value)?;
            }

            dict.into()
        }
    };

    Ok(result)
}

///
pub fn from_py(item: &PyAny) -> PyResult<serde_json::Value> {
    let result = if item.is_instance_of::<PyBool>()? {
        let item = item.downcast::<PyBool>()?;
        serde_json::Value::Bool(item.is_true())
    } else if item.is_instance_of::<PyFloat>()? {
        let item = item.downcast::<PyFloat>()?;
        let item: f64 = item.extract()?;
        let item = serde_json::Number::from_f64(item)
            .ok_or_else(|| PyTypeError::new_err("Invalid Number Type"))?;

        serde_json::Value::Number(item)
    } else if item.is_instance_of::<PyInt>()? {
        let item = item.downcast::<PyInt>()?;
        let item: i64 = item.extract()?;
        let item = serde_json::Number::from(item);
        serde_json::Value::Number(item)
    } else if item.is_instance_of::<PyString>()? {
        let item = item.downcast::<PyString>()?;

        serde_json::Value::String(item.to_str()?.to_owned())
    } else if item.is_instance_of::<PyList>()? {
        let item = item.downcast::<PyList>()?;
        let items: PyResult<Vec<_>> = item.into_iter().map(from_py).collect();

        serde_json::Value::Array(items?)
    } else if item.is_instance_of::<PyDict>()? {
        let item = item.downcast::<PyDict>()?;

        let mut map = serde_json::Map::new();

        for (key, value) in item {
            if key.is_instance_of::<PyString>()? {
                let key = key.downcast::<PyString>()?;
                map.insert(key.to_str()?.to_owned(), from_py(value)?);
            }
        }

        serde_json::Value::Object(map)
    } else {
        serde_json::Value::Null
    };

    Ok(result)
}
