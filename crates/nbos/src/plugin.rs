use agent::agent::plugin::{
    AgentPlugin as InnerPlugin, LlmRequestWrapper as InnerLlmRequest,
    LlmResponseWrapper as InnerLlmResponse, PluginRegistry as InnerRegistry,
    ToolCallWrapper as InnerToolCall, ToolResultWrapper as InnerToolResult,
};
use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::IntoPyObjectExt;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use std::sync::Arc;

#[pyclass(name = "LlmRequestWrapper", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyLlmRequestWrapper {
    #[pyo3(get, set)]
    pub model: String,
    #[pyo3(get, set)]
    pub temperature: Option<f32>,
    #[pyo3(get, set)]
    pub max_tokens: Option<u32>,
    #[pyo3(get, set)]
    pub top_p: Option<f32>,
    #[pyo3(get, set)]
    pub top_k: Option<u32>,
    #[pyo3(get, set)]
    pub input: String,
}

fn json_to_content(json: &serde_json::Value) -> react::llm::Content {
    match json {
        serde_json::Value::String(s) => react::llm::Content::Text(s.clone()),
        serde_json::Value::Array(arr) => {
            let parts: Vec<react::llm::ContentPart> = arr
                .iter()
                .filter_map(|v| {
                    if let Ok(part) = serde_json::from_value(v.clone()) {
                        Some(part)
                    } else {
                        None
                    }
                })
                .collect();
            react::llm::Content::Parts(parts)
        }
        _ => react::llm::Content::Text(json.to_string()),
    }
}

impl From<&InnerLlmRequest> for PyLlmRequestWrapper {
    fn from(req: &InnerLlmRequest) -> Self {
        let input_json = match &req.input {
            react::llm::Content::Text(s) => serde_json::Value::String(s.clone()),
            react::llm::Content::Parts(parts) => serde_json::Value::Array(
                parts.iter().map(|p| serde_json::to_value(p).unwrap_or_default()).collect()
            ),
        };
        Self {
            model: req.model.clone(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            top_p: req.top_p,
            top_k: req.top_k,
            input: input_json.to_string(),
        }
    }
}

impl From<PyLlmRequestWrapper> for InnerLlmRequest {
    fn from(py_req: PyLlmRequestWrapper) -> Self {
        let json: serde_json::Value = serde_json::from_str(&py_req.input).unwrap_or_else(|_| serde_json::Value::String(py_req.input));
        InnerLlmRequest {
            model: py_req.model,
            input: json_to_content(&json),
            temperature: py_req.temperature,
            max_tokens: py_req.max_tokens,
            top_p: py_req.top_p,
            top_k: py_req.top_k,
            metadata: Default::default(),
        }
    }
}

#[pyclass(name = "LlmResponseWrapper", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyLlmResponseWrapper {
    #[pyo3(get, set)]
    pub response_type: String,
    #[pyo3(get, set)]
    pub content: Option<String>,
    #[pyo3(get, set)]
    pub tool_name: Option<String>,
    #[pyo3(get, set)]
    pub tool_args: Option<String>,
    #[pyo3(get, set)]
    pub tool_id: Option<String>,
}

impl From<&InnerLlmResponse> for PyLlmResponseWrapper {
    fn from(resp: &InnerLlmResponse) -> Self {
        match resp {
            InnerLlmResponse::OpenAI(rsp) => {
                if let Some(choice) = rsp.choices.first() {
                    if let Some(tool_calls) = &choice.message.tool_calls {
                        if let Some(tc) = tool_calls.first() {
                            return Self {
                                response_type: "ToolCall".to_string(),
                                content: None,
                                tool_name: tc.function.name.clone(),
                                tool_args: tc.function.arguments.clone(),
                                tool_id: Some(tc.id.clone()),
                            };
                        }
                    }
                    if let Some(content) = &choice.message.content {
                        return Self {
                            response_type: "Text".to_string(),
                            content: Some(content.clone()),
                            tool_name: None,
                            tool_args: None,
                            tool_id: None,
                        };
                    }
                }
                Self {
                    response_type: "Done".to_string(),
                    content: None,
                    tool_name: None,
                    tool_args: None,
                    tool_id: None,
                }
            }
        }
    }
}

