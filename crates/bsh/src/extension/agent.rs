//! Agent integration: tools, commands, panels, prompts and contexts.
//!
//! Registration and authorization are deliberately separate. A package that
//! can register a tool (agent.tools.register, a low-risk capability) does not
//! therefore get the capability the tool's body needs; ExtensionTool asks the
//! policy engine on every call, with the call's own path, host and secret as
//! the scope. A denied call is a tool error the model can read, not a panic and
//! not a silent no-op.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use react::tool::registry::AsyncTool as _;
use serde_json::Value;

use crate::plugins::PluginInfo;

use super::capability::{CallScope, Capability, PolicyEngine};
use super::events::{topics, EventBus, ExtensionEvent};
use super::runtime::{CallContext, Runtime, ToolDescriptor};
use super::types::{ExtensionError, ExtensionId, TrustLevel};

/// The largest prompt or context section one call may append.
const MAX_PROMPT_SECTION: usize = 8 * 1024;

/// The largest total prompt material all extensions may append to one build.
const MAX_TOTAL_PROMPT: usize = 64 * 1024;

/// One extension's registered contributions.
pub(crate) struct Contributed {
    /// Extension id.
    pub(crate) extension: ExtensionId,
    /// Display name.
    pub(crate) name: String,
    /// Runtime that executes the tools.
    pub(crate) runtime: Arc<dyn Runtime>,
    /// Trust level used for policy.
    pub(crate) trust: TrustLevel,
    /// Tool descriptors.
    pub(crate) tools: Vec<ToolDescriptor>,
    /// Commands.
    pub(crate) commands: Vec<super::runtime::declarative::CommandContribution>,
    /// Panels.
    pub(crate) panels: Vec<super::runtime::declarative::PanelContribution>,
    /// Prompts.
    pub(crate) prompts: Vec<super::runtime::declarative::TextContribution>,
    /// Contexts.
    pub(crate) contexts: Vec<super::runtime::declarative::TextContribution>,
    /// Workflows, recorded only.
    pub(crate) workflows: Vec<super::runtime::declarative::RawContribution>,
    /// Themes, recorded only.
    pub(crate) themes: Vec<super::runtime::declarative::RawContribution>,
    /// One-call wall-clock ceiling.
    pub(crate) timeout_ms: u64,
}

/// One extension tool exposed to the agent.
pub(crate) struct ExtensionTool {
    owner: ExtensionId,
    runtime: Arc<dyn Runtime>,
    descriptor: ToolDescriptor,
    trust: TrustLevel,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    workspace: Option<PathBuf>,
    timeout_ms: u64,
    qualified: String,
    cancelled: Arc<AtomicBool>,
}

