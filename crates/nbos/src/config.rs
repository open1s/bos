use pyo3::prelude::*;
use std::sync::Mutex;

use crate::utils::{json_to_py, parse_merge_strategy, py_to_json, to_py_runtime_error};
use config::loader::ConfigLoader;

#[pyclass(name = "ConfigLoader", skip_from_py_object)]
pub struct PyConfigLoader {
    pub inner: Mutex<ConfigLoader>,
}

#[pymethods]
impl PyConfigLoader {
    #[new]
    #[pyo3(signature = (strategy = None))]
    fn new(strategy: Option<&str>) -> PyResult<Self> {
        let strategy = parse_merge_strategy(strategy)?;
        Ok(Self {
            inner: Mutex::new(ConfigLoader::new().with_strategy(strategy)),
        })
    }

    fn discover(&self) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("ConfigLoader lock poisoned"))?;
        guard.discover_mut();
        Ok(())
    }

    fn add_file(&self, path: String) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("ConfigLoader lock poisoned"))?;
        guard.add_file_mut(path);
        Ok(())
    }

    fn add_directory(&self, path: String) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("ConfigLoader lock poisoned"))?;
        guard.add_directory_mut(path).map_err(to_py_runtime_error)?;
        Ok(())
    }

    fn add_inline(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("ConfigLoader lock poisoned"))?;
        guard.add_inline_mut(py_to_json(value)?);
        Ok(())
    }

    fn reset(&self) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("ConfigLoader lock poisoned"))?;
        guard.reset();
        Ok(())
    }

    fn load_sync(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("ConfigLoader lock poisoned"))?;
        let value = guard.load_sync().map_err(to_py_runtime_error)?;
        json_to_py(py, &value)
    }

    fn reload_sync(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("ConfigLoader lock poisoned"))?;
        guard.reset();
        let value = guard.load_sync().map_err(to_py_runtime_error)?;
        json_to_py(py, &value)
    }
}
