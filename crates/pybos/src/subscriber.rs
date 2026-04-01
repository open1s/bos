use pyo3::prelude::*;
use pyo3::types::PyType;
use std::sync::Arc;

use bus::Subscriber;
use crate::bus::PyBus;
use crate::utils::session_from_bus;

#[pyclass(name = "Subscriber", skip_from_py_object)]
#[derive(Clone)]
pub struct PySubscriber {
    pub inner: Arc<tokio::sync::Mutex<Subscriber<String>>>,
}

#[pymethods]
impl PySubscriber {
    #[classmethod]
    fn create<'py>(
        _cls: &Bound<'py, PyType>,
        py: Python<'py>,
        bus: PyRef<'py, PyBus>,
        topic: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus_inner = bus.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let session = session_from_bus(bus_inner).await;
            let sub = Subscriber::<String>::new(topic)
                .with_session(session)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_obj = Py::new(
                    py,
                    PySubscriber {
                        inner: Arc::new(tokio::sync::Mutex::new(sub)),
                    },
                )?;
                Ok(py_obj.into_any())
            })
        })
    }

    fn recv<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = inner.lock().await;
            Ok(guard.recv().await)
        })
    }

    fn recv_with_timeout_ms<'py>(
        &self,
        py: Python<'py>,
        timeout_ms: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = inner.lock().await;
            let out = guard
                .recv_with_timeout(std::time::Duration::from_millis(timeout_ms))
                .await;
            Ok(out)
        })
    }

    fn recv_json_with_timeout_ms<'py>(
        &self,
        py: Python<'py>,
        timeout_ms: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = inner.lock().await;
            let out = guard
                .recv_with_timeout(std::time::Duration::from_millis(timeout_ms))
                .await;
            
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                match out {
                    Some(json_str) => {
                        // Parse JSON string to serde_json::Value
                        let json_value: serde_json::Value = serde_json::from_str(&json_str)
                            .map_err(|e| crate::utils::to_py_runtime_error(e))?;
                        // Convert serde_json::Value to Python object
                        crate::utils::json_to_py(py, &json_value)
                    }
                    None => Ok(py.None()),
                }
            })
        })
    }

    fn run<'py>(&self, py: Python<'py>, callback: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = inner.lock().await;
            
            // Continuously receive messages and call the callback
            loop {
                match guard.recv().await {
                    Some(message) => {
                        // Call Python callback with the message
                        Python::attach(|py| -> PyResult<()> {
                            callback.bind(py).call1((message,))?;
                            Ok(())
                        })?;
                    }
                    None => {
                        // Subscription ended
                        break;
                    }
                }
            }
            Ok(())
        })
    }

    fn run_json<'py>(&self, py: Python<'py>, callback: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = inner.lock().await;
            
            // Continuously receive messages and call the callback with JSON
            loop {
                match guard.recv().await {
                    Some(json_str) => {
                        // Parse JSON string and call callback
                        Python::attach(|py| -> PyResult<()> {
                            let json_value: serde_json::Value = serde_json::from_str(&json_str)
                                .map_err(|e| crate::utils::to_py_runtime_error(e))?;
                            let py_dict = crate::utils::json_to_py(py, &json_value)?;
                            callback.bind(py).call1((py_dict,))?;
                            Ok(())
                        })?;
                    }
                    None => {
                        // Subscription ended
                        break;
                    }
                }
            }
            Ok(())
        })
    }
}
