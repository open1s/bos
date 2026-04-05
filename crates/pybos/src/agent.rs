use crate::bus::PyBus;
use crate::utils::{invoke_python_handler_to_pyany, json_to_py, to_py_runtime_error};
use agent::{
    Agent, AgentCallableServer, AgentConfig, AgentRpcClient, StreamToken, Tool, ToolDescription,
};
use async_trait::async_trait;
use futures::StreamExt;
use pyo3::prelude::*;
use pyo3::types::PyType;
use pyo3::IntoPyObjectExt;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Async iterator that yields tokens from the agent stream.
/// Implements Python's async iteration protocol (__anext__).
#[pyclass(name = "StreamIterator", skip_from_py_object)]
pub struct PyStreamIterator {
    inner: Arc<tokio::sync::Mutex<Option<mpsc::Receiver<Result<String, String>>>>>,
}

#[pymethods]
impl PyStreamIterator {
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
                    Some(Ok(text)) => Ok(text),
                    Some(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e)),
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

/// A Python tool that wraps a Python callback function
#[pyclass(name = "PythonTool", skip_from_py_object)]
pub struct PyPythonTool {
    name: String,
    description: ToolDescription,
    schema: serde_json::Value,
    callback: Py<PyAny>,
}

#[pymethods]
impl PyPythonTool {
    #[new]
    fn new(
        name: String,
        description: String,
        parameters: String,
        schema: String,
        callback: Py<PyAny>,
    ) -> Self {
        Self {
            name,
            description: ToolDescription {
                short: description,
                parameters,
            },
            schema: serde_json::from_str(&schema).unwrap_or(serde_json::Value::Null),
            callback,
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl Tool for PyPythonTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> ToolDescription {
        self.description.clone()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, agent::ToolError> {
        let py_arg = Python::attach(|py| -> Result<Py<PyAny>, agent::ToolError> {
            let arg_json = serde_json::to_string(args)
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;
            Ok(arg_json
                .into_bound_py_any(py)
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?
                .unbind())
        })?;

        let result = invoke_python_handler_to_pyany(&self.callback, py_arg)
            .await
            .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;

        // Convert the result back to serde_json::Value
        let result_json = Python::attach(|py| -> Result<serde_json::Value, agent::ToolError> {
            let result_str = result
                .bind(py)
                .extract::<String>()
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;
            serde_json::from_str(&result_str)
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))
        })?;

        Ok(result_json)
    }
}

/// A wrapper for PyPythonTool that can be used as a Tool
/// Caches name/description/schema at construction to avoid Python GIL borrowing in sync trait methods
struct PyPythonToolWrapper {
    inner: Py<PyPythonTool>,
    name: String,
    description: ToolDescription,
    schema: serde_json::Value,
}

#[async_trait]
impl Tool for PyPythonToolWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> ToolDescription {
        self.description.clone()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, agent::ToolError> {
        let args_clone = args.clone();
        let py_arg = Python::attach(|py| -> Result<Py<PyAny>, agent::ToolError> {
            let inner = self.inner.clone_ref(py);
            let tool = inner.borrow(py);
            let arg_json = serde_json::to_string(&args_clone)
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;
            // Parse JSON string into Python dict
            let json_mod = py
                .import("json")
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;
            let args_dict = json_mod
                .call_method1("loads", (arg_json,))
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;
            tool.callback
                .clone_ref(py)
                .call1(py, (args_dict,))
                .map_err(|e: PyErr| agent::ToolError::ExecutionFailed(e.to_string()))
        })?;

        Python::attach(|py| -> Result<serde_json::Value, agent::ToolError> {
            let result_str = py_arg
                .bind(py)
                .extract::<String>()
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;
            serde_json::from_str(&result_str)
                .map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))
        })
    }
}

