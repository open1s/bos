//! Python bindings for the Ferris SDK authoring surface.
//!
//! A Python object can implement the SDK's Extension trait: it declares a
//! schema-2.0 manifest and supplies start, invoke and optional shutdown
//! callbacks. FerrisHarness adapts those callbacks to the Rust
//! ExtensionRuntime, so an extension written in Python starts and answers calls
//! exactly like one written in Rust.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pyo3::prelude::*;

use ferris_sdk::{
    Call, Contribution, Extension, ExtensionContext, ExtensionManifest, ExtensionRuntime,
    InvokeContext, Runtime,
};

use crate::utils::{
    await_python_coroutine, call_python_callback_maybe_async, json_to_py, py_to_json,
};

fn runtime_error(error: ferris_sdk::ExtError) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
}

pub(crate) async fn call_python(
    callback: &Py<PyAny>,
    payload: serde_json::Value,
) -> PyResult<serde_json::Value> {
    let argument = Python::attach(|py| json_to_py(py, &payload))?;
    let result = call_python_callback_maybe_async(callback, argument).await?;
    Python::attach(|py| py_to_json(result.bind(py)))
}

pub(crate) async fn call_python_noarg(callback: &Py<PyAny>) -> PyResult<()> {
    let (result, is_coroutine) = Python::attach(|py| -> PyResult<(Py<PyAny>, bool)> {
        let result = callback.bind(py).call0()?;
        let is_coroutine = result.hasattr("__await__").unwrap_or(false);
        Ok((result.unbind(), is_coroutine))
    })?;
    if is_coroutine {
        await_python_coroutine(result).await?;
    }
    Ok(())
}

/// One extension implemented by Python callbacks.
pub(crate) struct PyForeignExtension {
    pub(crate) manifest: ExtensionManifest,
    pub(crate) start: Py<PyAny>,
    pub(crate) invoke: Py<PyAny>,
    pub(crate) shutdown: Option<Py<PyAny>>,
}

#[async_trait]
impl Extension for PyForeignExtension {
    fn manifest(&self) -> ExtensionManifest {
        self.manifest.clone()
    }

    async fn start(&self, ctx: &ExtensionContext) -> ferris_sdk::Result<Vec<Contribution>> {
        let payload = serde_json::json!({
            "id": ctx.id(),
            "root": ctx.root().to_string_lossy(),
            "data_dir": ctx.data_dir().to_string_lossy(),
            "runtime_kind": ctx.manifest.runtime_kind(),
        });
        let value = call_python(&self.start, payload)
            .await
            .map_err(|error| ferris_sdk::error::runtime(error.to_string()))?;
        serde_json::from_value(value)
            .map_err(|error| ferris_sdk::error::invalid_input(error.to_string()))
    }

    async fn invoke(
        &self,
        call: &Call,
        ctx: &InvokeContext,
    ) -> ferris_sdk::Result<serde_json::Value> {
        let payload = serde_json::json!({
            "contribution": { "kind": call.kind(), "id": call.id() },
            "input": call.input,
            "context": { "correlation": ctx.correlation, "timeout_ms": ctx.timeout_ms },
        });
        call_python(&self.invoke, payload)
            .await
            .map_err(|error| ferris_sdk::error::runtime(error.to_string()))
    }

    async fn shutdown(&self) -> ferris_sdk::Result<()> {
        if let Some(callback) = &self.shutdown {
            call_python_noarg(callback)
                .await
                .map_err(|error| ferris_sdk::error::runtime(error.to_string()))?;
        }
        Ok(())
    }
}

/// A parsed, validated schema-2.0 extension manifest.
#[pyclass(name = "FerrisManifest", skip_from_py_object)]
#[derive(Clone)]
pub struct PyFerrisManifest {
    inner: ExtensionManifest,
}

