//! Python bindings for MCP (Model Context Protocol) clients.
//!
//! Provides `PyMcpClient` — a Python-accessible wrapper around the Rust `McpClient`.

use agent::McpClient;
use pyo3::prelude::*;

use std::sync::Arc;

use crate::utils::{json_to_py, to_py_runtime_error};

/// A Python-accessible MCP client.
///
/// Usage:
/// ```python
/// client = await McpClient.spawn("npx", ["-y", "@some/mcp-server"])
/// caps = await client.initialize()
/// tools = await client.list_tools()
/// result = await client.call_tool("echo", {"text": "hello"})
/// ```
#[pyclass(name = "McpClient", skip_from_py_object)]
pub struct PyMcpClient {
    inner: Arc<McpClient>,
}

#[pymethods]
impl PyMcpClient {
    /// Spawn a new MCP server process.
    ///
    /// Args:
    ///     command: The command to run (e.g. "npx", "python3")
    ///     args: Arguments to pass to the command
    ///
    /// Returns:
    ///     McpClient instance (not yet initialized)
    #[staticmethod]
    fn spawn<'py>(
        py: Python<'py>,
        command: String,
        args: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let client = McpClient::spawn(&command, &arg_refs)
                .await
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| {
                Ok(Py::new(
                    py,
                    PyMcpClient {
                        inner: Arc::new(client),
                    },
                )?
                .into_any())
            })
        })
    }

    /// Connect to an MCP server via HTTP (Streamable HTTP transport).
    ///
    /// Args:
    ///     url: The MCP server endpoint URL (e.g. "http://localhost:8080/mcp")
    ///
    /// Returns:
    ///     McpClient instance (not yet initialized)
    #[staticmethod]
    fn connect_http(url: String) -> PyResult<Self> {
        let client = McpClient::connect_http(url);
        Ok(PyMcpClient {
            inner: Arc::new(client),
        })
    }

    /// Initialize the MCP connection.
    ///
    /// Returns:
    ///     dict with server capabilities (tools, resources, prompts)
    fn initialize<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let caps = client.initialize().await.map_err(to_py_runtime_error)?;

            let caps_json = serde_json::to_value(&caps)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &caps_json))
        })
    }

    /// List available tools from the MCP server.
    ///
    /// Returns:
    ///     List of dicts, each with keys: name, description, input_schema
    fn list_tools<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let tools = client.list_tools().await.map_err(to_py_runtime_error)?;

            let tools_json: serde_json::Value = serde_json::to_value(&tools)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &tools_json))
        })
    }

    /// Call an MCP tool by name with the given arguments.
    ///
    /// Args:
    ///     name: Tool name
    ///     arguments: JSON-serializable dict of arguments
    ///
    /// Returns:
    ///     Tool result as a JSON-serializable dict
    fn call_tool<'py>(
        &self,
        py: Python<'py>,
        name: String,
        arguments: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let args: serde_json::Value = serde_json::from_str(&arguments)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let result = client
                .call_tool(&name, args)
                .await
                .map_err(to_py_runtime_error)?;

            let result_json = serde_json::to_value(&result)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &result_json))
        })
    }

    /// List available resources from the MCP server.
    ///
    /// Returns:
    ///     List of dicts, each with keys: uri, name, description, mime_type
    fn list_resources<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let resources = client.list_resources().await.map_err(to_py_runtime_error)?;

            let resources_json: serde_json::Value = serde_json::to_value(&resources)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &resources_json))
        })
    }

    /// Read a resource by URI.
    ///
    /// Args:
    ///     uri: Resource URI
    ///
    /// Returns:
    ///     dict with key "contents" — list of {uri, mime_type, text}
    fn read_resource<'py>(&self, py: Python<'py>, uri: String) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let result = client
                .read_resource(&uri)
                .await
                .map_err(to_py_runtime_error)?;

            let result_json: serde_json::Value = serde_json::to_value(&result)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &result_json))
        })
    }

    /// List available prompts from the MCP server.
    ///
    /// Returns:
    ///     List of dicts, each with keys: name, description, arguments
    fn list_prompts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let prompts = client.list_prompts().await;

            let prompts_json: serde_json::Value = serde_json::to_value(&prompts)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &prompts_json))
        })
    }
}
