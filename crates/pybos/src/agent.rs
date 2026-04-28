use crate::bus::PyBus;
use crate::utils::{invoke_python_handler_to_pyany, json_to_py, py_to_json, to_py_runtime_error};
use agent::agent::hooks::HookEvent;
use agent::{
    Agent, AgentCallableServer, AgentConfig, AgentRpcClient, CircuitBreakerConfig, LlmMessage,
    RateLimiterConfig, StreamToken, Tool, ToolDescription,
};
use async_trait::async_trait;
use futures::StreamExt;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};
use pyo3::IntoPyObjectExt;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Python wrapper for LlmMessage
#[pyclass(name = "LlmMessage", skip_from_py_object)]
pub struct PyLlmMessage {
    inner: LlmMessage,
}

#[pymethods]
impl PyLlmMessage {
    #[staticmethod]
    fn system(content: String) -> Self {
        Self {
            inner: LlmMessage::system(content),
        }
    }

    #[staticmethod]
    fn user(content: String) -> Self {
        Self {
            inner: LlmMessage::user(content),
        }
    }

    #[staticmethod]
    fn assistant(content: String) -> Self {
        Self {
            inner: LlmMessage::assistant(content),
        }
    }

    fn to_py<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        match &self.inner {
            LlmMessage::System { content } => {
                let dict = PyDict::new(py);
                dict.set_item("role", "system")?;
                dict.set_item("content", content)?;
                Ok(dict.into_any())
            }
            LlmMessage::User { content } => {
                let dict = PyDict::new(py);
                dict.set_item("role", "user")?;
                dict.set_item("content", content)?;
                Ok(dict.into_any())
            }
            LlmMessage::Assistant { content } => {
                let dict = PyDict::new(py);
                dict.set_item("role", "assistant")?;
                dict.set_item("content", content)?;
                Ok(dict.into_any())
            }
            LlmMessage::AssistantToolCall {
                tool_call_id,
                name,
                args,
            } => {
                let dict = PyDict::new(py);
                dict.set_item("role", "assistant_tool_call")?;
                dict.set_item("tool_call_id", tool_call_id)?;
                dict.set_item("name", name)?;
                let args_py = json_to_py(py, args)?;
                dict.set_item("args", args_py)?;
                Ok(dict.into_any())
            }
            LlmMessage::ToolResult {
                tool_call_id,
                content,
            } => {
                let dict = PyDict::new(py);
                dict.set_item("role", "tool_result")?;
                dict.set_item("tool_call_id", tool_call_id)?;
                dict.set_item("content", content)?;
                Ok(dict.into_any())
            }
        }
    }

    #[staticmethod]
    fn from_py<'py>(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        let dict = obj.cast::<PyDict>()?;
        let role = dict
            .get_item("role")?
            .map(|v| v.extract::<String>())
            .transpose()?
            .unwrap_or_default();
        let content = dict
            .get_item("content")?
            .map(|v| v.extract::<String>())
            .transpose()?
            .unwrap_or_default();

        let inner = match role.as_str() {
            "system" => LlmMessage::system(content),
            "user" => LlmMessage::user(content),
            "assistant" => LlmMessage::assistant(content),
            "assistant_tool_call" => {
                let tool_call_id = dict
                    .get_item("tool_call_id")?
                    .map(|v| v.extract::<String>())
                    .transpose()?
                    .unwrap_or_default();
                let name = dict
                    .get_item("name")?
                    .map(|v| v.extract::<String>())
                    .transpose()?
                    .unwrap_or_default();
                let args = dict
                    .get_item("args")?
                    .map(|v| crate::utils::py_to_json(&v))
                    .transpose()?
                    .unwrap_or(serde_json::Value::Null);
                LlmMessage::AssistantToolCall {
                    tool_call_id,
                    name,
                    args,
                }
            }
            "tool_result" => {
                let tool_call_id = dict
                    .get_item("tool_call_id")?
                    .map(|v| v.extract::<String>())
                    .transpose()?
                    .unwrap_or_default();
                LlmMessage::ToolResult {
                    tool_call_id,
                    content,
                }
            }
            _ => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "Invalid role: {}. Expected one of: system, user, assistant, assistant_tool_call, tool_result",
                    role
                )));
            }
        };
        Ok(Self { inner })
    }
}

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
    pub circuit_breaker_max_failures: Option<usize>,
    #[pyo3(get, set)]
    pub circuit_breaker_cooldown_secs: Option<u64>,
    #[pyo3(get, set)]
    pub rate_limit_capacity: Option<u32>,
    #[pyo3(get, set)]
    pub rate_limit_window_secs: Option<u64>,
    #[pyo3(get, set)]
    pub rate_limit_max_retries: Option<u32>,
    #[pyo3(get, set)]
    pub rate_limit_retry_backoff_secs: Option<u64>,
    #[pyo3(get, set)]
    pub rate_limit_auto_wait: Option<bool>,
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
            circuit_breaker_max_failures: None,
            circuit_breaker_cooldown_secs: None,
            rate_limit_capacity: None,
            rate_limit_window_secs: None,
            rate_limit_max_retries: None,
            rate_limit_retry_backoff_secs: None,
            rate_limit_auto_wait: None,
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
        let circuit_breaker = if value.circuit_breaker_max_failures.is_some()
            || value.circuit_breaker_cooldown_secs.is_some()
        {
            Some(CircuitBreakerConfig {
                max_failures: value.circuit_breaker_max_failures.unwrap_or(5),
                cooldown: std::time::Duration::from_secs(
                    value.circuit_breaker_cooldown_secs.unwrap_or(30),
                ),
            })
        } else {
            None
        };

        let rate_limit = if value.rate_limit_capacity.is_some()
            || value.rate_limit_window_secs.is_some()
            || value.rate_limit_max_retries.is_some()
        {
            Some(RateLimiterConfig {
                capacity: value.rate_limit_capacity.unwrap_or(40),
                window: std::time::Duration::from_secs(value.rate_limit_window_secs.unwrap_or(60)),
                max_retries: value.rate_limit_max_retries.unwrap_or(3),
                retry_backoff: std::time::Duration::from_secs(
                    value.rate_limit_retry_backoff_secs.unwrap_or(1),
                ),
                auto_wait: value.rate_limit_auto_wait.unwrap_or(true),
            })
        } else {
            None
        };

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
            circuit_breaker,
            rate_limit,
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