impl From<PyLlmResponseWrapper> for InnerLlmResponse {
    fn from(py_resp: PyLlmResponseWrapper) -> Self {
        match py_resp.response_type.as_str() {
            "ToolCall" => {
                let args: serde_json::Value = py_resp
                    .tool_args
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                InnerLlmResponse::OpenAI(ChatCompletionResponse {
                    id: format!("py-{}", uuid::Uuid::new_v4()),
                    object: "chat.completion".to_string(),
                    created: 1234567890,
                    model: "python-model".to_string(),
                    choices: vec![Choice {
                        index: 0,
                        message: ChatMessage {
                            role: "assistant".to_string(),
                            content: None,
                            tool_calls: Some(vec![ToolCall {
                                id: py_resp
                                    .tool_id
                                    .unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4())),
                                r#type: "function".to_string(),
                                function: FunctionCall {
                                    name: py_resp.tool_name,
                                    arguments: Some(args.to_string()),
                                },
                            }]),
                            function_call: None,
                            reasoning_content: None,
                            extra: serde_json::Value::Object(serde_json::Map::new()),
                        },
                        finish_reason: Some("tool_calls".to_string()),
                        stop_reason: None,
                        logprobs: None,
                    }],
                    usage: None,
                    system_fingerprint: None,
                    nvext: None,
                })
            }
            _ => InnerLlmResponse::OpenAI(ChatCompletionResponse {
                id: format!("py-{}", uuid::Uuid::new_v4()),
                object: "chat.completion".to_string(),
                created: 1234567890,
                model: "python-model".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage {
                        role: "assistant".to_string(),
                        content: py_resp.content,
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    },
                    finish_reason: Some("stop".to_string()),
                    stop_reason: None,
                    logprobs: None,
                }],
                usage: None,
                system_fingerprint: None,
                nvext: None,
            }),
        }
    }
}

#[pyclass(name = "ToolCallWrapper", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyToolCallWrapper {
    #[pyo3(get, set)]
    pub name: String,
    #[pyo3(get, set)]
    pub args: String,
    #[pyo3(get, set)]
    pub id: Option<String>,
}

impl From<&InnerToolCall> for PyToolCallWrapper {
    fn from(tc: &InnerToolCall) -> Self {
        Self {
            name: tc.name.clone(),
            args: tc.args.to_string(),
            id: tc.id.clone(),
        }
    }
}

impl From<PyToolCallWrapper> for InnerToolCall {
    fn from(py_tc: PyToolCallWrapper) -> Self {
        let args: serde_json::Value =
            serde_json::from_str(&py_tc.args).unwrap_or_else(|_| serde_json::json!(py_tc.args));
        InnerToolCall {
            name: py_tc.name,
            args,
            id: py_tc.id,
            metadata: Default::default(),
        }
    }
}

#[pyclass(name = "ToolResultWrapper", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyToolResultWrapper {
    #[pyo3(get, set)]
    pub result: String,
    #[pyo3(get, set)]
    pub success: bool,
    #[pyo3(get, set)]
    pub error: Option<String>,
}

impl From<&InnerToolResult> for PyToolResultWrapper {
    fn from(tr: &InnerToolResult) -> Self {
        Self {
            result: tr.result.to_string(),
            success: tr.success,
            error: tr.error.clone(),
        }
    }
}

impl From<PyToolResultWrapper> for InnerToolResult {
    fn from(py_tr: PyToolResultWrapper) -> Self {
        InnerToolResult {
            result: serde_json::json!(py_tr.result),
            success: py_tr.success,
            error: py_tr.error,
            metadata: Default::default(),
        }
    }
}

struct PythonPlugin {
    name: String,
    on_llm_request: Option<Arc<Py<PyAny>>>,
    on_llm_response: Option<Arc<Py<PyAny>>>,
    on_tool_call: Option<Arc<Py<PyAny>>>,
    on_tool_result: Option<Arc<Py<PyAny>>>,
}

