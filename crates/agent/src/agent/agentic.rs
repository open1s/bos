use crate::agent::context::{AgentReActApp, AgentReactContext, AgentSession};
use crate::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent, HookRegistry};
use crate::agent::plugin::{
    AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, PluginRegistry, StreamTokenWrapper,
    ToolCallWrapper, ToolResultWrapper,
};
use crate::session::AgentState;
use crate::tools::FunctionTool;
use crate::{AgentError, LlmClient, StreamToken, Tool, ToolRegistry};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use log::warn;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;

use react::engine::{ReActEngine, ReActEngineBuilder};
use react::llm::vendor::LlmRouter;
use react::llm::{
    LlmClient as ReactLlmTrait, LlmError as ReactLlmError, LlmRequest as ReactLlmRequest,
    LlmResponse as ReactLlmResponse, ReactContext, ReactSession, TokenStream as ReactTokenStream,
    TokenStream,
};
use react::tool::{registry::ToolVariant, Tool as ReactToolTrait, ToolError as ReactToolError};
use react::{CircuitBreakerConfig, LlmRequest, RateLimiterConfig, ReActResilience};

pub struct LlmProvider {
    inner: LlmRouter<AgentSession, AgentReactContext>,
}

impl LlmProvider {
    pub fn new() -> Self {
        Self {
            inner: LlmRouter::new(),
        }
    }

    pub fn register_vendor(
        &mut self,
        name: String,
        vendor: Box<dyn LlmClient<AgentSession, AgentReactContext>>,
    ) {
        self.inner.register_vendor(name, vendor);
    }

    pub fn as_dyn(self: Arc<Self>) -> Box<dyn LlmClient<AgentSession, AgentReactContext>> {
        Box::new(ArcLlmClient(self))
    }
}

