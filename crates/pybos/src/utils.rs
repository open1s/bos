use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};
use pyo3::IntoPyObjectExt;
use std::sync::Arc;

use bus::Bus;

pub fn to_py_runtime_error<E: std::fmt::Display>(err: E) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

pub async fn session_from_bus(inner: Arc<tokio::sync::Mutex<Bus>>) -> Arc<bus::Session> {
    let guard = inner.lock().await;
    let bus_copy = guard.clone();
    bus_copy.session()
}

pub async fn invoke_python_handler_to_pyany(
    callback: &Py<PyAny>,
    arg: Py<PyAny>,
) -> Result<Py<PyAny>, bus::ZenohError> {
    // Call the Python callback (sync for now)
    let result: Py<PyAny> = Python::attach(|py| -> PyResult<Py<PyAny>> {
        let raw_out = callback.bind(py).call1((arg.as_ref(),))?;
        raw_out.into_py_any(py)
    })
    .map_err(|e: PyErr| bus::ZenohError::Query(e.to_string()))?;

    Ok(result)
}

pub async fn invoke_python_string_handler(
    callback: &Py<PyAny>,
    input: String,
) -> Result<String, bus::ZenohError> {
    let py_arg = Python::attach(|py| -> PyResult<Py<PyAny>> { input.into_py_any(py) })
        .map_err(|e| bus::ZenohError::Query(e.to_string()))?;

    let result_obj = invoke_python_handler_to_pyany(callback, py_arg).await?;

    Python::attach(|py| {
        result_obj
            .bind(py)
            .extract::<String>()
            .map_err(|e| bus::ZenohError::Query(e.to_string()))
    })
    .map_err(|e| bus::ZenohError::Query(e.to_string()))
}

pub fn parse_merge_strategy(
    strategy: Option<&str>,
) -> PyResult<config::types::ConfigMergeStrategy> {
    match strategy.unwrap_or("override").to_ascii_lowercase().as_str() {
        "override" => Ok(config::types::ConfigMergeStrategy::Override),
        "deep_merge" | "deepmerge" => Ok(config::types::ConfigMergeStrategy::DeepMerge),
        "first" => Ok(config::types::ConfigMergeStrategy::First),
        "accumulate" => Ok(config::types::ConfigMergeStrategy::Accumulate),
        other => Err(PyValueError::new_err(format!(
            "Unsupported merge strategy '{}'. Use one of: override, deep_merge, first, accumulate",
            other
        ))),
    }
}

pub fn py_to_json(value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if value.is_none() {
        return Ok(serde_json::Value::Null);
    }

    if value.is_instance_of::<PyBool>() {
        return Ok(serde_json::Value::Bool(value.extract::<bool>()?));
    }
    if value.is_instance_of::<PyInt>() {
        let v = value.extract::<i64>()?;
        return Ok(serde_json::Value::Number(v.into()));
    }
    if value.is_instance_of::<PyFloat>() {
        let v = value.extract::<f64>()?;
        let num = serde_json::Number::from_f64(v).ok_or_else(|| {
            PyValueError::new_err("Float must be a finite number for JSON conversion")
        })?;
        return Ok(serde_json::Value::Number(num));
    }
    if value.is_instance_of::<PyString>() {
        return Ok(serde_json::Value::String(value.extract::<String>()?));
    }
    if let Ok(dict) = value.cast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in dict {
            let key = k.extract::<String>()?;
            map.insert(key, py_to_json(&v)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    if let Ok(list) = value.cast::<PyList>() {
        let mut out = Vec::with_capacity(list.len());
        for item in list.iter() {
            out.push(py_to_json(&item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Ok(tuple) = value.cast::<PyTuple>() {
        let mut out = Vec::with_capacity(tuple.len());
        for item in tuple.iter() {
            out.push(py_to_json(&item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }

    Err(PyValueError::new_err(format!(
        "Unsupported Python value for JSON conversion: '{}'",
        value.get_type().name()?
    )))
}

pub fn json_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(v) => Ok((*v).into_pyobject(py)?.to_owned().into_any().unbind()),
        serde_json::Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                Ok(i.into_pyobject(py)?.into_any().unbind())
            } else if let Some(u) = v.as_u64() {
                Ok(u.into_pyobject(py)?.into_any().unbind())
            } else if let Some(f) = v.as_f64() {
                Ok(f.into_pyobject(py)?.into_any().unbind())
            } else {
                Err(PyValueError::new_err("Unsupported JSON number"))
            }
        }
        serde_json::Value::String(v) => Ok(v.clone().into_pyobject(py)?.into_any().unbind()),
        serde_json::Value::Array(vs) => {
            let list = PyList::empty(py);
            for item in vs {
                list.append(json_to_py(py, item)?)?;
            }
            Ok(list.into_any().unbind())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(k, json_to_py(py, v)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}