#[pyclass(name = "Agent", frozen, subclass, skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgent {
    pub inner: std::sync::Arc<Mutex<Agent>>,
    /// Stored MCP clients for restart/health_status operations
    pub mcp_clients: std::sync::Arc<Mutex<Vec<std::sync::Arc<agent::mcp::McpClient>>>>,
    /// Hook registry for extensibility
    pub hooks: std::sync::Arc<Mutex<crate::hooks::PyHookRegistry>>,
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
        let py_hooks = crate::hooks::PyHookRegistry::create();
        let hooks = py_hooks.to_hook_registry();

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
            .with_hooks(hooks)
            .build()
            .map_err(to_py_runtime_error)?;

        Ok(Self {
            inner: std::sync::Arc::new(Mutex::new(agent)),
            mcp_clients: Default::default(),
            hooks: std::sync::Arc::new(Mutex::new(py_hooks)),
        })
    }

    #[classmethod]
    fn create<'py>(
        _cls: &Bound<'py, PyType>,
        py: Python<'py>,
        config: PyRef<'py, PyAgentConfig>,
        _bus: PyRef<'py, PyBus>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let cfg: AgentConfig = config.clone().into();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let py_hooks = crate::hooks::PyHookRegistry::create();
            let hooks = py_hooks.to_hook_registry();

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
                .with_hooks(hooks)
                .build()
                .map_err(to_py_runtime_error)?;

            Python::attach(|py| -> PyResult<Py<PyAny>> {
                let py_agent = Py::new(
                    py,
                    PyAgent {
                        inner: std::sync::Arc::new(Mutex::new(agent)),
                        mcp_clients: Default::default(),
                        hooks: std::sync::Arc::new(Mutex::new(py_hooks)),
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

    fn _stream_placeholder_<'py>(&self, py: Python<'py>, task: String) -> PyResult<Bound<'py, PyAny>> {
        let agent = self.inner.clone();
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;

        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let agent_clone = agent
                .lock()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?
                .clone();

            let (tx, rx) = mpsc::channel::<Result<String, String>>(32);

            tokio::spawn(async move {
                let mut _stream_placeholder_ = agent_clone.stream(&task);
                while let Some(token_result) = _stream_placeholder_.next().await {
                    let item = match token_result {
                        Ok(StreamToken::Text(text)) => Ok(text),
                        Ok(StreamToken::ReasoningContent(text)) => Ok(serde_json::json!({
                            "type": "thinking",
                            "text": text
                        }).to_string()),
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

    fn add_message<'py>(&self, _py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<()> {
        let py_message = PyLlmMessage::from_py(message)?;
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard.add_message(py_message.inner);
        Ok(())
    }

    fn get_messages<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        let messages = guard.get_messages();
        let py_list = pyo3::types::PyList::empty(py);
        for msg in messages {
            let wrapper = PyLlmMessage { inner: msg.clone() };
            py_list.append(wrapper.to_py(py)?)?;
        }
        Ok(py_list.into_any())
    }

    fn save_message_log<'py>(&self, _py: Python<'py>, path: String) -> PyResult<()> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard
            .save_message_log(&path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    fn restore_message_log<'py>(&self, _py: Python<'py>, path: String) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard
            .restore_message_log(&path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    fn session_context<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        let context = guard.session_context();
        json_to_py(py, &context).map(|py_obj| py_obj.into_bound(py))
    }

    fn set_session_context<'py>(&self, _py: Python<'py>, context: &Bound<'py, PyAny>) -> PyResult<()> {
        let context_value: serde_json::Value = py_to_json(context)?;
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard.set_session_context(context_value);
        Ok(())
    }

    fn clear_session_context<'py>(&self, _py: Python<'py>) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard.clear_session_context();
        Ok(())
    }

    fn session_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        let state = guard.session_state();
        let state_json = serde_json::to_value(state)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_py(py, &state_json).map(|py_obj| py_obj.into_bound(py))
    }

    fn save_session<'py>(&self, _py: Python<'py>, path: String) -> PyResult<()> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard
            .save_session(&path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    fn restore_session<'py>(&self, _py: Python<'py>, path: String) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard
            .restore_session(&path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    fn compact_message_log<'py>(&self, _py: Python<'py>) -> PyResult<()> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        guard
            .compact_message_log()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    fn register_hook(&self, event: &Bound<'_, PyAny>, callback: Py<PyAny>) -> PyResult<()> {
        let hook = crate::hooks::PythonHook {
            callback: callback.into(),
        };

        let event_str = event
            .getattr("value")
            .and_then(|v| v.extract::<String>())
            .or_else(|_| event.extract::<String>())
            .map_err(|_| {
                pyo3::exceptions::PyValueError::new_err("event must be a HookEvent or string")
            })?;

        let hook_event = match event_str.as_str() {
            "BeforeToolCall" => HookEvent::BeforeToolCall,
            "AfterToolCall" => HookEvent::AfterToolCall,
            "BeforeLlmCall" => HookEvent::BeforeLlmCall,
            "AfterLlmCall" => HookEvent::AfterLlmCall,
            "OnMessage" => HookEvent::OnMessage,
            "OnComplete" => HookEvent::OnComplete,
            "OnError" => HookEvent::OnError,
            _ => HookEvent::OnMessage,
        };

        {
            let mut agent = self.inner.lock().unwrap();
            agent.add_hook(hook_event, std::sync::Arc::new(hook));
        }

        Ok(())
    }

    fn register_plugin(&self, plugin: pyo3::Py<crate::plugin::PyAgentPlugin>) -> PyResult<()> {
        let plugin_arc = pyo3::Python::attach(
            |py| -> PyResult<std::sync::Arc<dyn agent::agent::plugin::AgentPlugin>> {
                let plugin_ref = plugin.bind(py).borrow();
                Ok(plugin_ref.inner.clone())
            },
        )?;

        self.inner.lock().unwrap().add_plugin(plugin_arc);
        Ok(())
    }

    fn token_usage(&self) -> PyResult<crate::llm_usage::PyTokenUsage> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        Ok(guard.token_usage().into())
    }

    fn token_budget_report(&self) -> PyResult<crate::llm_usage::PyTokenBudgetReport> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Agent lock poisoned"))?;
        Ok(guard.token_budget_report().into())
    }
}
