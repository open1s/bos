use bus::Bus;
use bus::BusConfig;
use pyo3::prelude::*;
use pyo3::types::PyType;
use std::sync::Arc;

#[pyclass(name = "BusConfig", skip_from_py_object)]
#[derive(Clone)]
pub struct PyBusConfig {
    #[pyo3(get, set)]
    pub mode: String,
    #[pyo3(get, set)]
    pub connect: Option<Vec<String>>,
    #[pyo3(get, set)]
    pub listen: Option<Vec<String>>,
    #[pyo3(get, set)]
    pub peer: Option<String>,
}

impl Default for PyBusConfig {
    fn default() -> Self {
        let mut loader = config::loader::ConfigLoader::new().discover();
        match loader.load_sync() {
            Ok(config) => {
                let bus_config = config.get("bus");
                Self {
                    mode: bus_config
                        .and_then(|c| c.get("mode"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("peer")
                        .to_string(),
                    connect: bus_config
                        .and_then(|c| c.get("connect"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    listen: bus_config
                        .and_then(|c| c.get("listen"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    peer: bus_config
                        .and_then(|c| c.get("peer"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                }
            }
            Err(_) => Self {
                mode: "peer".to_string(),
                connect: None,
                listen: None,
                peer: None,
            },
        }
    }
}

impl From<PyBusConfig> for BusConfig {
    fn from(value: PyBusConfig) -> Self {
        Self {
            mode: value.mode,
            connect: value.connect,
            listen: value.listen,
            peer: value.peer.map(Some),
        }
    }
}

#[pymethods]
impl PyBusConfig {
    #[new]
    #[pyo3(signature = (mode = "peer".to_string(), connect = None, listen = None, peer = None))]
    fn new(
        mode: String,
        connect: Option<Vec<String>>,
        listen: Option<Vec<String>>,
        peer: Option<String>,
    ) -> Self {
        Self {
            mode,
            connect,
            listen,
            peer,
        }
    }
}

#[pyclass(name = "Bus", skip_from_py_object)]
#[derive(Clone)]
pub struct PyBus {
    pub inner: Arc<tokio::sync::Mutex<Bus>>,
}

#[pymethods]
impl PyBus {
    #[classmethod]
    #[pyo3(signature = (config = None))]
    fn create<'py>(
        _cls: &Bound<'py, PyType>,
        py: Python<'py>,
        config: Option<PyRef<'py, PyBusConfig>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let cfg: BusConfig = config
            .as_ref()
            .map(|c| (*c).clone())
            .unwrap_or_default()
            .into();

        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let bus = Bus::from(cfg).await;
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_bus = Py::new(
                    py,
                    PyBus {
                        inner: Arc::new(tokio::sync::Mutex::new(bus)),
                    },
                )?;
                Ok(py_bus.into_any())
            })
        })
    }

    fn publish_text<'py>(
        &self,
        py: Python<'py>,
        topic: String,
        payload: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = bus.lock().await;
            guard
                .publish(&topic, &payload)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }

    fn publish_json<'py>(
        &self,
        py: Python<'py>,
        topic: String,
        data: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let json_value = crate::utils::py_to_json(data)?;
        let json_str = json_value.to_string();
        let bus = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = bus.lock().await;
            guard
                .publish(&topic, &json_str)
                .await
                .map_err(crate::utils::to_py_runtime_error)?;
            Ok(())
        })
    }
}
