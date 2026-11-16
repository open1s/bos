use agent::agent::plugin::{
    AgentPlugin as InnerPlugin, LlmRequestWrapper as InnerLlmRequest,
    LlmResponseWrapper as InnerLlmResponse, PluginRegistry as InnerRegistry,
    StreamTokenWrapper as InnerStreamToken, ToolCallWrapper as InnerToolCall,
    ToolResultWrapper as InnerToolResult, TurnEndWrapper as InnerTurnEnd,
    TurnStartWrapper as InnerTurnStart,
};
use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::IntoPyObjectExt;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use std::sync::Arc;

#[pyclass(name = "LlmRequestWrapper", from_py_object)]
#[derive(Clone, Debug)]
/// Mutable view of an outgoing LLM request.
pub struct PyLlmRequestWrapper {
    /// Model identifier.
    #[pyo3(get, set)]
    pub model: String,
    /// Sampling temperature.
    #[pyo3(get, set)]
    pub temperature: Option<f32>,
    /// Completion token cap.
    #[pyo3(get, set)]
    pub max_tokens: Option<u32>,
    /// Nucleus sampling probability.
    #[pyo3(get, set)]
    pub top_p: Option<f32>,
    /// Top-k sampling cutoff.
    #[pyo3(get, set)]
    pub top_k: Option<u32>,
    /// Prompt input.
    #[pyo3(get, set)]
    pub input: String,
    /// Reasoning effort, if any.
    #[pyo3(get, set)]
    pub reasoning_effort: Option<String>,
    /// API protocol.
    #[pyo3(get, set)]
    pub api_mode: String,
}

fn json_to_content(json: &serde_json::Value) -> react::llm::Content {
    match json {
        serde_json::Value::String(s) => react::llm::Content::Text(s.clone()),
        serde_json::Value::Array(arr) => {
            let parts: Vec<react::llm::ContentPart> = arr
                .iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
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
                parts
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap_or_default())
                    .collect(),
            ),
        };
        Self {
            model: req.model.clone(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            top_p: req.top_p,
            top_k: req.top_k,
            input: input_json.to_string(),
            reasoning_effort: req.reasoning_effort.map(|e| e.as_str().to_string()),
            api_mode: req.api_mode.as_str().to_string(),
        }
    }
}

impl From<PyLlmRequestWrapper> for InnerLlmRequest {
    fn from(py_req: PyLlmRequestWrapper) -> Self {
        let json: serde_json::Value =
            serde_json::from_str(&py_req.input).unwrap_or(serde_json::Value::String(py_req.input));
        InnerLlmRequest {
            model: py_req.model,
            input: json_to_content(&json),
            temperature: py_req.temperature,
            max_tokens: py_req.max_tokens,
            top_p: py_req.top_p,
            top_k: py_req.top_k,
            reasoning_effort: py_req
                .reasoning_effort
                .as_deref()
                .map(react::llm::ReasoningEffort::from_name),
            api_mode: react::llm::ApiMode::from_name(&py_req.api_mode),
            metadata: Default::default(),
        }
    }
}

