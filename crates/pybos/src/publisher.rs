use pyo3::prelude::*;
use pyo3::types::PyType;

use bus::Publisher;
use crate::bus::PyBus;
use crate::utils::session_from_bus;

#[pyclass(name = "Publisher", skip_from_py_object)]
#[derive(Clone)]
pub struct PyPublisher {
    pub inner: Publisher,
}

#[pymethods]
impl PyPublisher {
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
            let publisher = Publisher::new(topic)
                .with_session(session)
                .map_err(crate::utils::to_py_runtime_error)?;
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_obj = Py::new(py, PyPublisher { inner: publisher })?;
                Ok(py_obj.into_any())
            })
        })
    }

    fn topic(&self) -> String {
        self.inner.topic().to_string()
    }

    fn publish_text<'py>(&self, py: Python<'py>, payload: String) -> PyResult<Bound<'py, PyAny>> {
        let publisher = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            publisher
                .publish(&payload)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }

    fn publish_json<'py>(&self, py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        // Convert Python dict to serde_json::Value
        let json_value = crate::utils::py_to_json(data)?;
        // Convert to JSON string
        let json_str = json_value.to_string();
        
        let publisher = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            publisher
                .publish(&json_str)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }
}