struct ArcLlmClient(Arc<LlmProvider>);

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for ArcLlmClient {
    async fn complete(
        &self,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<ReactLlmResponse, ReactLlmError> {
        self.0.complete(req, session, context).await
    }

    async fn stream_complete(
        &self,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<TokenStream, ReactLlmError> {
        self.0.stream_complete(req, session, context).await
    }

    fn supports_tools(&self) -> bool {
        self.0.supports_tools()
    }

    fn provider_name(&self) -> &'static str {
        self.0.provider_name()
    }
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for LlmProvider {
    async fn complete(
        &self,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<ReactLlmResponse, ReactLlmError> {
        self.inner.complete(req, session, context).await
    }

    async fn stream_complete(
        &self,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<ReactTokenStream, ReactLlmError> {
        self.inner.stream_complete(req, session, context).await
    }

    fn supports_tools(&self) -> bool {
        self.inner.supports_tools()
    }

    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }
}

struct ExtensibleToolAdapter {
    inner: Arc<dyn Tool + Send + Sync>,
    plugins: PluginRegistry,
    hooks: HookRegistry,
    agent_id: String,
}

impl ExtensibleToolAdapter {
    fn new(
        inner: Arc<dyn Tool + Send + Sync>,
        plugins: PluginRegistry,
        hooks: HookRegistry,
        agent_id: String,
    ) -> Self {
        Self {
            inner,
            plugins,
            hooks,
            agent_id,
        }
    }

    fn trigger_before_hook(
        &self,
        tool_name: &str,
        original_args: &str,
    ) -> Result<(), ReactToolError> {
        if !self.hooks.has_hooks_blocking(&HookEvent::BeforeToolCall) {
            return Ok(());
        }

        let mut ctx = HookContext::new(&self.agent_id);
        ctx.set("tool_name", tool_name);
        ctx.set("tool_args", original_args);
        let decision = self.hooks.trigger_blocking(HookEvent::BeforeToolCall, ctx);
        match decision {
            HookDecision::Error(msg) => Err(ReactToolError::Failed(format!(
                "BeforeToolCall hook error: {}",
                msg
            ))),
            HookDecision::Abort => Err(ReactToolError::Failed("Tool call aborted by hook".into())),
            HookDecision::Continue => Ok(()),
        }
    }

    fn trigger_after_hook(
        &self,
        tool_name: &str,
        original_args: &str,
        effective_args: Option<&str>,
        result: &Result<serde_json::Value, ReactToolError>,
    ) -> Result<(), ReactToolError> {
        if !self.hooks.has_hooks_blocking(&HookEvent::AfterToolCall) {
            return Ok(());
        }

        let mut ctx = HookContext::new(&self.agent_id);
        ctx.set("tool_name", tool_name);
        ctx.set("tool_args", original_args);
        if let Some(effective_args) = effective_args {
            ctx.set("effective_tool_args", effective_args);
        }
        match result {
            Ok(v) => ctx.set("tool_result", v.to_string()),
            Err(e) => ctx.set("error", e.to_string()),
        }
        let decision = self.hooks.trigger_blocking(HookEvent::AfterToolCall, ctx);
        match decision {
            HookDecision::Error(msg) => {
                return Err(ReactToolError::Failed(format!(
                    "AfterToolCall hook error: {}",
                    msg
                )));
            }
            HookDecision::Abort => {
                return Err(ReactToolError::Failed(
                    "Tool call aborted by after hook".to_string(),
                ));
            }
            HookDecision::Continue => {}
        }
        Ok(())
    }
}

impl ReactToolTrait for ExtensibleToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.inner.json_schema()
    }

    fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ReactToolError> {
        let tool_name = self.inner.name().to_string();
        let original_args = input.to_string();

        self.trigger_before_hook(&tool_name, &original_args)?;

        let effective_args = input.clone();
        let effective_args_str = None;

        let execution_result = self
            .inner
            .run(&effective_args)
            .map_err(|e| ReactToolError::Failed(e.to_string()));

        self.trigger_after_hook(
            &tool_name,
            &original_args,
            effective_args_str,
            &execution_result,
        )?;

        execution_result
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }
}

// ============================================================================
// Simplified Agent API - Builder Pattern
// ============================================================================

/// Agent builder for fluent configuration.
#[derive(Debug, Clone)]
#[qserde::Archive]
pub struct AgentConfig {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub timeout_secs: u64,
    pub max_steps: usize,
    /// Circuit breaker configuration for resilience
    pub circuit_breaker: Option<CircuitBreakerConfig>,
    /// Rate limiter configuration for resilience
    pub rate_limit: Option<RateLimiterConfig>,
    pub context_compaction_threshold_tokens: usize,
    pub context_compaction_trigger_ratio: f32,
    pub context_compaction_keep_recent_messages: usize,
    pub context_compaction_max_summary_chars: usize,
    pub context_compaction_summary_max_tokens: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "agent".to_string(),
            model: "gpt-4".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            system_prompt: "You are a helpful assistant.".to_string(),
            temperature: 0.7,
            max_tokens: None,
            timeout_secs: 60,
            max_steps: 10,
            circuit_breaker: None,
            rate_limit: None,
            context_compaction_threshold_tokens: 24_000,
            context_compaction_trigger_ratio: 0.85,
            context_compaction_keep_recent_messages: 12,
            context_compaction_max_summary_chars: 4_000,
            context_compaction_summary_max_tokens: 600,
        }
    }
}

/// Agent is the main abstraction for AI agents with LLM integration,
/// tool registries, and skill management.
#[qserde::Archive]
#[rkyv(crate = qserde::rkyv)]
pub struct Agent {
    config: AgentConfig,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    llm: Arc<LlmProvider>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    registry: Option<Arc<ToolRegistry>>,
    #[rkyv(with = qserde::rkyv::with::Map<qserde::rkyv::with::AsString>)]
    skills_dir: Option<std::path::PathBuf>,
    skills: Vec<crate::skills::SkillContent>,
    resilience: ReActResilience,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    session: std::sync::Mutex<AgentSession>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    hooks: HookRegistry,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    plugins: PluginRegistry,
}