#[pyclass(name = "AgentConfig", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgentConfig {
    #[pyo3(get, set)]
    pub name: String,
    #[pyo3(get, set)]
    pub model: String,
    #[pyo3(get, set)]
    pub base_url: String,
    #[pyo3(get, set)]
    pub api_key: String,
    #[pyo3(get, set)]
    pub system_prompt: String,
    #[pyo3(get, set)]
    pub temperature: f32,
    #[pyo3(get, set)]
    pub max_tokens: Option<u32>,
    #[pyo3(get, set)]
    pub timeout_secs: u64,
    #[pyo3(get, set)]
    pub max_steps: usize,
    #[pyo3(get, set)]
    pub context_compaction_threshold_tokens: usize,
    #[pyo3(get, set)]
    pub context_compaction_trigger_ratio: f32,
    #[pyo3(get, set)]
    pub context_compaction_keep_recent_messages: usize,
    #[pyo3(get, set)]
    pub context_compaction_max_summary_chars: usize,
    #[pyo3(get, set)]
    pub context_compaction_summary_max_tokens: u32,
}

impl Default for PyAgentConfig {
    fn default() -> Self {
        let c = AgentConfig::default();
        Self {
            name: c.name,
            model: c.model,
            base_url: c.base_url,
            api_key: c.api_key,
            system_prompt: c.system_prompt,
            temperature: c.temperature,
            max_tokens: c.max_tokens,
            timeout_secs: c.timeout_secs,
            max_steps: 10,
            context_compaction_threshold_tokens: c.context_compaction_threshold_tokens,
            context_compaction_trigger_ratio: c.context_compaction_trigger_ratio,
            context_compaction_keep_recent_messages: c.context_compaction_keep_recent_messages,
            context_compaction_max_summary_chars: c.context_compaction_max_summary_chars,
            context_compaction_summary_max_tokens: c.context_compaction_summary_max_tokens,
        }
    }
}

impl From<PyAgentConfig> for AgentConfig {
    fn from(value: PyAgentConfig) -> Self {
        Self {
            name: value.name,
            model: value.model,
            base_url: value.base_url,
            api_key: value.api_key,
            system_prompt: value.system_prompt,
            temperature: value.temperature,
            max_tokens: value.max_tokens,
            timeout_secs: value.timeout_secs,
            max_steps: value.max_steps,
            rate_limit: None,
            context_compaction_threshold_tokens: value.context_compaction_threshold_tokens,
            context_compaction_trigger_ratio: value.context_compaction_trigger_ratio,
            context_compaction_keep_recent_messages: value.context_compaction_keep_recent_messages,
            context_compaction_max_summary_chars: value.context_compaction_max_summary_chars,
            context_compaction_summary_max_tokens: value.context_compaction_summary_max_tokens,
        }
    }
}

#[pymethods]
impl PyAgentConfig {
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        name = None,
        model = None,
        base_url = None,
        api_key = None,
        system_prompt = None,
        temperature = None,
        max_tokens = None,
        timeout_secs = None,
        context_compaction_threshold_tokens = None,
        context_compaction_trigger_ratio = None,
        context_compaction_keep_recent_messages = None,
        context_compaction_max_summary_chars = None,
        context_compaction_summary_max_tokens = None
    ))]
    fn new(
        name: Option<String>,
        model: Option<String>,
        base_url: Option<String>,
        api_key: Option<String>,
        system_prompt: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        timeout_secs: Option<u64>,
        context_compaction_threshold_tokens: Option<usize>,
        context_compaction_trigger_ratio: Option<f32>,
        context_compaction_keep_recent_messages: Option<usize>,
        context_compaction_max_summary_chars: Option<usize>,
        context_compaction_summary_max_tokens: Option<u32>,
    ) -> Self {
        let mut cfg = Self::default();
        if let Some(v) = name {
            cfg.name = v;
        }
        if let Some(v) = model {
            cfg.model = v;
        }
        if let Some(v) = base_url {
            cfg.base_url = v;
        }
        if let Some(v) = api_key {
            cfg.api_key = v;
        }
        if let Some(v) = system_prompt {
            cfg.system_prompt = v;
        }
        if let Some(v) = temperature {
            cfg.temperature = v;
        }
        if let Some(v) = max_tokens {
            cfg.max_tokens = Some(v);
        }
        if let Some(v) = timeout_secs {
            cfg.timeout_secs = v;
        }
        if let Some(v) = context_compaction_threshold_tokens {
            cfg.context_compaction_threshold_tokens = v;
        }
        if let Some(v) = context_compaction_trigger_ratio {
            cfg.context_compaction_trigger_ratio = v;
        }
        if let Some(v) = context_compaction_keep_recent_messages {
            cfg.context_compaction_keep_recent_messages = v;
        }
        if let Some(v) = context_compaction_max_summary_chars {
            cfg.context_compaction_max_summary_chars = v;
        }
        if let Some(v) = context_compaction_summary_max_tokens {
            cfg.context_compaction_summary_max_tokens = v;
        }
        cfg
    }
}

