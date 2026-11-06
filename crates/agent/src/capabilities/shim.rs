//! The engine-facing shim: a resolved capability exposed as one AsyncTool.
//!
//! The engine keeps calling a tool by name; this is the one object that turns
//! that call into a workspace invocation, so the capability path is reachable
//! from ReAct without the engine knowing the capability model exists.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ::capability::{
    CancelToken, CapabilityDescriptor, CapabilityError, CapabilityId, InvocationContext,
    InvocationId, Principal, WorkspaceRuntime,
};
use async_trait::async_trait;
use react::tool::registry::AsyncTool;
use react::tool::{ToolError, ToolMetadata};
use serde_json::Value;

/// Facts about a tool that change how the engine presents it.
///
/// A shim dispatches through the capability runtime, so these cannot be read
/// from the runtime: deferral, skill status, cancelability, and a dispatcher's
/// proxy target are properties of the tool the shim replaced.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolFacts {
    /// The tool is registered but not advertised by default.
    pub deferred: bool,
    /// The tool is a skill.
    pub is_skill: bool,
    /// A running call can be cancelled by id.
    pub cancelable: bool,
}

/// The tool a dispatcher forwards to, kept so proxy identity survives the shim.
#[derive(Clone)]
pub enum ProxySource {
    /// A synchronous tool.
    Sync(Arc<dyn react::tool::registry::Tool>),
    /// An asynchronous tool.
    Async(Arc<dyn AsyncTool>),
}

impl ProxySource {
    /// Ask the real tool what this call resolves to.
    pub fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        match self {
            Self::Sync(tool) => tool.proxy_target(args),
            Self::Async(tool) => tool.proxy_target(args),
        }
    }
}
/// One capability from a workspace, callable by the engine.
pub struct CapabilityTool {
    runtime: Arc<WorkspaceRuntime>,
    capability: CapabilityId,
    descriptor: CapabilityDescriptor,
    principal: Principal,
    timeout: Option<Duration>,
    calls: Mutex<HashMap<String, CancelToken>>,
    facts: ToolFacts,
    proxy: Option<ProxySource>,
}

impl CapabilityTool {
    /// Expose one capability of the runtime. None when the workspace was not
    /// built with it.
    pub fn new(
        runtime: Arc<WorkspaceRuntime>,
        capability: &CapabilityId,
        principal: Principal,
    ) -> Option<Self> {
        let descriptor = runtime
            .describe()
            .into_iter()
            .find(|descriptor| &descriptor.id == capability)
            .cloned()?;
        Some(Self {
            runtime,
            capability: capability.clone(),
            descriptor,
            principal,
            timeout: None,
            calls: Mutex::new(HashMap::new()),
            facts: ToolFacts::default(),
            proxy: None,
        })
    }

    /// Cap the wall clock of every call this shim makes.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Preserve how the replaced tool is presented.
    pub fn with_facts(mut self, facts: ToolFacts) -> Self {
        self.facts = facts;
        self
    }

    /// Preserve a dispatcher's proxy identity.
    pub fn with_proxy(mut self, proxy: ProxySource) -> Self {
        self.proxy = Some(proxy);
        self
    }

    fn context(&self, call_id: &str) -> InvocationContext {
        let mut context = InvocationContext::local(
            InvocationId::new(call_id),
            self.principal.clone(),
            self.runtime.workspace().clone(),
        );
        context.deadline = self.timeout.map(|timeout| Instant::now() + timeout);
        context
    }

    /// The descriptor this shim was built from.
    pub fn descriptor(&self) -> &CapabilityDescriptor {
        &self.descriptor
    }
}

#[async_trait]
impl AsyncTool for CapabilityTool {
    fn name(&self) -> &str {
        self.descriptor.surface.as_str()
    }

    fn description(&self) -> String {
        self.descriptor.description.clone()
    }

    fn category(&self) -> String {
        "capability".to_string()
    }

    fn json_schema(&self) -> Value {
        self.descriptor.input_schema.clone()
    }

    fn metadata(&self) -> Option<&ToolMetadata> {
        None
    }

    fn is_cancelable(&self) -> bool {
        self.facts.cancelable || self.descriptor.supports.cancellation
    }

    fn is_skill(&self) -> bool {
        self.facts.is_skill
    }

    fn deferred(&self) -> bool {
        self.facts.deferred
    }

    fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        self.proxy
            .as_ref()
            .and_then(|proxy| proxy.proxy_target(args))
    }

    fn cancel(&self, call_id: &str) {
        if let Some(token) = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(call_id)
        {
            token.cancel();
        }
    }

    async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let call_id = format!("capability:{}", self.capability);
        self.run_with_call_id(input, &call_id).await
    }

    async fn run_with_call_id(&self, input: &Value, call_id: &str) -> Result<Value, ToolError> {
        let mut context = self.context(call_id);
        let token = CancelToken::new();
        context.cancel = token.clone();
        {
            let mut calls = self
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            calls.insert(call_id.to_string(), token);
        }

        let result = self
            .runtime
            .invoke(&self.capability, input.clone(), &context)
            .await;

        {
            let mut calls = self
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            calls.remove(call_id);
        }

        result
            .map(|output| match output.structured {
                Some(value) => value,
                None => Value::String(
                    output
                        .content
                        .first()
                        .map(|content| format!("{content:?}"))
                        .unwrap_or_default(),
                ),
            })
            .map_err(to_tool_error)
    }
}

/// Map a capability failure onto the engine's tool error, which is the
/// vocabulary ReAct and the transcript already speak.
pub fn to_tool_error(error: CapabilityError) -> ToolError {
    match error {
        CapabilityError::Invalid { message } => ToolError::InvalidInput(message),
        CapabilityError::NotFound { capability } => ToolError::NotFound(capability.to_string()),
        CapabilityError::Timeout { elapsed_ms, .. } => ToolError::Timeout(format!(
            "capability exceeded its deadline after {elapsed_ms} ms"
        )),
        CapabilityError::Denied { reason, .. } => ToolError::Failed(format!("denied: {reason}")),
        other => ToolError::Failed(other.to_string()),
    }
}