impl Agent {
    /// Create a new Agent with the given config and LLM client.
    pub fn new(config: AgentConfig, llm: Arc<LlmProvider>) -> Self {
        let resilience = ReActResilience::new(react::ResilienceConfig {
            circuit_breaker: config.circuit_breaker.clone().unwrap_or_default(),
            rate_limiter: config.rate_limit.clone().unwrap_or_default(),
        });
        Self {
            config,
            llm,
            registry: Some(Arc::new(ToolRegistry::new())),
            skills_dir: None,
            skills: Vec::new(),
            resilience,
            session: std::sync::Mutex::new(AgentSession::new()),
            hooks: HookRegistry::new(),
            plugins: PluginRegistry::new(),
        }
    }

    /// Get the config.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Get tool registry.
    pub fn registry(&self) -> Option<&Arc<ToolRegistry>> {
        self.registry.as_ref()
    }

    /// Get hooks registry for external registration.
    pub fn hooks(&self) -> &HookRegistry {
        &self.hooks
    }

    pub fn plugins(&self) -> &PluginRegistry {
        &self.plugins
    }

    pub fn add_message(&mut self, message: react::llm::LlmMessage) {
        self.session.lock().unwrap().push(message);
        self.hooks
            .trigger_all_blocking(HookEvent::OnMessage, HookContext::new(&self.config.name));
    }

    pub fn session_state(&self) -> AgentState {
        let session = self.session.lock().unwrap();
        AgentState {
            agent_id: self.config.name.clone(),
            message_log: session.messages().to_vec(),
            context: session.session_context(),
            metadata: crate::session::SessionMetadata {
                created_at: 0,
                updated_at: 0,
                message_count: session.len(),
            },
        }
    }

    pub fn save_session(&self, path: &str) -> Result<(), AgentError> {
        self.session
            .lock()
            .unwrap()
            .save(path)
            .map_err(|e| AgentError::Session(e.to_string()))
    }

    pub fn restore_session(&mut self, path: &str) -> Result<(), AgentError> {
        self.session
            .lock()
            .unwrap()
            .restore(path)
            .map_err(|e| AgentError::Session(e.to_string()))
    }

    /// Add a tool that calls another agent via bus Caller.
    pub fn add_remote_agent_tool(
        &mut self,
        tool_name: impl Into<String>,
        endpoint: impl Into<String>,
        session: Arc<bus::Session>,
    ) -> Result<(), crate::ToolError> {
        let tool = Arc::new(crate::bus::AgentCallerTool::new(
            tool_name, endpoint, session,
        ));
        self.try_add_tool(tool)
    }

    /// Create a typed RPC client for another agent endpoint.
    pub fn rpc_client(
        &self,
        endpoint: impl Into<String>,
        session: Arc<bus::Session>,
    ) -> crate::bus::AgentRpcClient {
        crate::bus::AgentRpcClient::new(endpoint, session)
    }

    /// Expose this agent as a bus callable endpoint for agent-to-agent calls.
    pub fn as_callable_server(
        &self,
        endpoint: impl Into<String>,
        session: Arc<bus::Session>,
    ) -> crate::bus::AgentCallableServer {
        crate::bus::AgentCallableServer::new(endpoint, session, Arc::new(self.clone()))
    }

    /// Prepare context with tools and skills populated from the agent's state.
    /// Shared by react() and stream() to avoid code duplication.
    fn prepare_context(&self) -> AgentReactContext {
        let mut context = AgentReactContext::new(self.config.name.clone());

        if let Some(ref registry) = self.registry {
            context.tools = registry
                .iter()
                .map(|(name, tool)| react::llm::LlmTool {
                    name: name.clone(),
                    description: tool.description(),
                    parameters: tool.json_schema(),
                })
                .collect();
        }

        if !self.skills.is_empty() {
            context.skills = self
                .skills
                .iter()
                .map(|s| react::llm::Skill {
                    category: s.metadata.category.as_str().to_string(),
                    name: s.metadata.name.clone(),
                    description: s.metadata.description.clone(),
                })
                .collect();
        }

        context
    }