impl PyFerrisManifest {
    pub(crate) fn from_manifest(inner: ExtensionManifest) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyFerrisManifest {
    /// Parse and validate an extension.toml document.
    #[staticmethod]
    fn parse(text: String) -> PyResult<Self> {
        let inner = ExtensionManifest::parse(&text).map_err(runtime_error)?;
        Ok(Self { inner })
    }

    /// The canonical TOML document.
    fn to_toml(&self) -> String {
        self.inner.to_toml().to_string()
    }

    /// The manifest as a plain dictionary.
    fn to_json(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let value = serde_json::to_value(self.inner.as_v2())
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_py(py, &value)
    }

    #[getter]
    /// The extension id.
    fn id(&self) -> String {
        self.inner.id().as_str().to_string()
    }

    #[getter]
    /// The runtime kind the manifest asks for.
    fn runtime_kind(&self) -> String {
        self.inner.runtime_kind().to_string()
    }

    fn __repr__(&self) -> String {
        format!(
            "FerrisManifest(id={:?}, runtime_kind={:?})",
            self.id(),
            self.runtime_kind()
        )
    }
}

/// An in-process Ferris extension harness.
///
/// This is the Python equivalent of the SDK's TestHarness: it starts the
/// extension, returns its contributions, and forwards invoke calls. It does not
/// install or activate packages; that is the host runtime surface.
#[pyclass(name = "FerrisHarness", skip_from_py_object)]
pub struct PyFerrisHarness {
    inner: Arc<ExtensionRuntime<PyForeignExtension>>,
}

#[pymethods]
impl PyFerrisHarness {
    #[new]
    #[pyo3(signature = (manifest, root, start, invoke, shutdown = None))]
    /// Build a harness around Python extension callbacks.
    ///
    /// start(ctx) receives the start context and returns a list of
    /// {kind, id, payload} contributions. invoke(call) receives
    /// {contribution, input, context} and returns the call result.
    fn new(
        manifest: String,
        root: String,
        start: Py<PyAny>,
        invoke: Py<PyAny>,
        shutdown: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let manifest = ExtensionManifest::parse(&manifest).map_err(runtime_error)?;
        let extension = PyForeignExtension {
            manifest,
            start,
            invoke,
            shutdown,
        };
        let runtime =
            ExtensionRuntime::new(extension, PathBuf::from(root)).map_err(runtime_error)?;
        Ok(Self {
            inner: Arc::new(runtime),
        })
    }

    /// The parsed manifest.
    fn manifest(&self, py: Python<'_>) -> PyResult<Py<PyFerrisManifest>> {
        Py::new(
            py,
            PyFerrisManifest {
                inner: self.inner.manifest().clone(),
            },
        )
    }

    /// Start the extension and return its contributions.
    fn start<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let contributions = inner.start().await.map_err(runtime_error)?;
            let value = serde_json::to_value(contributions)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Invoke a contribution id as a tool.
    fn invoke<'py>(
        &self,
        py: Python<'py>,
        id: String,
        input: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input = py_to_json(input)?;
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let value = inner.invoke_id(&id, input).await.map_err(runtime_error)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Invoke a contribution with an explicit kind.
    fn invoke_contribution<'py>(
        &self,
        py: Python<'py>,
        kind: String,
        id: String,
        input: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input = py_to_json(input)?;
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let value = inner
                .invoke_contribution(&kind, &id, input)
                .await
                .map_err(runtime_error)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Shut the extension down.
    fn shutdown<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            inner.shutdown().await.map_err(runtime_error)
        })
    }

    #[getter]
    /// The runtime kind the manifest asks for.
    fn runtime_kind(&self) -> String {
        self.inner.manifest().runtime_kind().to_string()
    }

    #[getter]
    /// The package root handed to the extension.
    fn root(&self) -> String {
        self.inner.root().to_string_lossy().to_string()
    }
}

/// Convert a JSON value into a TOML value for runtime configuration and
/// contribution payloads. JSON null has no TOML representation.
fn json_to_toml(value: &serde_json::Value) -> PyResult<ferris_sdk::toml::Value> {
    ferris_bindings::json_to_toml(value).map_err(pyo3::exceptions::PyValueError::new_err)
}

fn startup_policy(raw: &str) -> PyResult<ferris_sdk::StartupPolicy> {
    ferris_bindings::startup_policy(raw).map_err(pyo3::exceptions::PyValueError::new_err)
}

/// A chainable builder for a schema-2.0 manifest.
#[pyclass(name = "FerrisManifestBuilder", skip_from_py_object)]
#[derive(Clone)]
pub struct PyFerrisManifestBuilder {
    inner: ferris_sdk::ManifestBuilder,
}

#[pymethods]
impl PyFerrisManifestBuilder {
    #[new]
    /// Start a manifest with the required identity fields.
    fn new(id: String, name: String, version: String, description: String) -> Self {
        Self {
            inner: ferris_sdk::ManifestBuilder::new(id, name, version, description),
        }
    }

    /// Set the host version range.
    fn host(&self, requirement: String) -> Self {
        Self {
            inner: self.inner.clone().host(requirement),
        }
    }

    /// Set the extension API range.
    fn extension_api(&self, requirement: String) -> Self {
        Self {
            inner: self.inner.clone().extension_api(requirement),
        }
    }

    /// Set the runtime kind.
    fn runtime(&self, kind: String) -> Self {
        Self {
            inner: self.inner.clone().runtime(kind),
        }
    }

