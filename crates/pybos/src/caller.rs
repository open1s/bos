use pyo3::prelude::*;
use pyo3::types::PyType;
use std::sync::Arc;
use pyo3::IntoPyObjectExt;
use bus::{Caller, Callable};
use crate::bus::PyBus;
use crate::utils::{
    session_from_bus,
    invoke_python_handler_to_pyany,
    invoke_python_string_handler,
};

#[pyclass(name = "Caller", skip_from_py_object)]
#[derive(Clone)]
pub struct PyCaller {
    pub inner: Arc<Caller>,  
}

#[pymethods]
impl PyCaller {
    #[classmethod]
    fn create<'py>(
        _cls: &Bound<'py, PyType>,
        py: Python<'py>,
        bus: PyRef<'py, PyBus>,
        name: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus_inner = bus.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let session = session_from_bus(bus_inner).await;
            let caller = Arc::new(Caller::new(name, Some(session)));
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_obj = Py::new(py, PyCaller { inner: caller })?;
                Ok(py_obj.into_any())
            })
        })
    }

    fn call_text<'py>(&self, py: Python<'py>, payload: String) -> PyResult<Bound<'py, PyAny>> {
        let caller = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let out = caller
                .call::<String, String>(&payload)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Ok(out)
        })
    }
}

#[pyclass(name = "Callable", skip_from_py_object)]
#[derive(Clone)]
pub struct PyCallable {
    pub inner: Arc<tokio::sync::Mutex<Callable<String, String>>>,
}

#[pymethods]
impl PyCallable {
    #[classmethod]
    #[pyo3(signature = (bus, uri, handler = None))]
    fn create<'py>(
        _cls: &Bound<'py, PyType>,
        py: Python<'py>,
        bus: PyRef<'py, PyBus>,
        uri: String,
        handler: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus_inner = bus.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let session = session_from_bus(bus_inner).await;
            let callable = if let Some(cb) = handler {
                let callback = Arc::new(cb);
                Callable::<String, String>::new(&uri, session).with_handler(move |input| {
                    let callback = callback.clone();
                    async move { invoke_python_string_handler(callback.as_ref(), input).await }
                })
            } else {
                Callable::<String, String>::new(&uri, session)
                    .with_handler(|input| async move { Ok(input) })
            };

            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_obj = Py::new(
                    py,
                    PyCallable {
                        inner: Arc::new(tokio::sync::Mutex::new(callable)),
                    },
                )?;
                Ok(py_obj.into_any())
            })
        })
    }

    fn start<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = inner.lock().await;
            guard.start().await.map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }

    fn is_started<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let guard = inner.lock().await;
            Ok(guard.is_started())
        })
    }

    fn run<'py>(&self, py: Python<'py>, handler: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = inner.lock().await;

            // Wrap handler in Arc for cheap cloning in closure
            let handler = std::sync::Arc::new(handler);

            // Set handler that wraps Python callback
            let handler_fn = move |input: String| {
                let handler = handler.clone();
                async move {
                    let py_arg = Python::attach(|py| -> PyResult<Py<PyAny>> {
                        Ok(input.into_py_any(py)?)
                    })
                    .map_err(|e| bus::ZenohError::Query(e.to_string()))?;

                    let output = invoke_python_handler_to_pyany(&handler, py_arg)
                        .await?;

                    let out_str = Python::attach(|py| {
                        output
                            .bind(py)
                            .extract::<String>()
                            .map_err(|e| bus::ZenohError::Query(e.to_string()))
                    })
                    .map_err(|e| bus::ZenohError::Query(e.to_string()))?;

                    Ok(out_str)
                }
            };

            guard
                .set_handler(handler_fn)
                .map_err(crate::utils::to_py_runtime_error)?;

            guard.init_and_run().await.map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }

    fn run_json<'py>(&self, py: Python<'py>, handler: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = inner.lock().await;

            // Wrap handler in Arc for cheap cloning in closure
            let handler = std::sync::Arc::new(handler);

            let handler_fn = move |json_str: String| {
                let handler = handler.clone();
                async move {
                    let py_arg = Python::attach(|py| -> PyResult<Py<PyAny>> {
                        let json_value: serde_json::Value = serde_json::from_str(&json_str)
                            .map_err(|e| crate::utils::to_py_runtime_error(e))?;
                        crate::utils::json_to_py(py, &json_value)
                    })
                    .map_err(|e| bus::ZenohError::Query(e.to_string()))?;

                    let output = invoke_python_handler_to_pyany(&handler, py_arg)
                        .await?;

                    let out_json = Python::attach(|py| -> PyResult<String> {
                        let py_out = crate::utils::py_to_json(output.bind(py))?;
                        Ok(py_out.to_string())
                    })
                    .map_err(|e| bus::ZenohError::Query(e.to_string()))?;

                    Ok(out_json)
                }
            };

            guard
                .set_handler(handler_fn)
                .map_err(crate::utils::to_py_runtime_error)?;

            guard.init_and_run().await.map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }
}