#[pyclass(name = "LlmResponseWrapper", from_py_object)]
#[derive(Clone, Debug)]
/// View of an LLM response.
pub struct PyLlmResponseWrapper {
    /// Response kind.
    #[pyo3(get, set)]
    pub response_type: String,
    /// Text content, if any.
    #[pyo3(get, set)]
    pub content: Option<String>,
    /// Tool name, if this is a tool call.
    #[pyo3(get, set)]
    pub tool_name: Option<String>,
    /// Tool arguments, if this is a tool call.
    #[pyo3(get, set)]
    pub tool_args: Option<String>,
    /// Tool call id, if any.
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
            InnerLlmResponse::Responses(rsp) => {
                if let Some(item) = rsp.output.first() {
                    match item {
                        react::llm::vendor::ResponsesItem::FunctionCall {
                            name, arguments, ..
                        } => {
                            return Self {
                                response_type: "ToolCall".to_string(),
                                content: None,
                                tool_name: Some(name.clone()),
                                tool_args: Some(arguments.clone()),
                                tool_id: None,
                            };
                        }
                        react::llm::vendor::ResponsesItem::Message { content, .. } => {
                            let text: String = content
                                .iter()
                                .filter_map(|p| match p {
                                    react::llm::vendor::ResponsesContentPart::OutputText {
                                        text,
                                        ..
                                    } => Some(text.clone()),
                                    _ => None,
                                })
                                .collect();
                            if !text.is_empty() {
                                return Self {
                                    response_type: "Text".to_string(),
                                    content: Some(text),
                                    tool_name: None,
                                    tool_args: None,
                                    tool_id: None,
                                };
                            }
                        }
                        _ => {}
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
/// View of a tool call.
pub struct PyToolCallWrapper {
    /// Tool name.
    #[pyo3(get, set)]
    pub name: String,
    /// JSON-encoded arguments.
    #[pyo3(get, set)]
    pub args: String,
    /// Provider call id.
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
/// View of a tool result.
pub struct PyToolResultWrapper {
    /// Serialized tool result.
    #[pyo3(get, set)]
    pub result: String,
    /// Whether the call succeeded.
    #[pyo3(get, set)]
    pub success: bool,
    /// Error message, if the call failed.
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

#[pyclass(name = "StreamTokenWrapper", from_py_object)]
#[derive(Clone, Debug)]
/// View of one stream token.
pub struct PyStreamTokenWrapper {
    /// Token kind: Text, ReasoningContent, ToolCall, ToolResult, Usage, Done, Stopped.
    #[pyo3(get, set)]
    pub token_type: String,
    /// Text payload for Text/ReasoningContent.
    #[pyo3(get, set)]
    pub text: Option<String>,
    /// Tool name for ToolCall/ToolResult.
    #[pyo3(get, set)]
    pub name: Option<String>,
    /// JSON-encoded tool arguments for ToolCall.
    #[pyo3(get, set)]
    pub args: Option<String>,
    /// Provider call id for ToolCall.
    #[pyo3(get, set)]
    pub id: Option<String>,
    /// Tool output text for ToolResult.
    #[pyo3(get, set)]
    pub output: Option<String>,
    /// Tool duration in milliseconds for ToolResult.
    #[pyo3(get, set)]
    pub ms: Option<u64>,
    /// JSON-encoded token usage for Usage.
    #[pyo3(get, set)]
    pub usage: Option<String>,
}

impl From<&InnerStreamToken> for PyStreamTokenWrapper {
    fn from(token: &InnerStreamToken) -> Self {
        match token {
            InnerStreamToken::Text(text) => Self {
                token_type: "Text".to_string(),
                text: Some(text.clone()),
                name: None,
                args: None,
                id: None,
                output: None,
                ms: None,
                usage: None,
            },
            InnerStreamToken::ReasoningContent(text) => Self {
                token_type: "ReasoningContent".to_string(),
                text: Some(text.clone()),
                name: None,
                args: None,
                id: None,
                output: None,
                ms: None,
                usage: None,
            },
            InnerStreamToken::ToolCall { name, args, id } => Self {
                token_type: "ToolCall".to_string(),
                text: None,
                name: Some(name.clone()),
                args: Some(args.to_string()),
                id: id.clone(),
                output: None,
                ms: None,
                usage: None,
            },
            InnerStreamToken::ToolResult { name, output, ms } => Self {
                token_type: "ToolResult".to_string(),
                text: None,
                name: Some(name.clone()),
                args: None,
                id: None,
                output: Some(output.clone()),
                ms: Some(*ms),
                usage: None,
            },
            InnerStreamToken::Usage(usage) => Self {
                token_type: "Usage".to_string(),
                text: None,
                name: None,
                args: None,
                id: None,
                output: None,
                ms: None,
                usage: serde_json::to_string(usage).ok(),
            },
            InnerStreamToken::Done => Self {
                token_type: "Done".to_string(),
                text: None,
                name: None,
                args: None,
                id: None,
                output: None,
                ms: None,
                usage: None,
            },
            InnerStreamToken::Stopped => Self {
                token_type: "Stopped".to_string(),
                text: None,
                name: None,
                args: None,
                id: None,
                output: None,
                ms: None,
                usage: None,
            },
        }
    }
}

impl From<PyStreamTokenWrapper> for InnerStreamToken {
    fn from(token: PyStreamTokenWrapper) -> Self {
        match token.token_type.as_str() {
            "Text" => InnerStreamToken::Text(token.text.unwrap_or_default()),
            "ReasoningContent" => {
                InnerStreamToken::ReasoningContent(token.text.unwrap_or_default())
            }
            "ToolCall" => InnerStreamToken::ToolCall {
                name: token.name.unwrap_or_default(),
                args: token
                    .args
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_else(|| serde_json::json!({})),
                id: token.id,
            },
            "ToolResult" => InnerStreamToken::ToolResult {
                name: token.name.unwrap_or_default(),
                output: token.output.unwrap_or_default(),
                ms: token.ms.unwrap_or_default(),
            },
            "Usage" => InnerStreamToken::Usage(
                token
                    .usage
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(react::llm::vendor::openaicompatible::Usage {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        total_tokens: 0,
                        prompt_tokens_details: None,
                    }),
            ),
            "Stopped" => InnerStreamToken::Stopped,
            _ => InnerStreamToken::Done,
        }
    }
}

#[pyclass(name = "TurnStartWrapper", from_py_object)]
#[derive(Clone, Debug)]
/// View of a turn that is about to start.
pub struct PyTurnStartWrapper {
    /// Agent name.
    #[pyo3(get, set)]
    pub agent: String,
    /// Text input for the turn.
    #[pyo3(get, set)]
    pub input: String,
    /// Free-form metadata.
    #[pyo3(get, set)]
    pub metadata: std::collections::HashMap<String, String>,
}

impl From<&InnerTurnStart> for PyTurnStartWrapper {
    fn from(turn: &InnerTurnStart) -> Self {
        Self {
            agent: turn.agent.clone(),
            input: turn.input.clone(),
            metadata: turn.metadata.clone(),
        }
    }
}

impl From<PyTurnStartWrapper> for InnerTurnStart {
    fn from(turn: PyTurnStartWrapper) -> Self {
        let mut inner = InnerTurnStart::new(turn.agent, turn.input);
        inner.metadata = turn.metadata;
        inner
    }
}

#[pyclass(name = "TurnEndWrapper", from_py_object)]
#[derive(Clone, Debug)]
/// View of a finished turn.
pub struct PyTurnEndWrapper {
    /// Agent name.
    #[pyo3(get, set)]
    pub agent: String,
    /// Final text when the turn succeeded.
    #[pyo3(get, set)]
    pub output: Option<String>,
    /// Error text when the turn failed.
    #[pyo3(get, set)]
    pub error: Option<String>,
}

impl From<&InnerTurnEnd> for PyTurnEndWrapper {
    fn from(turn: &InnerTurnEnd) -> Self {
        Self {
            agent: turn.agent.clone(),
            output: turn.output.clone(),
            error: turn.error.clone(),
        }
    }
}

impl From<PyTurnEndWrapper> for InnerTurnEnd {
    fn from(turn: PyTurnEndWrapper) -> Self {
        InnerTurnEnd {
            agent: turn.agent,
            output: turn.output,
            error: turn.error,
        }
    }
}

struct PythonPlugin {
    name: String,
    on_llm_request: Option<Arc<Py<PyAny>>>,
    on_llm_response: Option<Arc<Py<PyAny>>>,
    on_tool_call: Option<Arc<Py<PyAny>>>,
    on_tool_result: Option<Arc<Py<PyAny>>>,
    on_stream_token: Option<Arc<Py<PyAny>>>,
    on_turn_start: Option<Arc<Py<PyAny>>>,
    on_turn_end: Option<Arc<Py<PyAny>>>,
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

    let is_coroutine = Python::attach(|py| result.bind(py).hasattr("__await__")).unwrap_or(false);

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
        let Some(callback) = self.on_llm_request.as_ref() else {
            return Some(request);
        };
        let request_clone = request.clone();

        call_plugin_callback(
            callback,
            |py| PyLlmRequestWrapper::from(&request_clone).into_py_any(py),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyLlmRequestWrapper>().ok().map(|wrapped| {
                    let json: serde_json::Value = serde_json::from_str(&wrapped.input)
                        .unwrap_or_else(|_| serde_json::Value::String(wrapped.input.clone()));
                    InnerLlmRequest {
                        model: wrapped.model,
                        input: json_to_content(&json),
                        temperature: wrapped.temperature,
                        max_tokens: wrapped.max_tokens,
                        top_p: wrapped.top_p,
                        top_k: wrapped.top_k,
                        reasoning_effort: wrapped
                            .reasoning_effort
                            .as_deref()
                            .map(react::llm::ReasoningEffort::from_name),
                        api_mode: react::llm::ApiMode::from_name(&wrapped.api_mode),
                        metadata: request.metadata.clone(),
                    }
                })
            },
        )
        .await
    }

    async fn on_llm_response(&self, response: InnerLlmResponse) -> Option<InnerLlmResponse> {
        let Some(callback) = self.on_llm_response.as_ref() else {
            return Some(response);
        };
        let response_clone = response.clone();

        call_plugin_callback(
            callback,
            |py| PyLlmResponseWrapper::from(&response_clone).into_py_any(py),
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
        let Some(callback) = self.on_tool_call.as_ref() else {
            return Some(tool_call);
        };
        let tool_call_clone = tool_call.clone();

        call_plugin_callback(
            callback,
            |py| PyToolCallWrapper::from(&tool_call_clone).into_py_any(py),
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
        let Some(callback) = self.on_tool_result.as_ref() else {
            return Some(tool_result);
        };
        let tool_result_clone = tool_result.clone();

        call_plugin_callback(
            callback,
            |py| PyToolResultWrapper::from(&tool_result_clone).into_py_any(py),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyToolResultWrapper>().ok().map(|r| r.into())
            },
        )
        .await
    }

    async fn on_stream_token(&self, token: InnerStreamToken) -> Option<InnerStreamToken> {
        let Some(callback) = self.on_stream_token.as_ref() else {
            return Some(token);
        };
        let token_clone = token.clone();

        call_plugin_callback(
            callback,
            |py| PyStreamTokenWrapper::from(&token_clone).into_py_any(py),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyStreamTokenWrapper>().ok().map(|r| r.into())
            },
        )
        .await
    }

    async fn on_turn_start(&self, turn: InnerTurnStart) -> Option<InnerTurnStart> {
        let Some(callback) = self.on_turn_start.as_ref() else {
            return Some(turn);
        };
        let turn_clone = turn.clone();

        call_plugin_callback(
            callback,
            |py| PyTurnStartWrapper::from(&turn_clone).into_py_any(py),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyTurnStartWrapper>().ok().map(|r| r.into())
            },
        )
        .await
    }

    async fn on_turn_end(&self, turn: InnerTurnEnd) -> Option<InnerTurnEnd> {
        let Some(callback) = self.on_turn_end.as_ref() else {
            return Some(turn);
        };
        let turn_clone = turn.clone();

        call_plugin_callback(
            callback,
            |py| PyTurnEndWrapper::from(&turn_clone).into_py_any(py),
            |val| {
                if val.is_none() {
                    return None;
                }
                val.extract::<PyTurnEndWrapper>().ok().map(|r| r.into())
            },
        )
        .await
    }
}

#[pyclass(name = "AgentPlugin", subclass)]
/// Base class for Python agent plugins.
pub struct PyAgentPlugin {
    /// The wrapped plugin.
    pub inner: Arc<dyn InnerPlugin>,
}

#[pymethods]
impl PyAgentPlugin {
    #[new]
    #[pyo3(signature = (
        name,
        on_llm_request = None,
        on_llm_response = None,
        on_tool_call = None,
        on_tool_result = None,
        on_stream_token = None,
        on_turn_start = None,
        on_turn_end = None
    ))]
    fn new(
        name: String,
        on_llm_request: Option<Py<PyAny>>,
        on_llm_response: Option<Py<PyAny>>,
        on_tool_call: Option<Py<PyAny>>,
        on_tool_result: Option<Py<PyAny>>,
        on_stream_token: Option<Py<PyAny>>,
        on_turn_start: Option<Py<PyAny>>,
        on_turn_end: Option<Py<PyAny>>,
    ) -> Self {
        let plugin = PythonPlugin {
            name,
            on_llm_request: on_llm_request.map(Arc::new),
            on_llm_response: on_llm_response.map(Arc::new),
            on_tool_call: on_tool_call.map(Arc::new),
            on_tool_result: on_tool_result.map(Arc::new),
            on_stream_token: on_stream_token.map(Arc::new),
            on_turn_start: on_turn_start.map(Arc::new),
            on_turn_end: on_turn_end.map(Arc::new),
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
/// Registry of agent plugins.
pub struct PyPluginRegistry {
    inner: Arc<InnerRegistry>,
}

impl PyPluginRegistry {
    /// Create an empty registry.
    pub fn create() -> Self {
        Self {
            inner: Arc::new(InnerRegistry::new()),
        }
    }

    /// The shared inner registry.
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
