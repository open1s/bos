//! The engine-facing shim: a resolved capability exposed as one AsyncTool.
//!
//! The engine keeps calling a tool by name; this is the one object that turns
//! that call into a workspace invocation, so the capability path is reachable
//! from ReAct without the engine knowing the capability model exists.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ::capability::{
    Binding, CancelToken, CapabilityDescriptor, CapabilityError, CapabilityId, InvocationContext,
    InvocationId, PolicyDecisionRef, Principal, TaskId, TurnId, WorkspaceRuntime,
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

    /// Forward a cancellation to the real tool, which owns its process/job.
    pub fn cancel(&self, call_id: &str) {
        match self {
            Self::Sync(tool) => tool.cancel(call_id),
            Self::Async(tool) => tool.cancel(call_id),
        }
    }
}
/// Turn/task/policy correlation a host attaches to capability calls.
///
/// The shim is built once per agent and serves every turn, so the values must
/// be updatable between turns. A host holds the shared handle and changes it at
/// the turn boundary; the tool reads a snapshot per call.
#[derive(Debug, Clone)]
pub struct CapabilityCorrelation {
    /// The Agent turn that owns the call.
    pub turn: Option<TurnId>,
    /// The task the call belongs to.
    pub task: Option<TaskId>,
    /// The parent task, for a delegated or retried step.
    pub parent_task: Option<TaskId>,
    /// The attempt number for the logical task.
    pub attempt: u32,
    /// The policy decision that admitted the call.
    pub policy: Option<PolicyDecisionRef>,
}

impl Default for CapabilityCorrelation {
    fn default() -> Self {
        Self {
            turn: None,
            task: None,
            parent_task: None,
            attempt: 1,
            policy: None,
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
    correlation: CapabilityCorrelation,
    shared_correlation: Option<Arc<Mutex<CapabilityCorrelation>>>,
    /// Discovery metadata derived from the descriptor and the binding, so the
    /// model-facing surface name reports the authoritative capability identity.
    metadata: ToolMetadata,
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
        let provider = match runtime.binding(capability) {
            Some(Binding::Local { provider }) => Some(provider.to_string()),
            Some(Binding::Remote { node, .. }) => Some(format!("remote:{node}")),
            None => None,
        };
        let mut metadata = ToolMetadata::new()
            .with_version(descriptor.version.to_string())
            .with_capability(descriptor.id.to_string(), descriptor.version.to_string())
            .with_risk(descriptor.risk.as_str())
            .with_category(descriptor.kind.as_str())
            .with_side_effects(descriptor.side_effects.iter().map(|effect| effect.as_str()))
            .with_permissions(
                descriptor
                    .permissions
                    .iter()
                    .map(|permission| permission.as_str().to_string()),
            );
        if let Some(provider) = provider {
            metadata = metadata.with_provider(provider);
        }
        Some(Self {
            runtime,
            capability: capability.clone(),
            descriptor,
            principal,
            timeout: None,
            calls: Mutex::new(HashMap::new()),
            facts: ToolFacts::default(),
            proxy: None,
            correlation: CapabilityCorrelation::default(),
            shared_correlation: None,
            metadata,
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

    /// Attach the shared turn/task/policy state a host updates per turn.
    pub fn with_correlation(mut self, correlation: Arc<Mutex<CapabilityCorrelation>>) -> Self {
        self.shared_correlation = Some(correlation);
        self
    }

    /// Attach the Agent turn that owns this call.
    pub fn with_turn(mut self, turn: impl Into<String>) -> Self {
        self.correlation.turn = Some(TurnId::new(turn));
        self
    }

    /// Attach the task this call belongs to.
    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.correlation.task = Some(TaskId::new(task));
        self
    }

    /// Attach the parent task and attempt for a delegated/retried call.
    pub fn with_parent_task(mut self, task: impl Into<String>) -> Self {
        self.correlation.parent_task = Some(TaskId::new(task));
        self
    }

    /// Set the attempt number for this logical task.
    pub fn with_attempt(mut self, attempt: u32) -> Self {
        self.correlation.attempt = attempt.max(1);
        self
    }

    /// Attach the policy decision that admitted this call.
    pub fn with_policy(mut self, policy: PolicyDecisionRef) -> Self {
        self.correlation.policy = Some(policy);
        self
    }

    fn context(&self, call_id: &str) -> InvocationContext {
        let mut context = InvocationContext::local(
            InvocationId::new(call_id),
            self.principal.clone(),
            self.runtime.workspace().clone(),
        );
        context.deadline = self.timeout.map(|timeout| Instant::now() + timeout);
        context.composition = Some(self.runtime.composition().clone());
        let correlation = self
            .shared_correlation
            .as_ref()
            .map(|shared| {
                shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            })
            .unwrap_or_else(|| self.correlation.clone());
        context.turn = correlation.turn;
        context.task = correlation.task;
        context.parent_task = correlation.parent_task;
        context.attempt = correlation.attempt;
        context.policy = correlation.policy;
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
        Some(&self.metadata)
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
        if let Some(proxy) = &self.proxy {
            proxy.cancel(call_id);
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