#[pyclass(name = "AgentRpcClient", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgentRpcClient {
    pub inner: Arc<Mutex<AgentRpcClient>>,
}

#[pymethods]
impl PyAgentRpcClient {
    fn endpoint(&self) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Client lock poisoned"))?;
        Ok(guard.endpoint().to_string())
    }

    fn list<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let rpc = {
                let guard = client.lock().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Client lock poisoned")
                })?;
                guard.clone()
            };
            let result = rpc.list().await.map_err(to_py_runtime_error)?;
            Python::attach(|py| json_to_py(py, &result))
        })
    }

    fn call<'py>(
        &self,
        py: Python<'py>,
        tool_name: String,
        args_json: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let args: serde_json::Value = serde_json::from_str(&args_json)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let rpc = {
                let guard = client.lock().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Client lock poisoned")
                })?;
                guard.clone()
            };
            let result = rpc
                .call(&tool_name, args)
                .await
                .map_err(to_py_runtime_error)?;
            Python::attach(|py| json_to_py(py, &result))
        })
    }

    fn llm_run<'py>(&self, py: Python<'py>, task: String) -> PyResult<Bound<'py, PyAny>> {
        let client = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let rpc = {
                let guard = client.lock().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Client lock poisoned")
                })?;
                guard.clone()
            };
            let result = rpc.llm_run(&task).await.map_err(to_py_runtime_error)?;
            Python::attach(|py| json_to_py(py, &result))
        })
    }
}

#[pyclass(name = "AgentCallableServer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgentCallableServer {
    pub inner: Arc<Mutex<AgentCallableServer>>,
}

#[pymethods]
impl PyAgentCallableServer {
    fn endpoint(&self) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Server lock poisoned"))?;
        Ok(guard.endpoint().to_string())
    }

    fn is_started(&self) -> PyResult<bool> {
        // CallableServer auto-starts on creation; always true after construction
        Ok(true)
    }

    // Note: start() is called automatically when the server is created
    // The server starts listening for requests immediately
}

#[pyclass(name = "Agent", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgent {
    pub inner: std::sync::Arc<Mutex<Agent>>,
}