    /// Build a ReActEngine with the standard adapter stack (LLM, tools, skills).
    /// Shared by react(), run_simple(), and stream() to avoid duplicating adapter construction.
    fn build_react_engine(&self) -> Result<ReActEngine<AgentReActApp>, AgentError> {
        let react_llm = self.llm.clone().as_dyn();

        let mut builder = ReActEngineBuilder::<AgentReActApp>::new()
            .llm(react_llm)
            .resilience(self.resilience.clone())
            .llm_timeout(self.config.timeout_secs)
            .max_steps(self.config.max_steps)
            .model(self.config.model.clone());

        if let Some(ref registry) = self.registry {
            for (_name, tool) in registry.iter() {
                let tool_adapter = Box::new(ExtensibleToolAdapter::new(
                    tool.clone(),
                    self.plugins.clone(),
                    self.hooks.clone(),
                    self.config.name.clone(),
                ));
                builder = builder.with_tool(ToolVariant::Sync(tool_adapter));
            }
        }

        let has_skills = !self.skills.is_empty();
        if has_skills {
            let skill_names: Vec<String> = self
                .skills
                .iter()
                .map(|s| s.metadata.name.clone())
                .collect();

            for skill in &self.skills {
                let skill_name = skill.metadata.name.clone();
                let skill_desc = format!("Get instructions for the {} skill", skill_name);
                let skill_instructions = skill.instructions.clone();
                let skill_name_for_closure = skill_name.clone();
                let skill_tool = Arc::new(FunctionTool::skill(
                    &skill_name,
                    &skill_desc,
                    serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                    move |_args: &serde_json::Value| {
                        Ok(serde_json::json!({
                            "skill": skill_name_for_closure,
                            "instructions": skill_instructions
                        }))
                    },
                ));
                builder =
                    builder.with_tool(ToolVariant::Sync(Box::new(ExtensibleToolAdapter::new(
                        skill_tool,
                        self.plugins.clone(),
                        self.hooks.clone(),
                        self.config.name.clone(),
                    ))));
            }

            let load_skill_tool = Arc::new(FunctionTool::new(
                "load_skill",
                "Load a skill by name to get its instructions",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Name of the skill to load"
                        }
                    },
                    "required": ["name"]
                }),
                {
                    let skills = self.skills.clone();
                    move |args: &serde_json::Value| {
                        let name = args
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let found = skills.iter().find(|s| s.metadata.name == name);
                        if let Some(skill) = found {
                            Ok(serde_json::json!({
                                "name": skill.metadata.name,
                                "description": skill.metadata.description,
                                "instructions": skill.instructions
                            }))
                        } else {
                            Ok(serde_json::json!({
                                "error": format!("Skill '{}' not found. Available: {}", name, skill_names.join(", "))
                            }))
                        }
                    }
                },
            ));
            builder = builder.with_tool(ToolVariant::Sync(Box::new(ExtensibleToolAdapter::new(
                load_skill_tool,
                self.plugins.clone(),
                self.hooks.clone(),
                self.config.name.clone(),
            ))));
        }

        builder
            .build()
            .map_err(|e| AgentError::Session(format!("ReAct build error: {}", e)))
    }

    /// Run the agent using ReAct engine. Uses the agent's existing session
    /// and writes results back after completion. Session is not locked during
    /// engine execution to avoid deadlocks with hooks.
    pub async fn react(&self, task: &str) -> Result<String, AgentError> {
        let mut engine = self.build_react_engine()?;

        // Clone messages out of the shared session so the lock is released
        // before engine execution (hooks may need to access the session).
        let initial_messages = {
            let session = self.session.lock().unwrap();
            session.messages().to_vec()
        };
        let mut agent_session = AgentSession::new();
        for msg in initial_messages {
            agent_session.push(msg);
        }

        let mut context = self.prepare_context();

        let request = LlmRequest {
            model: self.config.model.clone(),
            input: task.to_string(),
            temperature: Some(self.config.temperature),
            ..Default::default()
        };

        let result = engine
            .react(request, &mut agent_session, &mut context)
            .await;

        // Write results back to the shared session
        {
            let mut session = self.session.lock().unwrap();
            for msg in agent_session.messages().to_vec() {
                session.push(msg);
            }
        }

        match result {
            Ok(answer) => {
                self.hooks
                    .trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name))
                    .await;
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", "0");
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
                Ok(answer)
            }
            Err(e) => {
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("error", e.to_string());
                let decision = self.hooks.trigger(HookEvent::OnError, ctx).await;
                match decision {
                    HookDecision::Error(msg) => log::warn!("OnError hook error: {}", msg),
                    HookDecision::Abort => return Ok("LLM call aborted by hook".to_string()),
                    HookDecision::Continue => {}
                }
                Err(AgentError::Session(format!("ReAct run error: {}", e)))
            }
        }
    }

    /// Run the agent (delegates to react with session + hooks via AgentReActApp).
    pub async fn run_simple(&self, task: &str) -> Result<String, AgentError> {
        self.react(task).await
    }

    /// Stream the agent response using ReAct-style loop.
    /// Supports tools and skills with multi-turn LLM calls - executes tools and continues
    /// until final response from LLM.
    pub fn stream(
        &self,
        task: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send + '_>> {
        let engine_result = self.build_react_engine();
        let task_str = task.to_string();

        let mut context = self.prepare_context();

        let initial_messages = {
            let session = self.session.lock().unwrap();
            session.messages().to_vec()
        };
        let mut agent_session = AgentSession::new();
        for msg in initial_messages {
            agent_session.push(msg);
        }

        let stream = async_stream::stream! {
            let engine = match engine_result {
                Ok(e) => e,
                Err(e) => {
                    yield Err(e);
                    return;
                    }
            };

            let request = LlmRequest {
                model: self.config.model.clone(),
                input: task_str.clone(),
                temperature: Some(self.config.temperature),
                ..Default::default()
            };

            // Consume stream in its own scope to release borrow of agent_session
            {
                let react_stream = engine.react_stream(request, &mut agent_session, &mut context);
                futures::pin_mut!(react_stream);
                while let Some(item) = react_stream.next().await {
                    yield item.map_err(|e| AgentError::Session(e.to_string()));
                }
            }

            // Write results back to the shared session
            {
                let mut session = self.session.lock().unwrap();
                for msg in agent_session.messages().to_vec() {
                    session.push(msg);
                }
            }

        };

        Box::pin(stream)
    }

    /// Register a tool.
    pub fn add_tool(&mut self, tool: Arc<dyn Tool>) {
        if let Err(e) = self.try_add_tool(tool) {
            warn!("Failed to register tool: {}", e);
        }
    }

    pub fn add_plugin(&mut self, plugin: Arc<dyn AgentPlugin>) {
        self.plugins.register_blocking(plugin);
    }

    pub fn add_hook(&mut self, event: HookEvent, hook: Arc<dyn AgentHook>) {
        self.hooks.register_blocking(event, hook);
    }

    /// Clear runtime extensions (tools, hooks, plugins).
    /// Useful for host-language bindings to release callback resources promptly.
    pub fn clear_runtime_extensions(&mut self) {
        self.registry = Some(Arc::new(ToolRegistry::new()));
        self.hooks.clear_all_blocking();
        self.plugins.clear_blocking();
    }

    /// Register a tool and return explicit error on failure.
    pub fn try_add_tool(&mut self, tool: Arc<dyn Tool>) -> Result<(), crate::ToolError> {
        if let Some(ref mut reg) = self.registry {
            Arc::make_mut(reg).register(tool)?;
        } else {
            let mut reg = ToolRegistry::new();
            reg.register(tool)?;
            self.registry = Some(Arc::new(reg));
        }
        Ok(())
    }

    /// Get skills schemas for LLM.
    pub fn get_skills_schemas(&self) -> Vec<serde_json::Value> {
        self.skills
            .iter()
            .map(|skill| {
                serde_json::json!({
                    "name": skill.metadata.name,
                    "description": skill.metadata.description,
                    "category": skill.metadata.category.as_str(),
                    "tags": skill.metadata.tags
                })
            })
            .collect()
    }

    /// Get skills content (including instructions) for LLM system prompt.
    pub fn get_skills_content(&self) -> Vec<(&str, &str)> {
        self.skills
            .iter()
            .map(|skill| (skill.metadata.name.as_str(), skill.instructions.as_str()))
            .collect()
    }

    /// Register skills from directory.
    pub fn register_skills_from_dir(
        &mut self,
        dir: std::path::PathBuf,
    ) -> Result<(), crate::skills::SkillError> {
        use crate::skills::SkillLoader;
        let mut loader = SkillLoader::new(dir.clone());
        loader.discover()?;
        for skill_meta in loader.list() {
            let content = loader
                .load(&skill_meta.name)
                .ok_or_else(|| crate::skills::SkillError::NotFound(skill_meta.name.clone()))?;
            self.skills.push(content);
        }
        self.skills_dir = Some(dir);
        Ok(())
    }

    /// Register MCP tools from an MCP client.
    pub async fn register_mcp_tools(
        &mut self,
        client: std::sync::Arc<crate::mcp::McpClient>,
    ) -> Result<(), crate::mcp::McpError> {
        self.register_mcp_tools_with_namespace(client, "mcp").await
    }

    /// Register MCP tools under a namespace (tool names become `{namespace}/{tool}`).
    pub async fn register_mcp_tools_with_namespace(
        &mut self,
        client: std::sync::Arc<crate::mcp::McpClient>,
        namespace: &str,
    ) -> Result<(), crate::mcp::McpError> {
        use crate::mcp::McpToolAdapter;
        let namespace = namespace.trim();
        if namespace.is_empty() {
            return Err(crate::mcp::McpError::Protocol(
                "MCP namespace must not be empty".to_string(),
            ));
        }
        if namespace.contains('/') {
            return Err(crate::mcp::McpError::Protocol(format!(
                "Invalid MCP namespace '{}': '/' is not allowed",
                namespace
            )));
        }
        if !namespace
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(crate::mcp::McpError::Protocol(format!(
                "Invalid MCP namespace '{}': allowed chars are [A-Za-z0-9._-]",
                namespace
            )));
        }

        if client.get_capabilities().await.is_none() {
            client.initialize().await?;
        }

        let tools = client.list_tools().await?;
        let registry = self
            .registry
            .get_or_insert_with(|| Arc::new(ToolRegistry::new()));

        // Preflight first to keep registration atomic.
        let mut seen_names = HashSet::new();
        for tool in &tools {
            if !seen_names.insert(tool.name.clone()) {
                return Err(crate::mcp::McpError::Protocol(format!(
                    "Duplicate MCP tool name in server response: '{}'",
                    tool.name
                )));
            }

            let namespaced_name = format!("{}/{}", namespace, tool.name);
            if registry.get(&namespaced_name).is_some() {
                return Err(crate::mcp::McpError::Protocol(format!(
                    "Failed to register MCP tool '{}': duplicate tool '{}'",
                    tool.name, namespaced_name
                )));
            }
        }

        let reg_mut = Arc::make_mut(registry);
        for tool in tools {
            let schema = tool.input_schema.clone();
            let tool_name = tool.name.clone();
            let mcp_tool = std::sync::Arc::new(McpToolAdapter::new(
                client.clone(),
                tool_name.clone(),
                tool_name.clone(),
                tool.description.clone(),
                schema,
            ));
            reg_mut
                .register_with_namespace(mcp_tool, namespace)
                .map_err(|e| {
                    crate::mcp::McpError::Protocol(format!(
                        "Failed to register MCP tool '{}': {}",
                        tool_name, e
                    ))
                })?;
        }
        Ok(())
    }
}

