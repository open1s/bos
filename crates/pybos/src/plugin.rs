use agent::agent::plugin::{
    AgentPlugin as InnerPlugin, LlmRequestWrapper as InnerLlmRequest,
    LlmResponseWrapper as InnerLlmResponse, PluginRegistry as InnerRegistry,
    ToolCallWrapper as InnerToolCall, ToolResultWrapper as InnerToolResult,
};
use async_trait::async_trait;
use pyo3::prelude::*;
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

impl From<&InnerLlmRequest> for PyLlmRequestWrapper {
    fn from(req: &InnerLlmRequest) -> Self {
        Self {
            model: req.model.clone(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            top_p: req.top_p,
            top_k: req.top_k,
            input: req.input.clone(),
        }
    }
}

impl From<PyLlmRequestWrapper> for InnerLlmRequest {
    fn from(py_req: PyLlmRequestWrapper) -> Self {
        InnerLlmRequest {
            model: py_req.model,
            input: py_req.input,
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
        let args: serde_json::Value = serde_json::from_str(&py_tc.args).unwrap_or_else(|_| serde_json::json!(py_tc.args));
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

#[async_trait]
impl InnerPlugin for PythonPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_llm_request(&self, request: InnerLlmRequest) -> Option<InnerLlmRequest> {
        let callback = self.on_llm_request.as_ref()?;
        let py_request = PyLlmRequestWrapper::from(&request);
        Python::attach(|py| -> Option<InnerLlmRequest> {
            let result = callback.call1(py, (py_request,));
            match result {
                Ok(val) if val.is_none(py) => None,
                Ok(val) => val.extract::<PyLlmRequestWrapper>(py).ok().map(|wrapped| {
                    InnerLlmRequest {
                        model: wrapped.model,
                        input: wrapped.input,
                        temperature: wrapped.temperature,
                        max_tokens: wrapped.max_tokens,
                        top_p: wrapped.top_p,
                        top_k: wrapped.top_k,
                        metadata: request.metadata.clone(),
                    }
                }),
                Err(_) => None,
            }
        })
    }

    async fn on_llm_response(&self, response: InnerLlmResponse) -> Option<InnerLlmResponse> {
        let callback = self.on_llm_response.as_ref()?;
        let py_response = PyLlmResponseWrapper::from(&response);
        Python::attach(|py| -> Option<InnerLlmResponse> {
            let result = callback.call1(py, (py_response,));
            match result {
                Ok(val) if val.is_none(py) => None,
                Ok(val) => val.extract::<PyLlmResponseWrapper>(py).ok().map(|r| r.into()),
                Err(_) => None,
            }
        })
    }

    async fn on_tool_call(&self, tool_call: InnerToolCall) -> Option<InnerToolCall> {
        let callback = self.on_tool_call.as_ref()?;
        let py_tool_call = PyToolCallWrapper::from(&tool_call);
        Python::attach(|py| -> Option<InnerToolCall> {
            let result = callback.call1(py, (py_tool_call,));
            match result {
                Ok(val) if val.is_none(py) => None,
                Ok(val) => val.extract::<PyToolCallWrapper>(py).ok().map(|r| r.into()),
                Err(_) => None,
            }
        })
    }

    async fn on_tool_result(&self, tool_result: InnerToolResult) -> Option<InnerToolResult> {
        let callback = self.on_tool_result.as_ref()?;
        let py_tool_result = PyToolResultWrapper::from(&tool_result);
        Python::attach(|py| -> Option<InnerToolResult> {
            let result = callback.call1(py, (py_tool_result,));
            match result {
                Ok(val) if val.is_none(py) => None,
                Ok(val) => val.extract::<PyToolResultWrapper>(py).ok().map(|r| r.into()),
                Err(_) => None,
            }
        })
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
