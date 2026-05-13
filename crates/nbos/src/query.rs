use crate::bus::PyBus;
use crate::utils::{
    invoke_python_handler_to_pyany, invoke_python_string_handler, session_from_bus,
};
use bus::{Query, QueryableWrapper};
use pyo3::prelude::*;
use pyo3::types::PyType;
use pyo3::IntoPyObjectExt;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Python-exposed sender for streaming query replies.
/// Python handlers call `sender.send(chunk)` to push replies.
#[pyclass(name = "StreamSender", skip_from_py_object)]
pub struct PyStreamSender {
    inner: Option<tokio::sync::mpsc::Sender<Result<String, bus::ZenohError>>>,
}

impl PyStreamSender {
    pub fn new(tx: tokio::sync::mpsc::Sender<Result<String, bus::ZenohError>>) -> Self {
        Self { inner: Some(tx) }
    }
}

#[pymethods]
impl PyStreamSender {
    fn send(&self, chunk: String) -> PyResult<()> {
        if let Some(tx) = &self.inner {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async { tx.send(Ok(chunk)).await })
            })
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        }
        Ok(())
    }

    fn close(&self) {
        // Drop the sender to signal stream end
    }
}

/// Async iterator that yields query results one at a time.
#[pyclass(name = "QueryStreamIterator", skip_from_py_object)]
pub struct PyQueryStreamIterator {
    inner: Arc<tokio::sync::Mutex<Option<mpsc::Receiver<Result<Vec<u8>, bus::ZenohError>>>>>,
}

#[pymethods]
impl PyQueryStreamIterator {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        let receiver = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;

        let future = pyo3_async_runtimes::tokio::future_into_py_with_locals(
            py,
            current_locals,
            async move {
                let mut guard = receiver.lock().await;
                let rx = guard.as_mut().ok_or_else(|| {
                    pyo3::exceptions::PyStopAsyncIteration::new_err("Stream exhausted")
                })?;

                match rx.recv().await {
                    Some(Ok(bytes)) => {
                        let codec = bus::DEFAULT_CODEC;
                        let text: String = codec.decode(&bytes).map_err(|e| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })?;
                        Ok(text)
                    }
                    Some(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
                    None => {
                        *guard = None;
                        Err(pyo3::exceptions::PyStopAsyncIteration::new_err(
                            "Stream complete",
                        ))
                    }
                }
            },
        );

        match future {
            Ok(bound) => Ok(Some(bound)),
            Err(e) => Err(e),
        }
    }
}

#[pyclass(name = "Query", skip_from_py_object)]
#[derive(Clone)]
pub struct PyQuery {
    pub inner: Query,
}

#[pymethods]
impl PyQuery {
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
            let query = Query::new(topic)
                .with_session(session)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_obj = Py::new(py, PyQuery { inner: query })?;
                Ok(py_obj.into_any())
            })
        })
    }

    fn topic(&self) -> String {
        self.inner.topic().to_string()
    }

    fn query_text<'py>(&self, py: Python<'py>, payload: String) -> PyResult<Bound<'py, PyAny>> {
        let query = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let out = query
                .query::<String, String>(&payload)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Ok(out)
        })
    }

    fn query_text_timeout_ms<'py>(
        &self,
        py: Python<'py>,
        payload: String,
        timeout_ms: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let query = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let out = query
                .query_with_timeout::<String, String>(
                    &payload,
                    std::time::Duration::from_millis(timeout_ms),
                )
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Ok(out)
        })
    }

    fn stream_text<'py>(&self, py: Python<'py>, payload: String) -> PyResult<Bound<'py, PyAny>> {
        let query = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let codec = bus::DEFAULT_CODEC;
            let bytes = codec
                .encode(&payload)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            let rx = query
                .stream_channel(&bytes)
                .await
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let iter = PyQueryStreamIterator {
                    inner: Arc::new(tokio::sync::Mutex::new(Some(rx))),
                };
                Ok(Py::new(py, iter)?.into_any())
            })
        })
    }
}

#[pyclass(name = "Queryable", skip_from_py_object)]
#[derive(Clone)]
pub struct PyQueryable {
    pub inner: Arc<tokio::sync::Mutex<QueryableWrapper<String, String>>>,
}

#[pymethods]
impl PyQueryable {
    #[classmethod]
    #[pyo3(signature = (bus, topic, handler = None))]
    fn create<'py>(
        _cls: &Bound<'py, PyType>,
        py: Python<'py>,
        bus: PyRef<'py, PyBus>,
        topic: String,
        handler: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus_inner = bus.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let session = session_from_bus(bus_inner).await;

            let mut queryable = if let Some(cb) = handler {
                let callback = Arc::new(cb);
                QueryableWrapper::<String, String>::new(topic).with_handler(move |input| {
                    let callback = callback.clone();
                    async move { invoke_python_string_handler(callback.as_ref(), input).await }
                })
            } else {
                QueryableWrapper::<String, String>::new(topic)
                    .with_handler(|input| async move { Ok(input) })
            };

            queryable
                .init(&session)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;

            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_obj = Py::new(
                    py,
                    PyQueryable {
                        inner: Arc::new(tokio::sync::Mutex::new(queryable)),
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
            guard.run().map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }

    fn run<'py>(&self, py: Python<'py>, handler: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = inner.lock().await;

            // Wrap handler in Arc for cheap cloning in closure
            let handler = std::sync::Arc::new(handler);

            let handler_fn = move |input: String| {
                let handler = handler.clone();
                async move {
                    let py_arg =
                        Python::attach(|py| -> PyResult<Py<PyAny>> { input.into_py_any(py) })
                            .map_err(|e| bus::ZenohError::Query(e.to_string()))?;

                    let output = invoke_python_handler_to_pyany(&handler, py_arg).await?;

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

            // Note: init() was already called in create(), just run now
            guard.run().map_err(crate::utils::to_py_runtime_error)?;
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
                            .map_err(crate::utils::to_py_runtime_error)?;
                        crate::utils::json_to_py(py, &json_value)
                    })
                    .map_err(|e| bus::ZenohError::Query(e.to_string()))?;

                    let output = invoke_python_handler_to_pyany(&handler, py_arg).await?;

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

            guard.run().map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }

    fn run_stream<'py>(&self, py: Python<'py>, handler: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = inner.lock().await;

            let handler = std::sync::Arc::new(handler);

            let handler_fn = move |input: String,
                                   tx: tokio::sync::mpsc::Sender<
                Result<String, bus::ZenohError>,
            >| {
                let handler = handler.clone();
                async move {
                    let tx_wrapper = PyStreamSender::new(tx);

                    let _ = Python::attach(|py| {
                        let py_arg = input
                            .into_py_any(py)
                            .map_err(|e| bus::ZenohError::Query(e.to_string()))?;
                        let py_tx = Py::new(py, tx_wrapper)
                            .map_err(|e| bus::ZenohError::Query(e.to_string()))?;
                        handler
                            .call1(py, (py_arg, py_tx))
                            .map_err(|e| bus::ZenohError::Query(e.to_string()))?;
                        Ok::<_, bus::ZenohError>(())
                    });
                }
            };

            guard
                .set_stream_handler(handler_fn)
                .map_err(crate::utils::to_py_runtime_error)?;

            guard.run().map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }
}