/// Clone implementation for stateless agent.
impl Clone for Agent {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            llm: self.llm.clone(),
            registry: self.registry.clone(),
            skills_dir: self.skills_dir.clone(),
            skills: self.skills.clone(),
            resilience: self.resilience.clone(),
            session: std::sync::Mutex::new(self.session.lock().unwrap().clone()),
            hooks: self.hooks.clone(),
            plugins: self.plugins.clone(),
        }
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::{AgentReactContext, AgentSession};
    use crate::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent};
    use crate::tools::FunctionTool;
    use async_trait::async_trait;
    use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
    use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, StreamToken};
    use std::sync::Arc;

    /// Mock LLM for testing
    struct MockLlm;

    impl MockLlm {
        fn new() -> Self {
            Self
        }
    }

    fn make_text_response(content: String) -> LlmResponse {
        LlmResponse::OpenAI(ChatCompletionResponse {
            id: "test-mock".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "mock-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
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
        })
    }

    #[async_trait]
    impl LlmClient<AgentSession, AgentReactContext> for MockLlm {
        async fn complete(
            &self,
            _req: LlmRequest,
            _session: &mut AgentSession,
            _context: &mut AgentReactContext,
        ) -> Result<LlmResponse, LlmError> {
            Ok(make_text_response("mock response".to_string()))
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
            _session: &mut AgentSession,
            _context: &mut AgentReactContext,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::Text("chunk1".to_string())),
                Ok(StreamToken::Text("chunk2".to_string())),
                Ok(StreamToken::Done),
            ])))
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    fn make_llm_provider() -> LlmProvider {
        let mut provider = LlmProvider::new();
        provider.register_vendor("mock".to_string(), Box::new(MockLlm::new()));
        provider
    }

    // =========================================================================
    // AgentConfig Tests
    // =========================================================================

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.name, "agent");
        assert_eq!(config.model, "gpt-4");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.api_key, "");
        assert_eq!(config.system_prompt, "You are a helpful assistant.");
        assert_eq!(config.temperature, 0.7);
        assert!(config.max_tokens.is_none());
        assert_eq!(config.timeout_secs, 60);
        assert_eq!(config.max_steps, 10);
        assert!(config.circuit_breaker.is_none());
        assert!(config.rate_limit.is_none());
    }

    #[test]
    fn test_agent_config_context_compaction_defaults() {
        let config = AgentConfig::default();
        assert_eq!(config.context_compaction_threshold_tokens, 24_000);
        assert_eq!(config.context_compaction_trigger_ratio, 0.85);
        assert_eq!(config.context_compaction_keep_recent_messages, 12);
        assert_eq!(config.context_compaction_max_summary_chars, 4_000);
        assert_eq!(config.context_compaction_summary_max_tokens, 600);
    }

    // =========================================================================
    // LlmProvider Tests
    // =========================================================================

    #[test]
    fn test_llm_provider_new() {
        let provider = LlmProvider::new();
        drop(provider);
    }

    #[test]
    fn test_llm_provider_register_vendor() {
        let mut provider = LlmProvider::new();
        provider.register_vendor("test".to_string(), Box::new(MockLlm::new()));
    }

    #[test]
    fn test_llm_provider_as_dyn() {
        let provider = Arc::new(LlmProvider::new());
        let _dyn: Box<dyn LlmClient<AgentSession, AgentReactContext>> = provider.as_dyn();
    }

    // =========================================================================
    // Agent Creation Tests
    // =========================================================================

    #[test]
    fn test_agent_new_with_config_and_llm() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        assert_eq!(agent.config().name, "agent");
        assert_eq!(agent.config().model, "gpt-4");
    }

    #[test]
    fn test_agent_new_with_custom_config() {
        let mut config = AgentConfig::default();
        config.name = "test-agent".to_string();
        config.model = "gpt-3.5-turbo".to_string();
        config.max_steps = 5;

        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        assert_eq!(agent.config().name, "test-agent");
        assert_eq!(agent.config().model, "gpt-3.5-turbo");
        assert_eq!(agent.config().max_steps, 5);
    }

    // =========================================================================
    // Agent Tool Registration Tests
    // =========================================================================

    #[test]
    fn test_agent_add_tool() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({"type":"object"}),
            |_args| Ok(serde_json::json!("result")),
        ));

        agent.add_tool(tool);

        let registry = agent.registry().unwrap();
        assert!(registry.get("test_tool").is_some());
    }

    #[test]
    fn test_agent_add_multiple_tools() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let tool1 = Arc::new(FunctionTool::new(
            "tool1",
            "First tool",
            serde_json::json!({"type":"object"}),
            |_args| Ok(serde_json::json!("result1")),
        ));

        let tool2 = Arc::new(FunctionTool::new(
            "tool2",
            "Second tool",
            serde_json::json!({"type":"object"}),
            |_args| Ok(serde_json::json!("result2")),
        ));

        agent.add_tool(tool1);
        agent.add_tool(tool2);

        let registry = agent.registry().unwrap();
        assert!(registry.get("tool1").is_some());
        assert!(registry.get("tool2").is_some());
    }

    // =========================================================================
    // Agent Hook Registration Tests
    // =========================================================================

    struct TestHook {
        called: std::sync::Arc<std::sync::Mutex<bool>>,
    }

    impl TestHook {
        fn new() -> Self {
            Self {
                called: Arc::new(std::sync::Mutex::new(false)),
            }
        }
    }

    #[async_trait]
    impl AgentHook for TestHook {
        async fn on_event(&self, _event: HookEvent, _context: &HookContext) -> HookDecision {
            *self.called.lock().unwrap() = true;
            HookDecision::Continue
        }
    }

    #[test]
    fn test_agent_add_hook() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let hook = Arc::new(TestHook::new());
        agent.add_hook(HookEvent::BeforeToolCall, hook.clone());

        let hooks = agent.hooks();
        let before_hooks = hooks.get_hooks(&HookEvent::BeforeToolCall);
        assert_eq!(before_hooks.len(), 1);
    }

    #[test]
    fn test_agent_add_multiple_hooks() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let hook1 = Arc::new(TestHook::new());
        let hook2 = Arc::new(TestHook::new());

        agent.add_hook(HookEvent::BeforeToolCall, hook1);
        agent.add_hook(HookEvent::AfterToolCall, hook2);

        let hooks = agent.hooks();
        assert_eq!(hooks.get_hooks(&HookEvent::BeforeToolCall).len(), 1);
        assert_eq!(hooks.get_hooks(&HookEvent::AfterToolCall).len(), 1);
    }

    // =========================================================================
    // Agent Session Tests
    // =========================================================================

    #[test]
    fn test_agent_session_state_empty() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let state = agent.session_state();
        assert_eq!(state.agent_id, "agent");
        assert_eq!(state.message_log.len(), 0);
        assert_eq!(state.metadata.message_count, 0);
    }

    #[test]
    fn test_agent_add_message() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        agent.add_message(react::llm::LlmMessage::User {
            content: "Hello".to_string(),
        });

        let state = agent.session_state();
        assert_eq!(state.message_log.len(), 1);
        assert_eq!(state.metadata.message_count, 1);
    }

    #[test]
    fn test_agent_add_multiple_messages() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        agent.add_message(react::llm::LlmMessage::User {
            content: "Hello".to_string(),
        });
        agent.add_message(react::llm::LlmMessage::assistant("Hi there!"));

        let state = agent.session_state();
        assert_eq!(state.message_log.len(), 2);
    }

    // =========================================================================
    // Agent Clone Tests
    // =========================================================================

    #[test]
    fn test_agent_clone() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let cloned = agent.clone();
        assert_eq!(cloned.config().name, agent.config().name);
        assert_eq!(cloned.config().model, agent.config().model);
    }

    // =========================================================================
    // Agent Debug Tests
    // =========================================================================

    #[test]
    fn test_agent_debug() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let agent = Agent::new(config, Arc::new(provider));

        let debug_str = format!("{:?}", agent);
        assert!(debug_str.contains("Agent"));
        assert!(debug_str.contains("agent"));
    }

    // =========================================================================
    // Agent Plugin Tests
    // =========================================================================

    struct TestPlugin;

    impl AgentPlugin for TestPlugin {
        fn name(&self) -> &'static str {
            "test-plugin"
        }
    }

    #[test]
    fn test_agent_add_plugin() {
        let config = AgentConfig::default();
        let provider = make_llm_provider();
        let mut agent = Agent::new(config, Arc::new(provider));

        let plugin = Arc::new(TestPlugin);
        agent.add_plugin(plugin);
    }

    // =========================================================================
    // LlmProvider Default Tests
    // =========================================================================

    #[test]
    fn test_llm_provider_default() {
        let _provider = LlmProvider::default();
    }
}

/// Manual Debug implementation that skips the non-Debug llm field.
impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("config", &self.config)
            .field("skills_dir", &self.skills_dir)
            .field("skills", &self.skills)
            .field("resilience", &self.resilience)
            .finish_non_exhaustive()
    }
}
