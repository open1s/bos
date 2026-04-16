use agent::agent::plugin::{
    AgentPlugin as InnerPlugin, LlmRequestWrapper as InnerLlmRequest,
    LlmResponseWrapper as InnerLlmResponse, PluginRegistry as InnerRegistry,
    ToolCallWrapper as InnerToolCall, ToolResultWrapper as InnerToolResult,
};
use async_trait::async_trait;
use pyo3::prelude::*;
use react::llm::LlmContext;
use std::sync::Arc;

#[pyclass(name = "LlmRequestWrapper")]
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
}

impl From<&InnerLlmRequest> for PyLlmRequestWrapper {
    fn from(req: &InnerLlmRequest) -> Self {
        Self {
            model: req.model.clone(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            top_p: req.top_p,
            top_k: req.top_k,
        }
    }
}

impl From<PyLlmRequestWrapper> for InnerLlmRequest {
    fn from(py_req: PyLlmRequestWrapper) -> Self {
        let mut ctx = LlmContext::default();
        ctx.conversations = vec![];
        InnerLlmRequest {
            model: py_req.model,
            context: ctx,
            temperature: py_req.temperature,
            max_tokens: py_req.max_tokens,
            top_p: py_req.top_p,
            top_k: py_req.top_k,
            metadata: Default::default(),
        }
    }
}

#[pyclass(name = "LlmResponseWrapper")]
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
            InnerLlmResponse::Text(s) => Self {
                response_type: "Text".to_string(),
                content: Some(s.clone()),
                tool_name: None,
                tool_args: None,
                tool_id: None,
            },
            InnerLlmResponse::Partial(s) => Self {
                response_type: "Partial".to_string(),
                content: Some(s.clone()),
                tool_name: None,
                tool_args: None,
                tool_id: None,
            },
            InnerLlmResponse::ToolCall { name, args, id } => Self {
                response_type: "ToolCall".to_string(),
                content: None,
                tool_name: Some(name.clone()),
                tool_args: Some(args.to_string()),
                tool_id: id.clone(),
            },
            InnerLlmResponse::Done => Self {
                response_type: "Done".to_string(),
                content: None,
                tool_name: None,
                tool_args: None,
                tool_id: None,
            },
        }
    }
}

impl From<PyLlmResponseWrapper> for InnerLlmResponse {
    fn from(py_resp: PyLlmResponseWrapper) -> Self {
        match py_resp.response_type.as_str() {
            "Text" => InnerLlmResponse::Text(py_resp.content.unwrap_or_default()),
            "Partial" => InnerLlmResponse::Partial(py_resp.content.unwrap_or_default()),
            "ToolCall" => InnerLlmResponse::ToolCall {
                name: py_resp.tool_name.unwrap_or_default(),
                args: py_resp
                    .tool_args
                    .map_or(serde_json::json!({}), |s| serde_json::json!({"raw": s})),
                id: py_resp.tool_id,
            },
            _ => InnerLlmResponse::Done,
        }
    }
}

#[pyclass(name = "ToolCallWrapper")]
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
        InnerToolCall {
            name: py_tc.name,
            args: serde_json::json!(py_tc.args),
            id: py_tc.id,
            metadata: Default::default(),
        }
    }
}

#[pyclass(name = "ToolResultWrapper")]
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
            result
                .ok()
                .and_then(|val| val.extract::<PyLlmRequestWrapper>(py).ok())
                .map(|r| r.into())
        })
    }

    async fn on_llm_response(&self, response: InnerLlmResponse) -> Option<InnerLlmResponse> {
        let callback = self.on_llm_response.as_ref()?;
        let py_response = PyLlmResponseWrapper::from(&response);
        Python::attach(|py| -> Option<InnerLlmResponse> {
            let result = callback.call1(py, (py_response,));
            result
                .ok()
                .and_then(|val| val.extract::<PyLlmResponseWrapper>(py).ok())
                .map(|r| r.into())
        })
    }

    async fn on_tool_call(&self, tool_call: InnerToolCall) -> Option<InnerToolCall> {
        let callback = self.on_tool_call.as_ref()?;
        let py_tool_call = PyToolCallWrapper::from(&tool_call);
        Python::attach(|py| -> Option<InnerToolCall> {
            let result = callback.call1(py, (py_tool_call,));
            result
                .ok()
                .and_then(|val| val.extract::<PyToolCallWrapper>(py).ok())
                .map(|r| r.into())
        })
    }

    async fn on_tool_result(&self, tool_result: InnerToolResult) -> Option<InnerToolResult> {
        let callback = self.on_tool_result.as_ref()?;
        let py_tool_result = PyToolResultWrapper::from(&tool_result);
        Python::attach(|py| -> Option<InnerToolResult> {
            let result = callback.call1(py, (py_tool_result,));
            result
                .ok()
                .and_then(|val| val.extract::<PyToolResultWrapper>(py).ok())
                .map(|r| r.into())
        })
    }
}

#[pyclass(name = "AgentPlugin", subclass)]
pub struct PyAgentPlugin {
    pub inner: Arc<dyn InnerPlugin>,
}

impl PyAgentPlugin {
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