impl ExtensionTool {
    /// Build one tool. The name is qualified with the extension namespace.
    pub(crate) fn new(
        contributed: &Contributed,
        descriptor: ToolDescriptor,
        policy: Arc<PolicyEngine>,
        events: Arc<EventBus>,
        workspace: Option<PathBuf>,
    ) -> Self {
        let qualified = contributed.extension.qualify(&descriptor.id);
        Self {
            owner: contributed.extension.clone(),
            runtime: Arc::clone(&contributed.runtime),
            descriptor,
            trust: contributed.trust,
            policy,
            events,
            workspace,
            timeout_ms: contributed.timeout_ms,
            qualified,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn scope(&self, arguments: &Value) -> CallScope {
        let mut scope = CallScope {
            workspace: self.workspace.clone(),
            path: self.workspace.clone(),
            ..CallScope::default()
        };
        if let Some(raw) = arguments.get("path").and_then(Value::as_str) {
            let path = PathBuf::from(raw);
            scope.path = Some(if path.is_absolute() {
                path
            } else if let Some(workspace) = &self.workspace {
                workspace.join(path)
            } else {
                path
            });
        }
        if let Some(raw) = arguments.get("url").and_then(Value::as_str) {
            if let Ok(url) = reqwest::Url::parse(raw) {
                scope.domain = url.host_str().map(str::to_string);
            }
        }
        scope.secret = arguments
            .get("secret")
            .and_then(Value::as_str)
            .map(str::to_string);
        scope
    }

    fn emit(&self, topic: &str, payload: Value, correlation: &str) {
        if let Ok(event) = ExtensionEvent::new(self.owner.as_str(), topic, payload) {
            let _ = self.events.publish(&event.correlated(correlation));
        }
    }
}

#[async_trait]
impl react::tool::registry::AsyncTool for ExtensionTool {
    fn name(&self) -> &str {
        &self.qualified
    }

    fn description(&self) -> String {
        self.descriptor.description.clone()
    }

    fn category(&self) -> String {
        "extension".to_string()
    }

    fn json_schema(&self) -> Value {
        self.descriptor.input_schema.clone()
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn is_cancelable(&self) -> bool {
        true
    }

    fn cancel(&self, _call_id: &str) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    async fn run(&self, arguments: &Value) -> Result<Value, agent::tools::ToolError> {
        self.cancelled.store(false, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let correlation = uuid::Uuid::new_v4().to_string();
        let scope = self.scope(arguments);
        for capability in &self.descriptor.capabilities {
            if let Err(error) =
                self.policy
                    .authorize_call(self.owner.as_str(), self.trust, *capability, &scope)
            {
                self.emit(
                    topics::TOOL_FAILED,
                    serde_json::json!({
                        "tool": self.descriptor.id.as_str(),
                        "capability": capability.as_str(),
                        "error": error.to_string(),
                    }),
                    &correlation,
                );
                return Err(error.to_tool_error());
            }
        }
        self.emit(
            topics::TOOL_INVOKED,
            serde_json::json!({ "tool": self.descriptor.id.as_str() }),
            &correlation,
        );
        let context = CallContext::new(
            self.owner.clone(),
            correlation.clone(),
            self.workspace.clone(),
            self.timeout_ms,
        );
        let call =
            self.runtime
                .call_tool_result(self.descriptor.id.as_str(), arguments.clone(), &context);
        let cancel = Arc::clone(&self.cancelled);
        let cancelled = async move {
            while !cancel.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let outcome = tokio::select! {
            _ = cancelled => Err(ExtensionError::Runtime {
                extension: self.owner.to_string(),
                message: "cancelled".to_string(),
            }),
            result = tokio::time::timeout(
                Duration::from_millis(self.timeout_ms.max(1)),
                call,
            ) => match result {
                Ok(result) => result,
                Err(_) => Err(ExtensionError::Timeout {
                    extension: self.owner.to_string(),
                    operation: self.qualified.clone(),
                    millis: self.timeout_ms,
                }),
            },
        };
        match outcome {
            Ok(result) => {
                let value = result.to_value();
                let bytes = serde_json::to_vec(&value)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0);
                super::host::record_tool_call(
                    started.elapsed().as_millis() as u64,
                    bytes as u64,
                    true,
                );
                self.emit(
                    topics::TOOL_COMPLETED,
                    serde_json::json!({
                        "tool": self.descriptor.id.as_str(),
                        "bytes": bytes,
                    }),
                    &correlation,
                );
                Ok(value)
            }
            Err(error) => {
                super::host::record_tool_call(started.elapsed().as_millis() as u64, 0, false);
                self.emit(
                    topics::TOOL_FAILED,
                    serde_json::json!({
                        "tool": self.descriptor.id.as_str(),
                        "error": truncate(&error.to_string(), 512),
                    }),
                    &correlation,
                );
                Err(error.to_tool_error())
            }
        }
    }

    async fn run_streaming(
        &self,
        input: &Value,
    ) -> Result<
        std::pin::Pin<Box<dyn Stream<Item = Result<String, agent::tools::ToolError>> + Send>>,
        agent::tools::ToolError,
    > {
        let value = self.run(input).await?;
        let text = match value {
            Value::String(text) => text,
            other => serde_json::to_string(&other)
                .map_err(|error| agent::tools::ToolError::Failed(error.to_string()))?,
        };
        let characters: Vec<char> = text.chars().collect();
        let chunks: Vec<Result<String, agent::tools::ToolError>> = characters
            .chunks(4096)
            .map(|chunk| Ok(chunk.iter().collect()))
            .collect();
        Ok(Box::pin(futures::stream::iter(chunks)))
    }
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Register every contribution a package offered.
///
/// A duplicate tool name is reported and skipped rather than allowed to replace
/// a built-in: the load order of two packages must not decide which body the
/// model calls. Commands and panels share the host plugin namespace, which has
/// the same collision rule.
pub(crate) fn install(
    agent: &mut agent::Agent,
    registry: &mut agent::plugins::PluginRegistry,
    contributed: &Contributed,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    workspace: Option<PathBuf>,
) -> PluginInfo {
    let mut installed_tools = Vec::new();
    for descriptor in &contributed.tools {
        let tool = ExtensionTool::new(
            contributed,
            descriptor.clone(),
            Arc::clone(&policy),
            Arc::clone(&events),
            workspace.clone(),
        );
        let name = tool.name().to_string();
        match agent.try_add_async_tool(Arc::new(tool)) {
            Ok(()) => installed_tools.push(name),
            Err(error) => log::warn!(
                "extension {}: tool {} was not registered: {error}",
                contributed.extension,
                name
            ),
        }
    }
    for command in &contributed.commands {
        let name = contributed.extension.qualify(&command.id);
        let command = agent::plugins::PluginCommand::new(
            name,
            format!("[{}] {}", contributed.name, command.description),
        );
        if let Err(error) = registry.register_command(contributed.extension.as_str(), command) {
            log::warn!(
                "extension {}: command was not registered: {error}",
                contributed.extension
            );
        }
    }
    for panel in &contributed.panels {
        let name = contributed.extension.qualify(&panel.id);
        let panel = agent::plugins::UiPanel::new(name, panel.title.clone());
        if let Err(error) = registry.register_panel(contributed.extension.as_str(), panel) {
            log::warn!(
                "extension {}: panel was not registered: {error}",
                contributed.extension
            );
        }
    }
    let mut prompt_budget = 0usize;
    for prompt in &contributed.prompts {
        append_prompt(
            agent,
            &contributed.extension,
            contributed.trust,
            &policy,
            "Extension prompt",
            prompt,
            &mut prompt_budget,
        );
    }
    for context in &contributed.contexts {
        append_prompt(
            agent,
            &contributed.extension,
            contributed.trust,
            &policy,
            "Extension context",
            context,
            &mut prompt_budget,
        );
    }
    PluginInfo {
        name: contributed.extension.as_str().to_string(),
        tools: installed_tools,
        enabled: true,
    }
}

fn append_prompt(
    agent: &mut agent::Agent,
    extension: &ExtensionId,
    trust: TrustLevel,
    policy: &PolicyEngine,
    heading: &str,
    contribution: &super::runtime::declarative::TextContribution,
    budget: &mut usize,
) {
    let capability = if heading == "Extension prompt" {
        Capability::AgentPromptsProvide
    } else {
        Capability::AgentContextProvide
    };
    if policy
        .authorize_call(extension.as_str(), trust, capability, &CallScope::default())
        .is_err()
    {
        return;
    }
    let text = contribution.text.trim();
    if text.is_empty() {
        return;
    }
    let section = format!(
        "## {heading}: {} ({})
{}",
        contribution.description,
        extension,
        truncate(text, MAX_PROMPT_SECTION)
    );
    if *budget + section.len() > MAX_TOTAL_PROMPT {
        return;
    }
    *budget += section.len();
    agent.append_system_prompt(&section);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::PolicyMode;
    use crate::extension::runtime::empty_schema;
    use crate::extension::types::RuntimeKind;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let index = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bsh-ext-agent-{label}-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct EchoRuntime;

    #[async_trait]
    impl Runtime for EchoRuntime {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(arguments)
        }
    }

    fn policy(scratch: &Scratch) -> Arc<PolicyEngine> {
        Arc::new(
            PolicyEngine::open(
                scratch.0.join("grants.toml"),
                scratch.0.join("audit.jsonl"),
                PolicyMode::Enforce,
            )
            .expect("policy"),
        )
    }

    fn contributed(capabilities: Vec<Capability>, trust: TrustLevel) -> Contributed {
        let extension = ExtensionId::parse("com.example.tool").expect("id");
        Contributed {
            extension,
            name: "Tool".to_string(),
            runtime: Arc::new(EchoRuntime),
            trust,
            tools: vec![ToolDescriptor {
                id: super::super::types::ContributionId::parse("echo").expect("tool id"),
                description: "Echo".to_string(),
                input_schema: empty_schema(),
                capabilities,
            }],
            commands: Vec::new(),
            panels: Vec::new(),
            prompts: Vec::new(),
            contexts: Vec::new(),
            workflows: Vec::new(),
            themes: Vec::new(),
            timeout_ms: 1000,
        }
    }

    #[tokio::test]
    async fn a_builtin_tool_is_qualified_and_runs() {
        let scratch = Scratch::new("run");
        let contributed = contributed(vec![Capability::WorkspaceWrite], TrustLevel::Builtin);
        let tool = ExtensionTool::new(
            &contributed,
            contributed.tools[0].clone(),
            policy(&scratch),
            EventBus::new(),
            Some(PathBuf::from("/tmp/workspace")),
        );
        assert_eq!(tool.name(), "tool__echo");
        let result = react::tool::registry::AsyncTool::run(&tool, &serde_json::json!({"x": 1}))
            .await
            .expect("run");
        assert_eq!(result["x"], 1);
    }

    #[tokio::test]
    async fn a_sensitive_capability_is_denied_without_a_grant() {
        let scratch = Scratch::new("deny");
        let contributed = contributed(vec![Capability::WorkspaceWrite], TrustLevel::Local);
        let tool = ExtensionTool::new(
            &contributed,
            contributed.tools[0].clone(),
            policy(&scratch),
            EventBus::new(),
            Some(PathBuf::from("/tmp/workspace")),
        );
        let error = react::tool::registry::AsyncTool::run(
            &tool,
            &serde_json::json!({"path": "/tmp/workspace/a.txt"}),
        )
        .await
        .expect_err("denied");
        assert!(error.to_string().contains("permission denied"), "{error}");
    }

    #[tokio::test]
    async fn an_explicit_grant_allows_the_call() {
        let scratch = Scratch::new("grant");
        let contributed = contributed(vec![Capability::WorkspaceWrite], TrustLevel::Local);
        let policy = policy(&scratch);
        policy
            .grant(
                "com.example.tool",
                Capability::WorkspaceWrite,
                crate::extension::capability::GrantScope::Workspace,
                None,
            )
            .expect("grant");
        let tool = ExtensionTool::new(
            &contributed,
            contributed.tools[0].clone(),
            policy,
            EventBus::new(),
            Some(PathBuf::from("/tmp/workspace")),
        );
        let result = react::tool::registry::AsyncTool::run(
            &tool,
            &serde_json::json!({"path": "/tmp/workspace/a.txt"}),
        )
        .await
        .expect("run");
        assert_eq!(result["path"], "/tmp/workspace/a.txt");
    }
}