/// Helper: call callback, await if coroutine, return Option<T>
async fn call_plugin_callback<T, F>(
    callback: &Py<PyAny>,
    py_arg: impl FnOnce(Python<'_>) -> PyResult<Py<PyAny>>,
    extract: F,
) -> Option<T>
where
    F: Fn(&Bound<'_, PyAny>) -> Option<T>,
{
    let result = Python::attach(|py| callback.call1(py, (py_arg(py)?,))).ok()?;

    let is_coroutine = Python::attach(|py| result.bind(py).hasattr("__await__"))
        .unwrap_or(false);

    let final_result = if is_coroutine {
        crate::utils::await_python_coroutine(result).await.ok()?
    } else {
        result
    };

    Python::attach(|py| extract(final_result.bind(py)))
}

#[async_trait]
impl InnerPlugin for PythonPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_llm_request(&self, request: InnerLlmRequest) -> Option<InnerLlmRequest> {
        let callback = self.on_llm_request.as_ref()?;
        let request_clone = request.clone();

        call_plugin_callback(
            callback,
            |py| Ok(PyLlmRequestWrapper::from(&request_clone).into_py_any(py)?),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyLlmRequestWrapper>()
                    .ok()
                    .map(|wrapped| {
                        let json: serde_json::Value = serde_json::from_str(&wrapped.input).unwrap_or_else(|_| serde_json::Value::String(wrapped.input.clone()));
                        InnerLlmRequest {
                            model: wrapped.model,
                            input: json_to_content(&json),
                            temperature: wrapped.temperature,
                            max_tokens: wrapped.max_tokens,
                            top_p: wrapped.top_p,
                            top_k: wrapped.top_k,
                            metadata: request.metadata.clone(),
                        }
                    })
            },
        )
        .await
    }

    async fn on_llm_response(&self, response: InnerLlmResponse) -> Option<InnerLlmResponse> {
        let callback = self.on_llm_response.as_ref()?;
        let response_clone = response.clone();

        call_plugin_callback(
            callback,
            |py| Ok(PyLlmResponseWrapper::from(&response_clone).into_py_any(py)?),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyLlmResponseWrapper>().ok().map(|r| r.into())
            },
        )
        .await
    }

    async fn on_tool_call(&self, tool_call: InnerToolCall) -> Option<InnerToolCall> {
        let callback = self.on_tool_call.as_ref()?;
        let tool_call_clone = tool_call.clone();

        call_plugin_callback(
            callback,
            |py| Ok(PyToolCallWrapper::from(&tool_call_clone).into_py_any(py)?),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyToolCallWrapper>().ok().map(|r| r.into())
            },
        )
        .await
    }

    async fn on_tool_result(&self, tool_result: InnerToolResult) -> Option<InnerToolResult> {
        let callback = self.on_tool_result.as_ref()?;
        let tool_result_clone = tool_result.clone();

        call_plugin_callback(
            callback,
            |py| Ok(PyToolResultWrapper::from(&tool_result_clone).into_py_any(py)?),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyToolResultWrapper>().ok().map(|r| r.into())
            },
        )
        .await
    }
}

#[pyclass(name = "AgentPlugin", subclass)]
pub struct PyAgentPlugin {
    pub inner: Arc<dyn InnerPlugin>,
}

impl PyAgentPlugin {
    #[allow(dead_code)]
    fn to_inner(self: Arc<Self>) -> Arc<dyn InnerPlugin> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyAgentPlugin {
    #[new]
    #[pyo3(signature = (
        name,
        on_llm_request = None,
        on_llm_response = None,
        on_tool_call = None,
        on_tool_result = None
    ))]
    fn new(
        name: String,
        on_llm_request: Option<Py<PyAny>>,
        on_llm_response: Option<Py<PyAny>>,
        on_tool_call: Option<Py<PyAny>>,
        on_tool_result: Option<Py<PyAny>>,
    ) -> Self {
        let plugin = PythonPlugin {
            name,
            on_llm_request: on_llm_request.map(Arc::new),
            on_llm_response: on_llm_response.map(Arc::new),
            on_tool_call: on_tool_call.map(Arc::new),
            on_tool_result: on_tool_result.map(Arc::new),
        };
        Self {
            inner: Arc::new(plugin),
        }
    }

    fn get_name(&self) -> String {
        self.inner.name().to_string()
    }
}

#[pyclass(name = "PluginRegistry", frozen, subclass)]
pub struct PyPluginRegistry {
    inner: Arc<InnerRegistry>,
}

impl PyPluginRegistry {
    pub fn create() -> Self {
        Self {
            inner: Arc::new(InnerRegistry::new()),
        }
    }

    pub fn inner(&self) -> Arc<InnerRegistry> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyPluginRegistry {
    #[new]
    fn new() -> Self {
        Self::create()
    }

    fn register(&self, plugin: Py<PyAgentPlugin>) {
        let plugin_arc: Arc<dyn InnerPlugin> = Python::attach(|py| {
            let plugin_ref = plugin.bind(py).borrow();
            plugin_ref.inner.clone()
        });
        self.inner.register_blocking(plugin_arc);
    }

    fn list_plugins(&self) -> Vec<String> {
        self.inner.plugin_names_blocking()
    }
}

impl Default for PyPluginRegistry {
    fn default() -> Self {
        Self::create()
    }
}
