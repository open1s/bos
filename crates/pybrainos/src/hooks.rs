use agent::agent::hooks::{
    AgentHook, HookContext, HookDecision as InnerDecision, HookEvent as InnerEvent,
    HookRegistry as InnerRegistry,
};
use async_trait::async_trait;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

#[pyclass(name = "HookEvent", from_py_object)]
#[derive(Clone)]
pub struct PyHookEvent {
    pub value: String,
}

#[pymethods]
impl PyHookEvent {
    #[new]
    fn new(value: String) -> Self {
        Self { value }
    }

    fn __str__(&self) -> String {
        self.value.clone()
    }

    fn __eq__(&self, other: &PyHookEvent) -> bool {
        self.value == other.value
    }

    fn __hash__(&self) -> isize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.value.hash(&mut hasher);
        hasher.finish() as isize
    }

    #[getter]
    fn value(&self) -> String {
        self.value.clone()
    }
}

impl From<PyHookEvent> for InnerEvent {
    fn from(e: PyHookEvent) -> Self {
        match e.value.as_str() {
            "BeforeToolCall" => InnerEvent::BeforeToolCall,
            "AfterToolCall" => InnerEvent::AfterToolCall,
            "BeforeLlmCall" => InnerEvent::BeforeLlmCall,
            "AfterLlmCall" => InnerEvent::AfterLlmCall,
            "OnMessage" => InnerEvent::OnMessage,
            "OnComplete" => InnerEvent::OnComplete,
            "OnError" => InnerEvent::OnError,
            _ => InnerEvent::OnMessage,
        }
    }
}

impl From<InnerEvent> for PyHookEvent {
    fn from(e: InnerEvent) -> Self {
        let s = match e {
            InnerEvent::BeforeToolCall => "BeforeToolCall",
            InnerEvent::AfterToolCall => "AfterToolCall",
            InnerEvent::BeforeLlmCall => "BeforeLlmCall",
            InnerEvent::AfterLlmCall => "AfterLlmCall",
            InnerEvent::OnMessage => "OnMessage",
            InnerEvent::OnComplete => "OnComplete",
            InnerEvent::OnError => "OnError",
        };
        Self {
            value: s.to_string(),
        }
    }
}

#[pyclass(name = "HookDecision", from_py_object)]
#[derive(Clone)]
pub struct PyHookDecision(String, Option<String>);

#[pymethods]
impl PyHookDecision {
    #[new]
    fn new(value: String, msg: Option<String>) -> Self {
        Self(value, msg)
    }

    fn __str__(&self) -> String {
        match self.1 {
            Some(ref m) => format!("{}({})", self.0, m),
            None => self.0.clone(),
        }
    }
}

impl From<InnerDecision> for PyHookDecision {
    fn from(d: InnerDecision) -> Self {
        match d {
            InnerDecision::Continue => PyHookDecision("Continue".to_string(), None),
            InnerDecision::Abort => PyHookDecision("Abort".to_string(), None),
            InnerDecision::Error(msg) => PyHookDecision("Error".to_string(), Some(msg)),
        }
    }
}

#[pyclass(name = "HookContext", from_py_object)]
#[derive(Clone)]
pub struct PyHookContext {
    #[pyo3(get, set)]
    pub agent_id: String,
    #[pyo3(get, set)]
    pub data: HashMap<String, String>,
}

#[pymethods]
impl PyHookContext {
    #[new]
    fn new(agent_id: String) -> Self {
        Self {
            agent_id,
            data: HashMap::new(),
        }
    }

    fn set(&mut self, key: String, value: String) {
        self.data.insert(key, value);
    }

    fn get(&self, key: &str) -> Option<String> {
        self.data.get(key).cloned()
    }
}

pub(crate) struct PythonHook {
    pub(crate) callback: Arc<Py<PyAny>>,
}

#[async_trait]
impl AgentHook for PythonHook {
    async fn on_event(&self, event: InnerEvent, context: &HookContext) -> InnerDecision {
        let py_hook_context = PyHookContext {
            agent_id: context.agent_id.clone(),
            data: context.data.clone(),
        };
        let py_event: PyHookEvent = event.into();

        Python::attach(|py| {
            let callback = self.callback.clone();
            let result = callback.call1(py, (py_event, py_hook_context));

            match result {
                Ok(val) => {
                    if let Ok(decision) = val.extract::<PyHookDecision>(py) {
                        match decision.0.as_str() {
                            "Continue" => InnerDecision::Continue,
                            "Abort" => InnerDecision::Abort,
                            "Error" => InnerDecision::Error(decision.1.clone().unwrap_or_default()),
                            _ => InnerDecision::Continue,
                        }
                    } else if let Ok(decision_str) = val.extract::<String>(py) {
                        match decision_str.as_str() {
                            "Continue" => InnerDecision::Continue,
                            "Abort" => InnerDecision::Abort,
                            _ if decision_str.starts_with("Error(") => {
                                let msg = decision_str
                                    .trim_start_matches("Error(")
                                    .trim_end_matches(')');
                                InnerDecision::Error(msg.to_string())
                            }
                            _ => InnerDecision::Continue,
                        }
                    } else {
                        InnerDecision::Continue
                    }
                }
                Err(_) => InnerDecision::Continue,
            }
        })
    }
}

#[pyclass(name = "HookRegistry", frozen, subclass)]
pub struct PyHookRegistry {
    inner: Arc<InnerRegistry>,
}

impl PyHookRegistry {
    pub fn create() -> Self {
        Self {
            inner: Arc::new(InnerRegistry::new()),
        }
    }

    pub fn inner(&self) -> Arc<InnerRegistry> {
        self.inner.clone()
    }

    pub fn to_hook_registry(&self) -> InnerRegistry {
        self.inner.as_ref().clone()
    }
}

#[pymethods]
impl PyHookRegistry {
    #[new]
    fn new() -> Self {
        Self::create()
    }

    pub fn register(&self, event: PyHookEvent, callback: Py<PyAny>) {
        let event: InnerEvent = event.into();
        let hook = PythonHook {
            callback: callback.into(),
        };
        self.inner.register_blocking(event, Arc::new(hook));
    }
}