#[pymethods]
impl PyAgent {
    #[classmethod]
    fn from_config<'py>(
        _cls: &Bound<'py, PyType>,
        _py: Python<'py>,
        config: PyRef<'py, PyAgentConfig>,
    ) -> PyResult<Self> {
        let cfg: AgentConfig = config.clone().into();
        let agent = Agent::builder()
            .name(cfg.name)
            .model(cfg.model)
            .base_url(cfg.base_url)
            .api_key(cfg.api_key)
            .system_prompt(cfg.system_prompt)
            .temperature(cfg.temperature)
            .max_tokens(cfg.max_tokens.unwrap_or(4096))
            .timeout(cfg.timeout_secs)
            .context_compaction_threshold_tokens(cfg.context_compaction_threshold_tokens)
            .context_compaction_trigger_ratio(cfg.context_compaction_trigger_ratio)
            .context_compaction_keep_recent_messages(cfg.context_compaction_keep_recent_messages)
            .context_compaction_max_summary_chars(cfg.context_compaction_max_summary_chars)
            .context_compaction_summary_max_tokens(cfg.context_compaction_summary_max_tokens)
            .build()
            .map_err(to_py_runtime_error)?;

        Ok(Self {
            inner: std::sync::Arc::new(Mutex::new(agent)),
        })
    }

    #[classmethod]
    fn create<'py>(
        _cls: &Bound<'py, PyType>,
        py: Python<'py>,
        config: PyRef<'py, PyAgentConfig>,
        _bus: PyRef<'py, PyBus>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Create agent from config (bus parameter is currently unused but kept for API compatibility)
        let cfg: AgentConfig = config.clone().into();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let agent = Agent::builder()
                .name(cfg.name)
                .model(cfg.model)
                .base_url(cfg.base_url)
                .api_key(cfg.api_key)
                .system_prompt(cfg.system_prompt)
                .temperature(cfg.temperature)
                .max_tokens(cfg.max_tokens.unwrap_or(4096))
                .timeout(cfg.timeout_secs)
                .context_compaction_threshold_tokens(cfg.context_compaction_threshold_tokens)
                .context_compaction_trigger_ratio(cfg.context_compaction_trigger_ratio)
                .context_compaction_keep_recent_messages(
                    cfg.context_compaction_keep_recent_messages,
                )
                .context_compaction_max_summary_chars(cfg.context_compaction_max_summary_chars)
                .context_compaction_summary_max_tokens(cfg.context_compaction_summary_max_tokens)
                .build()
                .map_err(to_py_runtime_error)?;

            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_agent = Py::new(
                    py,
                    PyAgent {
                        inner: std::sync::Arc::new(Mutex::new(agent)),
                    },
                )?;
                Ok(py_agent.into_any())
            })
        })
    }

    fn add_remote_agent_tool<'py>(
        &self,
        py: Python<'py>,
        tool_name: String,
        endpoint: String,
        bus: PyRef<'py, PyBus>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus_inner = bus.inner.clone();
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let session = {
                let guard = bus_inner.lock().await;
                let bus_copy = guard.clone();
                std::sync::Arc::<bus::Session>::from(bus_copy)
            };

            let mut guard = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
            guard
                .add_remote_agent_tool(tool_name, endpoint, session)
                .map_err(to_py_runtime_error)?;
            Ok(())
        })
    }

    fn add_python_tool<'py>(
        &self,
        py: Python<'py>,
        tool_name: String,
        server_endpoint: String,
        bus: PyRef<'py, PyBus>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Convenience method for adding tools from PythonToolServer
        // server_endpoint should be "zenoh://server_name" format
        // Tool will be accessed at "{server_name}/{tool_name}"
        let bus_inner = bus.inner.clone();
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let session = {
                let guard = bus_inner.lock().await;
                let bus_copy = guard.clone();
                std::sync::Arc::<bus::Session>::from(bus_copy)
            };

            let mut guard = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;

            // Extract server name from endpoint (e.g., "zenoh://python_agent" -> "python_agent/tool_name")
            let endpoint_path = if server_endpoint.starts_with("zenoh/") {
                let server_name = server_endpoint
                    .strip_prefix("zenoh/")
                    .unwrap_or("python_agent");
                format!("{}/{}", server_name, tool_name)
            } else {
                format!("{}/{}", server_endpoint, tool_name)
            };

            guard
                .add_remote_agent_tool(tool_name, endpoint_path, session)
                .map_err(to_py_runtime_error)?;
            Ok(())
        })
    }

    fn react<'py>(&self, py: Python<'py>, task: String) -> PyResult<Bound<'py, PyAny>> {
        let agent = {
            let guard = self
                .inner
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
            guard.clone()
        };
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let out = agent.react(&task).await.map_err(to_py_runtime_error)?;
            Ok(out)
        })
    }

    fn run_simple<'py>(&self, py: Python<'py>, task: String) -> PyResult<Bound<'py, PyAny>> {
        let agent = {
            let guard = self
                .inner
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
            guard.clone()
        };
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let out = agent.run_simple(&task).await.map_err(to_py_runtime_error)?;
            Ok(out)
        })
    }

    fn stream<'py>(&self, py: Python<'py>, task: String) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;

        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let agent_clone = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?
                .clone();

            let (tx, rx) = mpsc::channel::<Result<String, String>>(32);

            tokio::spawn(async move {
                let mut stream = agent_clone.stream(&task);
                while let Some(token_result) = stream.next().await {
                    let item = match token_result {
                        Ok(StreamToken::Text(text)) => Ok(text),
                        Ok(StreamToken::ToolCall { name, args, id }) => Ok(serde_json::json!({
                            "type": "tool_call",
                            "name": name,
                            "args": args,
                            "id": id
                        })
                        .to_string()),
                        Ok(StreamToken::Done) => break,
                        Err(e) => Err(e.to_string()),
                    };
                    if tx.send(item).await.is_err() {
                        break;
                    }
                }
            });

            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let iter = PyStreamIterator {
                    inner: Arc::new(tokio::sync::Mutex::new(Some(rx))),
                };
                Ok(Py::new(py, iter)?.into_any())
            })
        })
    }

    fn config(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        let cfg = guard.config();
        let mut map = BTreeMap::<String, serde_json::Value>::new();
        map.insert(
            "name".to_string(),
            serde_json::Value::String(cfg.name.clone()),
        );
        map.insert(
            "model".to_string(),
            serde_json::Value::String(cfg.model.clone()),
        );
        map.insert(
            "base_url".to_string(),
            serde_json::Value::String(cfg.base_url.clone()),
        );
        map.insert(
            "api_key".to_string(),
            serde_json::Value::String(cfg.api_key.clone()),
        );
        map.insert(
            "system_prompt".to_string(),
            serde_json::Value::String(cfg.system_prompt.clone()),
        );
        map.insert(
            "temperature".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(cfg.temperature as f64).ok_or_else(|| {
                    pyo3::exceptions::PyRuntimeError::new_err(
                        "Unable to convert temperature to JSON",
                    )
                })?,
            ),
        );
        map.insert(
            "max_tokens".to_string(),
            cfg.max_tokens
                .map(|v| serde_json::Value::Number(v.into()))
                .unwrap_or(serde_json::Value::Null),
        );
        map.insert(
            "timeout_secs".to_string(),
            serde_json::Value::Number(cfg.timeout_secs.into()),
        );

        json_to_py(py, &serde_json::to_value(map).map_err(to_py_runtime_error)?)
    }

    fn rpc_client<'py>(
        &self,
        py: Python<'py>,
        endpoint: String,
        bus: PyRef<'py, PyBus>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus_inner = bus.inner.clone();
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let session = {
                let guard = bus_inner.lock().await;
                let bus_copy = guard.clone();
                std::sync::Arc::<bus::Session>::from(bus_copy)
            };
            let client = {
                let guard = agent.lock().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned")
                })?;
                guard.rpc_client(endpoint, session)
            };
            Ok(PyAgentRpcClient {
                inner: Arc::new(Mutex::new(client)),
            })
        })
    }

    fn as_callable_server<'py>(
        &self,
        py: Python<'py>,
        endpoint: String,
        bus: PyRef<'py, PyBus>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let bus_inner = bus.inner.clone();
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let session = {
                let guard = bus_inner.lock().await;
                let bus_copy = guard.clone();
                std::sync::Arc::<bus::Session>::from(bus_copy)
            };
            // Create the server in a nested scope so the guard is dropped before await
            let mut server = {
                let guard = agent.lock().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned")
                })?;
                guard.as_callable_server(endpoint, session)
            }; // guard is dropped here
               // Start the server
            if let Err(e) = server.start().await {
                return Err(to_py_runtime_error(e));
            }
            Ok(PyAgentCallableServer {
                inner: Arc::new(Mutex::new(server)),
            })
        })
    }

    fn list_tools(&self) -> PyResult<Vec<String>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        if let Some(registry) = guard.registry() {
            let tools: Vec<String> = registry.iter().map(|(name, _)| name.clone()).collect();
            Ok(tools)
        } else {
            Ok(Vec::new())
        }
    }

    fn register_skills_from_dir<'py>(
        &self,
        py: Python<'py>,
        dir_path: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let mut guard = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
            guard
                .register_skills_from_dir(std::path::PathBuf::from(dir_path))
                .map_err(to_py_runtime_error)?;
            Ok(())
        })
    }

    fn add_tool<'py>(
        &self,
        py: Python<'py>,
        tool: Py<PyPythonTool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let py_tool = tool.borrow(py);
        let tool_name = py_tool.name().to_string();
        let tool_description = py_tool.description();
        let tool_schema = py_tool.json_schema();
        drop(py_tool);

        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        let tool_name_for_return = tool_name.clone();
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let tool_box: Arc<dyn Tool> = Arc::new(PyPythonToolWrapper {
                inner: tool,
                name: tool_name,
                description: tool_description,
                schema: tool_schema,
            });

            let mut guard = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
            guard.add_tool(tool_box);
            Ok(tool_name_for_return)
        })
    }

    fn add_mcp_server<'py>(
        &self,
        py: Python<'py>,
        namespace: String,
        command: String,
        args: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let client = agent::McpClient::spawn(
                &command,
                args.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
            .await
            .map_err(to_py_runtime_error)?;
            let client = Arc::new(client);

            client.initialize().await.map_err(to_py_runtime_error)?;
            let mcp_tools = client.list_tools().await.map_err(to_py_runtime_error)?;

            {
                let mut guard = agent.lock().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned")
                })?;
                for tool_def in &mcp_tools {
                    let namespaced = format!("{}/{}", namespace, tool_def.name);
                    let adapter = agent::McpToolAdapter::new(
                        client.clone(),
                        namespaced,
                        tool_def.name.clone(),
                        tool_def.description.clone(),
                        tool_def.input_schema.clone(),
                    );
                    guard.add_tool(Arc::new(adapter));
                }
            }
            Ok(())
        })
    }

    fn add_mcp_server_http<'py>(
        &self,
        py: Python<'py>,
        namespace: String,
        url: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let client = agent::McpClient::connect_http(&url);
            let client = Arc::new(client);

            client.initialize().await.map_err(to_py_runtime_error)?;
            let mcp_tools = client.list_tools().await.map_err(to_py_runtime_error)?;

            {
                let mut guard = agent.lock().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned")
                })?;
                for tool_def in &mcp_tools {
                    let namespaced = format!("{}/{}", namespace, tool_def.name);
                    let adapter = agent::McpToolAdapter::new(
                        client.clone(),
                        namespaced,
                        tool_def.name.clone(),
                        tool_def.description.clone(),
                        tool_def.input_schema.clone(),
                    );
                    guard.add_tool(Arc::new(adapter));
                }
            }
            Ok(())
        })
    }

    fn list_mcp_tools<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let guard = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
            let tools = guard
                .registry()
                .map(|r| {
                    r.iter()
                        .filter(|(name, _)| name.contains('/'))
                        .map(|(name, tool)| {
                            serde_json::json!({
                                "name": name,
                                "description": tool.description().short,
                                "parameters": tool.description().parameters,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let tools_json = serde_json::to_value(&tools)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &tools_json))
        })
    }

    fn list_mcp_resources<'py>(
        &self,
        py: Python<'py>,
        namespace: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let guard = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;

            let tools = guard
                .registry()
                .map(|r| {
                    r.iter()
                        .filter(|(name, _)| name.starts_with(&format!("{}/", namespace)))
                        .map(|(name, tool)| {
                            serde_json::json!({
                                "name": name,
                                "description": tool.description().short,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let tools_json = serde_json::to_value(&tools)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Python::attach(|py| json_to_py(py, &tools_json))
        })
    }

    fn list_mcp_prompts<'py>(&self) -> PyResult<Vec<String>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        let prompts: Vec<String> = guard
            .registry()
            .map(|r| {
                r.iter()
                    .filter(|(name, _)| name.contains('/'))
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default();
        Ok(prompts)
    }
}