    /// Set one opaque runtime configuration value.
    fn runtime_config(&self, key: String, value: &Bound<'_, PyAny>) -> PyResult<Self> {
        let value = json_to_toml(&py_to_json(value)?)?;
        Ok(Self {
            inner: self.inner.clone().runtime_config(key, value),
        })
    }

    /// Require one capability.
    fn capability(&self, capability: String) -> Self {
        Self {
            inner: self.inner.clone().capability(capability),
        }
    }

    /// Declare one contribution with an optional payload.
    #[pyo3(signature = (kind, id, payload = None))]
    fn contribution(
        &self,
        kind: String,
        id: String,
        payload: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let mut spec = ferris_sdk::ContributionSpec::new(kind, id);
        if let Some(payload) = payload {
            spec = spec.payload(json_to_toml(&py_to_json(payload)?)?);
        }
        Ok(Self {
            inner: self.inner.clone().contribution(spec),
        })
    }

    /// Add one author.
    fn author(&self, author: String) -> Self {
        Self {
            inner: self.inner.clone().author(author),
        }
    }

    /// Set the SPDX license expression.
    fn license(&self, license: String) -> Self {
        Self {
            inner: self.inner.clone().license(license),
        }
    }

    /// Set the source repository URL.
    fn repository(&self, repository: String) -> Self {
        Self {
            inner: self.inner.clone().repository(repository),
        }
    }

    /// Set the project homepage URL.
    fn homepage(&self, homepage: String) -> Self {
        Self {
            inner: self.inner.clone().homepage(homepage),
        }
    }

    /// Add one dependency requirement.
    fn dependency(&self, id: String, requirement: String) -> Self {
        Self {
            inner: self.inner.clone().dependency(id, requirement),
        }
    }

    /// Declare one provided service from a dictionary.
    fn provide(&self, spec: &Bound<'_, PyAny>) -> PyResult<Self> {
        let spec: ferris_sdk::ServiceProvideSpec = serde_json::from_value(py_to_json(spec)?)
            .map_err(|error| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "invalid service provide spec: {error}"
                ))
            })?;
        Ok(Self {
            inner: self.inner.clone().provide(spec),
        })
    }

    /// Declare one required service from a dictionary.
    fn require(&self, spec: &Bound<'_, PyAny>) -> PyResult<Self> {
        let spec: ferris_sdk::ServiceRequireSpec = serde_json::from_value(py_to_json(spec)?)
            .map_err(|error| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "invalid service require spec: {error}"
                ))
            })?;
        Ok(Self {
            inner: self.inner.clone().require(spec),
        })
    }

    /// Set the memory ceiling in MiB.
    fn max_memory_mb(&self, value: u64) -> Self {
        Self {
            inner: self.inner.clone().max_memory_mb(value),
        }
    }

    /// Set the one-call wall-clock ceiling in milliseconds.
    fn max_execution_ms(&self, value: u64) -> Self {
        Self {
            inner: self.inner.clone().max_execution_ms(value),
        }
    }

    /// Set the concurrent-call ceiling.
    fn max_concurrent_calls(&self, value: u32) -> Self {
        Self {
            inner: self.inner.clone().max_concurrent_calls(value),
        }
    }

    /// Set the per-result output ceiling in bytes.
    fn max_output_bytes(&self, value: u64) -> Self {
        Self {
            inner: self.inner.clone().max_output_bytes(value),
        }
    }

    /// Set the WASM table element ceiling.
    fn max_table_elements(&self, value: u64) -> Self {
        Self {
            inner: self.inner.clone().max_table_elements(value),
        }
    }

    /// Set the package-relative configuration schema path.
    fn configuration_schema(&self, path: String) -> Self {
        Self {
            inner: self.inner.clone().configuration_schema(path),
        }
    }

    /// Set the default configuration values.
    fn configuration_defaults(&self, value: &Bound<'_, PyAny>) -> PyResult<Self> {
        let value = json_to_toml(&py_to_json(value)?)?;
        Ok(Self {
            inner: self.inner.clone().configuration_defaults(value),
        })
    }

    /// Set the startup policy: on-startup, on-demand, on-workspace-open, manual.
    fn startup(&self, policy: String) -> PyResult<Self> {
        Ok(Self {
            inner: self.inner.clone().startup(startup_policy(&policy)?),
        })
    }

    /// Validate and build the manifest.
    fn build(&self) -> PyResult<PyFerrisManifest> {
        let inner = self.inner.clone().build().map_err(runtime_error)?;
        Ok(PyFerrisManifest::from_manifest(inner))
    }

    fn __repr__(&self) -> String {
        "FerrisManifestBuilder(...)".to_string()
    }
}
