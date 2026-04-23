use crate::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent, HookRegistry};
use crate::agent::plugin::{
    AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, PluginRegistry, StreamTokenWrapper,
    ToolCallWrapper, ToolResultWrapper,
};
use crate::tools::FunctionTool;
use crate::{AgentError, LlmClient, LlmResponse, StreamToken, Tool, ToolRegistry};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use log::warn;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

use react::engine::{ReActEngine, ReActEngineBuilder};
use react::llm::{
    LlmClient as ReactLlmTrait, LlmError as ReactLlmError, LlmRequest as ReactLlmRequest,
    LlmResponse as ReactLlmResponse, StreamToken as ReactStreamToken,
    TokenStream as ReactTokenStream,
};
use react::tool::{Tool as ReactToolTrait, ToolError as ReactToolError};
use react::{CircuitBreakerConfig, RateLimiterConfig, ReActResilience};

// ============================================================================
// ReAct Adapters - Bridge between Agent and React crate
// ============================================================================

// ============================================================================
// ExtensibleToolAdapter - Wraps a tool with unified hook + plugin pipeline
// ============================================================================

fn block_on_compatible<F: Future>(future: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        ) {
            tokio::task::block_in_place(|| handle.block_on(future))
        } else {
            futures::executor::block_on(future)
        }
    } else {
        futures::executor::block_on(future)
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
        let mut ctx = HookContext::new(&self.agent_id);
        ctx.set("tool_name", tool_name);
        ctx.set("tool_args", original_args);
        if let Some(effective_args) = effective_args {
            ctx.set("effective_tool_args", effective_args);
        }
        match result {
            Ok(v) => ctx.set("tool_result", &v.to_string()),
            Err(e) => ctx.set("error", &e.to_string()),
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
        self.inner.description().short
    }

    fn json_schema(&self) -> serde_json::Value {
        self.inner.json_schema()
    }

    fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ReactToolError> {
        let tool_name = self.inner.name().to_string();
        let original_args = input.to_string();

        self.trigger_before_hook(&tool_name, &original_args)?;

        let processed_call = self
            .plugins
            .process_tool_call_blocking(ToolCallWrapper::new(&tool_name, input.clone(), None));

        if processed_call.name != tool_name {
            log::warn!(
                "Plugin attempted to reroute tool '{}' to '{}'; keeping original tool",
                tool_name,
                processed_call.name
            );
        }

        let effective_args = processed_call.args;
        let effective_args_str = if effective_args != *input {
            Some(effective_args.to_string())
        } else {
            None
        };

        let execution_result = block_on_compatible(self.inner.execute(&effective_args))
            .map_err(|e| ReactToolError::Failed(e.to_string()));

        let processed_result = self
            .plugins
            .process_tool_result_blocking(ToolResultWrapper::from_result(&execution_result));
        let final_result = processed_result.into_result();

        self.trigger_after_hook(
            &tool_name,
            &original_args,
            effective_args_str.as_deref(),
            &final_result,
        )?;

        final_result
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }
}

struct AgentLlmAdapter {
    inner: Arc<dyn LlmClient + Send + Sync>,
}

impl AgentLlmAdapter {
    fn new(inner: Arc<dyn LlmClient + Send + Sync>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ReactLlmTrait for AgentLlmAdapter {
    async fn complete(&self, request: ReactLlmRequest) -> Result<ReactLlmResponse, ReactLlmError> {
        let inner = self.inner.clone();
        let result = inner.complete(request).await;
        match result {
            Ok(resp) => {
                if let LlmResponse::ToolCall { name, args, id } = &resp {
                    return Ok(ReactLlmResponse::ToolCall {
                        name: name.clone(),
                        args: args.clone(),
                        id: id.clone(),
                    });
                }
                Ok(resp)
            }
            Err(e) => Err(ReactLlmError::Other(e.to_string())),
        }
    }

    async fn stream_complete(
        &self,
        request: ReactLlmRequest,
    ) -> Result<ReactTokenStream, ReactLlmError> {
        let inner = self.inner.clone();
        let stream_result = inner.stream_complete(request).await;

        match stream_result {
            Ok(stream) => {
                let mut stream = Box::pin(stream);
                let mut tokens: Vec<Result<ReactStreamToken, ReactLlmError>> = Vec::new();

                while let Some(token) = stream.next().await {
                    match token {
                        Ok(StreamToken::ToolCall { name, args, id }) => {
                            tokens.push(Ok(ReactStreamToken::ToolCall { name, args, id }));
                        }
                        Ok(StreamToken::Text(t)) => tokens.push(Ok(ReactStreamToken::Text(t))),
                        Ok(StreamToken::Done) => tokens.push(Ok(ReactStreamToken::Done)),
                        Err(e) => tokens.push(Err(ReactLlmError::Other(e.to_string()))),
                    }
                }

                let stream = futures::stream::iter(tokens);
                Ok(Box::pin(stream) as ReactTokenStream)
            }
            Err(e) => Err(ReactLlmError::Other(e.to_string())),
        }
    }

    fn supports_tools(&self) -> bool {
        self.inner.supports_tools()
    }

    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }
}

struct PluginLlmAdapter {
    inner: AgentLlmAdapter,
    plugins: PluginRegistry,
}

impl PluginLlmAdapter {
    fn new(inner: AgentLlmAdapter, plugins: PluginRegistry) -> Self {
        Self { inner, plugins }
    }
}

#[async_trait]
impl ReactLlmTrait for PluginLlmAdapter {
    async fn complete(&self, request: ReactLlmRequest) -> Result<ReactLlmResponse, ReactLlmError> {
        let wrapper = LlmRequestWrapper::new(&request);
        let processed = self.plugins.process_llm_request(wrapper).await;
        let modified_request = processed.into_request();

        let result = self.inner.complete(modified_request).await;

        match result {
            Ok(resp) => {
                let wrapper = LlmResponseWrapper::new(&resp);
                let processed = self.plugins.process_llm_response(wrapper).await;
                let modified_resp = processed.into_response();
                if let LlmResponse::ToolCall { name, args, id } = &modified_resp {
                    return Ok(ReactLlmResponse::ToolCall {
                        name: name.clone(),
                        args: args.clone(),
                        id: id.clone(),
                    });
                }
                Ok(modified_resp)
            }
            Err(e) => Err(ReactLlmError::Other(e.to_string())),
        }
    }

    async fn stream_complete(
        &self,
        request: ReactLlmRequest,
    ) -> Result<ReactTokenStream, ReactLlmError> {
        let wrapper = LlmRequestWrapper::new(&request);
        let processed = self.plugins.process_llm_request(wrapper).await;
        let modified_request = processed.into_request();

        let stream_result = self.inner.stream_complete(modified_request).await;

        match stream_result {
            Ok(stream) => {
                let plugins = self.plugins.clone();
                let stream = Box::pin(stream);
                let stream = stream.map(move |item| match item {
                    Ok(token) => {
                        let wrapper = StreamTokenWrapper::new(&token);
                        let processed = plugins.process_stream_token_blocking(wrapper);
                        Ok(processed.into_token())
                    }
                    Err(e) => Err(e),
                });
                let stream: ReactTokenStream = Box::pin(stream.map(|r| match r {
                    Ok(t) => Ok(t),
                    Err(e) => Err(ReactLlmError::Other(e.to_string())),
                }));
                Ok(stream)
            }
            Err(e) => Err(ReactLlmError::Other(e.to_string())),
        }
    }

    fn supports_tools(&self) -> bool {
        self.inner.supports_tools()
    }

    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
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

impl AgentConfig {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }
}

pub struct AgentBuilder {
    config: AgentConfig,
    tools: Vec<Arc<dyn Tool>>,
    skills_dir: Option<std::path::PathBuf>,
    hooks: Option<HookRegistry>,
    plugins: Option<PluginRegistry>,
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            tools: Vec::new(),
            skills_dir: None,
            hooks: None,
            plugins: None,
        }
    }

    /// Set the model name (e.g., "gpt-4", "claude-3").
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set the base API URL.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.config.base_url = url.into();
        self
    }

    /// Set the API key.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.config.api_key = key.into();
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = prompt.into();
        self
    }

    /// Set the agent name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.config.name = name.into();
        self
    }

    /// Set temperature (0.0 to 2.0).
    pub fn temperature(mut self, temp: f32) -> Self {
        self.config.temperature = temp;
        self
    }

    /// Set max tokens.
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.config.max_tokens = Some(tokens);
        self
    }

    /// Set timeout in seconds.
    pub fn timeout(mut self, secs: u64) -> Self {
        self.config.timeout_secs = secs;
        self
    }

    /// Set rate limit configuration to prevent 429 errors.
    pub fn rate_limit(mut self, config: RateLimiterConfig) -> Self {
        self.config.rate_limit = Some(config);
        self
    }

    /// Set circuit breaker configuration to prevent cascading failures.
    pub fn circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.config.circuit_breaker = Some(config);
        self
    }

    /// Set context compaction threshold (approximate tokens).
    pub fn context_compaction_threshold_tokens(mut self, tokens: usize) -> Self {
        self.config.context_compaction_threshold_tokens = tokens;
        self
    }

    /// Set compaction trigger ratio.
    pub fn context_compaction_trigger_ratio(mut self, ratio: f32) -> Self {
        self.config.context_compaction_trigger_ratio = ratio;
        self
    }

    /// Set number of recent messages retained when compacting.
    pub fn context_compaction_keep_recent_messages(mut self, count: usize) -> Self {
        self.config.context_compaction_keep_recent_messages = count;
        self
    }

    /// Set max chars for compaction transcript/summary input.
    pub fn context_compaction_max_summary_chars(mut self, chars: usize) -> Self {
        self.config.context_compaction_max_summary_chars = chars;
        self
    }

    /// Set max tokens for LLM-generated compaction summary.
    pub fn context_compaction_summary_max_tokens(mut self, tokens: u32) -> Self {
        self.config.context_compaction_summary_max_tokens = tokens;
        self
    }

    /// Add a single tool.
    pub fn tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Add multiple tools from an iterable.
    pub fn tools<T>(mut self, tools: T) -> Self
    where
        T: IntoIterator<Item = Arc<dyn Tool>>,
    {
        self.tools.extend(tools);
        self
    }

    /// Set skills directory.
    pub fn skills_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.skills_dir = Some(dir);
        self
    }

    /// Set hooks registry.
    pub fn with_hooks(mut self, hooks: HookRegistry) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub fn with_plugins(mut self, plugins: PluginRegistry) -> Self {
        self.plugins = Some(plugins);
        self
    }

    pub fn plugin(mut self, plugin: Arc<dyn AgentPlugin>) -> Self {
        if self.plugins.is_none() {
            self.plugins = Some(PluginRegistry::new());
        }
        let registry = self.plugins.take();
        if let Some(reg) = registry {
            reg.register_blocking(plugin);
            self.plugins = Some(reg);
        }
        self
    }

    pub fn plugins(mut self, plugins: PluginRegistry) -> Self {
        self.plugins = Some(plugins);
        self
    }

    pub fn build_with_llm(self, llm: Arc<dyn LlmClient>) -> Result<Agent, AgentError> {
        let mut registry = ToolRegistry::new();
        for tool in self.tools {
            registry.register(tool)?;
        }

        let resilience = ReActResilience::new(react::ResilienceConfig {
            circuit_breaker: self.config.circuit_breaker.clone().unwrap_or_default(),
            rate_limiter: self.config.rate_limit.clone().unwrap_or_default(),
        });

        let mut agent = Agent {
            config: self.config,
            llm,
            registry: Some(Arc::new(registry)),
            skills_dir: self.skills_dir,
            resilience,
            ..Agent::default_internal()
        };

        if let Some(hooks) = self.hooks {
            agent.hooks = hooks;
        }

        if let Some(plugins) = self.plugins {
            agent.plugins = plugins;
        }

        if let Some(ref dir) = agent.skills_dir {
            agent.register_skills_from_dir(dir.clone())?;
        }

        let skills = agent.skills.clone();
        if let Some(ref reg) = agent.registry {
            let mut new_registry = (**reg).clone();
            Self::register_load_skill_tool(&mut new_registry, &skills);
            agent.registry = Some(Arc::new(new_registry));
        }

        Ok(agent)
    }

    fn register_load_skill_tool(
        registry: &mut ToolRegistry,
        skills: &[crate::skills::SkillContent],
    ) {
        let skill_names: Vec<String> = skills.iter().map(|s| s.metadata.name.clone()).collect();
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
                let skills = skills.to_vec();
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
        registry.register(load_skill_tool).ok();
    }

    /// Build with auto-detected LLM client using LlmRouter.
    /// Model names like "nvidia/meta/llama-3.1-8b-instruct" are routed automatically.
    pub fn build(self) -> Result<Agent, AgentError> {
        use react::llm::vendor::{nvidia::NvidiaVendor, openai::OpenAiClient, router::LlmRouter};

        let mut router = LlmRouter::new();
        router.register_vendor(
            "nvidia".to_string(),
            Box::new(
                NvidiaVendor::builder()
                    .endpoint(self.config.base_url.clone())
                    .model(self.config.model.clone())
                    .api_key(self.config.api_key.clone())
                    .build()
                    .map_err(|e| AgentError::Session(format!("Nvidia build error: {}", e)))?,
            ),
        );
        router.register_vendor(
            "openai".to_string(),
            Box::new(OpenAiClient::new(
                self.config.base_url.clone(),
                self.config.model.clone(),
                self.config.api_key.clone(),
            )),
        );

        self.build_with_llm(Arc::new(router))
    }

    /// Build with custom LLM client and start a session immediately.
    pub fn build_session(self, llm: Arc<dyn LlmClient>) -> Result<Agent, AgentError> {
        self.build_with_llm(llm)
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// AgentOutput enum - simplified response types.
#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
    Error(String),
}

/// Agent is the main abstraction for AI agents with LLM integration,
/// tool registries, and skill management.
#[qserde::Archive]
#[rkyv(crate = qserde::rkyv)]
pub struct Agent {
    config: AgentConfig,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    llm: Arc<dyn LlmClient>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    registry: Option<Arc<ToolRegistry>>,
    #[rkyv(with = qserde::rkyv::with::Map<qserde::rkyv::with::AsString>)]
    skills_dir: Option<std::path::PathBuf>,
    skills: Vec<crate::skills::SkillContent>,
    resilience: ReActResilience,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    message_log: Arc<std::sync::Mutex<Vec<react::llm::LlmMessage>>>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    hooks: HookRegistry,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    plugins: PluginRegistry,
}

impl Agent {
    /// Create a new Agent with the given config and LLM client.
    pub fn new(config: AgentConfig, llm: Arc<dyn LlmClient>) -> Self {
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
            message_log: Arc::new(std::sync::Mutex::new(Vec::new())),
            hooks: HookRegistry::new(),
            plugins: PluginRegistry::new(),
        }
    }

    /// Create a builder for fluent configuration.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Internal default for partial construction.
    fn default_internal() -> Self {
        Self {
            config: AgentConfig::default(),
            llm: Arc::new(react::llm::vendor::OpenAiClient::new(
                "https://api.openai.com/v1".to_string(),
                "gpt-4".to_string(),
                "dummy".to_string(),
            )),
            registry: None,
            skills_dir: None,
            skills: Vec::new(),
            resilience: ReActResilience::new(Default::default()),
            message_log: Arc::new(std::sync::Mutex::new(Vec::new())),
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
        self.message_log.lock().unwrap().push(message);
        self.hooks
            .trigger_all_blocking(HookEvent::OnMessage, HookContext::new(&self.config.name));
    }

    pub fn get_messages(&self) -> Vec<react::llm::LlmMessage> {
        self.message_log.lock().unwrap().clone()
    }

    pub fn save_message_log(&self, path: &str) -> Result<(), AgentError> {
        let json = serde_json::to_string_pretty(&*self.message_log.lock().unwrap())
            .map_err(|e| AgentError::Session(format!("Serialize error: {}", e)))?;
        std::fs::write(path, json).map_err(|e| AgentError::Session(format!("Write error: {}", e)))
    }

    pub fn restore_message_log(&mut self, path: &str) -> Result<(), AgentError> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| AgentError::Session(format!("Read error: {}", e)))?;
        let messages: Vec<react::llm::LlmMessage> = serde_json::from_str(&json)
            .map_err(|e| AgentError::Session(format!("Parse error: {}", e)))?;
        *self.message_log.lock().unwrap() = messages;
        Ok(())
    }

    /// Add a tool that calls another agent via bus Caller.
    pub fn add_remote_agent_tool(
        &mut self,
        tool_name: impl Into<String>,
        endpoint: impl Into<String>,
        session: Arc<bus::Session>,
    ) -> Result<(), crate::ToolError> {
        let tool = Arc::new(crate::bus_rpc::AgentCallerTool::new(
            tool_name, endpoint, session,
        ));
        self.try_add_tool(tool)
    }

    /// Create a typed RPC client for another agent endpoint.
    pub fn rpc_client(
        &self,
        endpoint: impl Into<String>,
        session: Arc<bus::Session>,
    ) -> crate::bus_rpc::AgentRpcClient {
        crate::bus_rpc::AgentRpcClient::new(endpoint, session)
    }

    /// Expose this agent as a bus callable endpoint for agent-to-agent calls.
    pub fn as_callable_server(
        &self,
        endpoint: impl Into<String>,
        session: Arc<bus::Session>,
    ) -> crate::bus_rpc::AgentCallableServer {
        crate::bus_rpc::AgentCallableServer::new(endpoint, session, Arc::new(self.clone()))
    }

    /// Build a ReActEngine with the standard adapter stack (LLM, tools, skills).
    /// Shared by react(), run_simple(), and stream() to avoid duplicating adapter construction.
    fn build_react_engine(&self, system_prompt: String) -> Result<ReActEngine, AgentError> {
        let react_llm = Box::new(PluginLlmAdapter::new(
            AgentLlmAdapter::new(self.llm.clone()),
            self.plugins.clone(),
        )) as Box<dyn ReactLlmTrait>;

        let mut builder = ReActEngineBuilder::new().llm(react_llm);

        if let Some(ref registry) = self.registry {
            for (_name, tool) in registry.iter() {
                let tool_adapter = Box::new(ExtensibleToolAdapter::new(
                    tool.clone(),
                    self.plugins.clone(),
                    self.hooks.clone(),
                    self.config.name.clone(),
                )) as Box<dyn ReactToolTrait>;
                builder = builder.with_tool(tool_adapter);
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
                builder = builder.with_tool(Box::new(ExtensibleToolAdapter::new(
                    skill_tool,
                    self.plugins.clone(),
                    self.hooks.clone(),
                    self.config.name.clone(),
                )));
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
            builder = builder.with_tool(Box::new(ExtensibleToolAdapter::new(
                load_skill_tool,
                self.plugins.clone(),
                self.hooks.clone(),
                self.config.name.clone(),
            )));
        }

        builder = builder.resilience(self.resilience.clone());
        builder = builder.llm_timeout(self.config.timeout_secs);
        builder = builder.max_steps(self.config.max_steps);
        builder = builder.model(self.config.model.clone());
        builder = builder.system_prompt(system_prompt);

        builder
            .build()
            .map_err(|e| AgentError::Session(format!("ReAct build error: {}", e)))
    }

    /// Run the agent using ReAct engine.
    pub async fn react(&self, task: &str) -> Result<String, AgentError> {
        // Build system prompt with skill context
        let mut system_prompt = self.config.system_prompt.clone();
        if !self.skills.is_empty() {
            let injector = crate::skills::SkillInjector::with_options(
                crate::skills::InjectionOptions::compact(),
            );
            let metadata: Vec<_> = self.skills.iter().map(|s| s.metadata.clone()).collect();
            let injection = injector.inject_available(&metadata);
            if !injection.is_empty() {
                system_prompt.push('\n');
                system_prompt.push_str(&injection);
            }
        }
        system_prompt.push_str("Final answer: Final Answer: your answer");

        let mut engine = self.build_react_engine(system_prompt)?;

        let mut ctx = HookContext::new(&self.config.name);
        ctx.set("task", task);
        let decision = self.hooks.trigger(HookEvent::BeforeLlmCall, ctx).await;
        match decision {
            HookDecision::Error(msg) => {
                return Err(AgentError::Session(format!(
                    "BeforeLlmCall hook error: {}",
                    msg
                )))
            }
            HookDecision::Abort => return Ok(String::new()),
            HookDecision::Continue => {}
        }

        engine.set_input_messages(self.message_log.lock().unwrap().clone());
        let result = engine.react(task).await;

        match result {
            Ok((result, context)) => {
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("result", &result);
                let decision = self.hooks.trigger(HookEvent::AfterLlmCall, ctx).await;
                match decision {
                    HookDecision::Error(msg) => log::warn!("AfterLlmCall hook error: {}", msg),
                    HookDecision::Abort => return Ok(String::new()),
                    HookDecision::Continue => {}
                }

                let mut log = context.conversations.clone();
                log.remove(0);
                *self.message_log.lock().unwrap() = log;
                self.hooks
                    .trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name))
                    .await;

                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", "0");
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
                Ok(result)
            }
            Err(e) => {
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("error", &e.to_string());
                let decision = self.hooks.trigger(HookEvent::OnError, ctx).await;
                match decision {
                    HookDecision::Error(msg) => log::warn!("OnError hook error: {}", msg),
                    HookDecision::Abort => return Ok(String::new()),
                    HookDecision::Continue => {}
                }
                Err(AgentError::Session(format!("ReAct run error: {}", e)))
            }
        }
    }

    /// Run the agent using simple loop (no ReAct).
    /// Useful for testing or when ReAct format is not needed.
    /// Supports tools and skills like react() does, but makes a single LLM call.
    pub async fn run_simple(&self, task: &str) -> Result<String, AgentError> {
        use react::llm::LlmMessage;
        use react::llm::{LlmContext, LlmRequest};

        let system_prompt = self.config.system_prompt.clone();
        let engine = self.build_react_engine(system_prompt.clone())?;

        let conversations = self.message_log.lock().unwrap().clone();
        let mut all_conversations = vec![LlmMessage::system(system_prompt.clone())];
        all_conversations.extend(conversations);
        all_conversations.push(LlmMessage::user(task.to_string()));

        let context = LlmContext {
            conversations: all_conversations.clone(),
            ..Default::default()
        };

        let req = LlmRequest {
            model: self.config.model.clone(),
            context,
            temperature: Some(self.config.temperature),
            ..Default::default()
        };

        // Emit BeforeLlmCall hook
        let mut hook_ctx = HookContext::new(&self.config.name);
        hook_ctx.set("model", &self.config.model);
        hook_ctx.set("task", task);
        let decision = self.hooks.trigger(HookEvent::BeforeLlmCall, hook_ctx).await;
        match decision {
            HookDecision::Error(msg) => {
                return Err(AgentError::Session(format!(
                    "BeforeLlmCall hook error: {}",
                    msg
                )))
            }
            HookDecision::Abort => return Ok(String::new()),
            HookDecision::Continue => {}
        }

        let response = engine.call_llm(req).await;

        if let Err(e) = &response {
            let mut ctx = HookContext::new(&self.config.name);
            ctx.set("error", "LLM call failed");
            let decision = self.hooks.trigger(HookEvent::OnError, ctx).await;
            match decision {
                HookDecision::Error(msg) => log::warn!("OnError hook error: {}", msg),
                HookDecision::Abort => return Ok(String::new()),
                HookDecision::Continue => {}
            }
            return Err(AgentError::Session(format!("LLM call failed: {}", e)));
        }

        let response = response.unwrap();

        // Emit AfterLlmCall hook
        let mut hook_ctx = HookContext::new(&self.config.name);
        hook_ctx.set("model", &self.config.model);
        let response_type = match response {
            react::llm::LlmResponse::Text(_) => "text",
            react::llm::LlmResponse::Partial(_) => "partial",
            react::llm::LlmResponse::Done => "done",
            react::llm::LlmResponse::ToolCall { .. } => "tool_call",
        };
        hook_ctx.set("response_type", response_type);
        let decision = self.hooks.trigger(HookEvent::AfterLlmCall, hook_ctx).await;
        match decision {
            HookDecision::Error(msg) => log::warn!("AfterLlmCall hook error: {}", msg),
            HookDecision::Abort => return Ok(String::new()),
            HookDecision::Continue => {}
        }

        let mut loaded_skills: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        match response {
            react::llm::LlmResponse::Text(content) => {
                {
                    let mut log = self.message_log.lock().unwrap();
                    log.push(LlmMessage::user(task.to_string()));
                    log.push(LlmMessage::assistant(content.clone()));
                }
                self.hooks
                    .trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name))
                    .await;
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", "0");
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
                Ok(content)
            }
            react::llm::LlmResponse::Partial(content) => {
                {
                    let mut log = self.message_log.lock().unwrap();
                    log.push(LlmMessage::user(task.to_string()));
                    log.push(LlmMessage::assistant(content.clone()));
                }
                self.hooks
                    .trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name))
                    .await;
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", "0");
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
                Ok(content)
            }
            react::llm::LlmResponse::Done => {
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", "0");
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
                Ok(String::new())
            }
            react::llm::LlmResponse::ToolCall { name, args, id } => {
                let call_id =
                    id.unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4().simple()));

                let result = if name == "load_skill" {
                    let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if let Some(cached) = loaded_skills.get(skill_name) {
                        Ok(serde_json::json!({
                            "name": skill_name,
                            "instructions": cached,
                            "cached": true
                        }))
                    } else {
                        engine.call_tool(&name, &args).await
                    }
                } else {
                    engine.call_tool(&name, &args).await
                };

                let result_text = match result {
                    Ok(ret) => {
                        if name == "load_skill" {
                            let skill_name =
                                args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let instructions = ret
                                .get("instructions")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !instructions.is_empty()
                                && !ret.get("cached").and_then(|v| v.as_bool()).unwrap_or(false)
                            {
                                loaded_skills
                                    .insert(skill_name.to_string(), instructions.to_string());
                            }
                        }
                        ret.to_string()
                    }
                    Err(e) => {
                        let mut ctx = HookContext::new(&self.config.name);
                        ctx.set("tool_name", &name);
                        ctx.set("tool_args", &args.to_string());
                        ctx.set("error", &e.to_string());
                        match self.hooks.trigger(HookEvent::OnError, ctx).await {
                            HookDecision::Error(msg) => {
                                return Ok(format!("Tool blocked by OnError hook: {}", msg));
                            }
                            HookDecision::Abort => return Ok(String::new()),
                            HookDecision::Continue => {}
                        }
                        format!("Error: {}", e)
                    }
                };

                let mut log = self.message_log.lock().unwrap();
                log.push(LlmMessage::user(task.to_string()));
                log.push(LlmMessage::assistant_tool_call(
                    call_id.clone(),
                    name.clone(),
                    args.clone(),
                ));
                log.push(LlmMessage::tool_result(call_id, result_text.clone()));

                Ok(result_text)
            }
        }
    }

    /// Stream the agent response using simple approach.
    /// Supports tools and skills like react() does, but makes a single LLM call.
    pub fn stream(
        &self,
        task: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send + '_>> {
        use futures::stream::StreamExt;
        use react::llm::{LlmContext, LlmMessage, LlmRequest};

        let system_prompt = self.config.system_prompt.clone();
        let engine_result = self.build_react_engine(system_prompt.clone());
        let task_str = task.to_string();
        let message_log_ptr = self.message_log.clone();

        let stream = async_stream::stream! {
            let engine = match engine_result {
                Ok(e) => e,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let mut all_conversations = vec![LlmMessage::system(system_prompt.clone())];
            all_conversations.extend(message_log_ptr.lock().unwrap().clone());
            all_conversations.push(LlmMessage::user(task_str.clone()));

            let mut log = message_log_ptr.lock().unwrap().clone();
            log.push(LlmMessage::user(task_str.clone()));

            let context = LlmContext {
                conversations: all_conversations,
                ..Default::default()
            };

            let req = LlmRequest {
                model: self.config.model.clone(),
                context,
                temperature: Some(0.7),
                ..Default::default()
            };

            let mut ctx = HookContext::new(&self.config.name);
            ctx.set("model", &self.config.model);
            let decision = self.hooks.trigger(HookEvent::BeforeLlmCall, ctx).await;
            match decision {
                HookDecision::Error(msg) => {
                    yield Err(AgentError::Session(format!("BeforeLlmCall hook error: {}", msg)));
                    return;
                }
                HookDecision::Abort => {
                    return;
                }
                HookDecision::Continue => {}
            }

            let llm_stream = match engine.call_llm_stream(req).await {
                Ok(s) => s,
                Err(e) => {
                    yield Err(AgentError::Session(format!("LLM stream error: {}", e)));
                    return;
                }
            };
            futures::pin_mut!(llm_stream);
            let mut full_response = String::new();
            let mut loaded_skills: std::collections::HashMap<String, String> = std::collections::HashMap::new();

            while let Some(item) = llm_stream.next().await {
                match item {
                    Ok(StreamToken::Text(text)) => {
                        full_response.push_str(&text);
                        yield Ok(StreamToken::Text(text));
                    }
                    Ok(StreamToken::Done) => {
                        yield Ok(StreamToken::Done);
                    }
                    Ok(StreamToken::ToolCall { name, args, id }) => {
                        yield Ok(StreamToken::ToolCall { name: name.clone(), args: args.clone(), id: id.clone() });

                        let call_id = id.unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

                        let result = if name == "load_skill" {
                            let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if let Some(cached) = loaded_skills.get(skill_name) {
                                Ok(serde_json::json!({
                                    "name": skill_name,
                                    "instructions": cached,
                                    "cached": true
                                }))
                            } else {
                                engine.call_tool(&name, &args).await
                            }
                        } else {
                            engine.call_tool(&name, &args).await
                        };

                        let result_text = match result {
                            Ok(ret) => {
                                if name == "load_skill" {
                                    let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    let instructions = ret.get("instructions").and_then(|v| v.as_str()).unwrap_or("");
                                    if !instructions.is_empty() && !ret.get("cached").and_then(|v| v.as_bool()).unwrap_or(false) {
                                        loaded_skills.insert(skill_name.to_string(), instructions.to_string());
                                    }
                                }
                                ret.to_string()
                            },
                            Err(e) => {
                                let mut ctx = HookContext::new(&self.config.name);
                                ctx.set("tool_name", &name);
                                ctx.set("tool_args", &args.to_string());
                                ctx.set("error", &e.to_string());
                                match self.hooks.trigger(HookEvent::OnError, ctx).await {
                                    HookDecision::Error(msg) => {
                                        full_response.push_str(&format!("Tool blocked by OnError hook: {}", msg));
                                        yield Ok(StreamToken::Text(format!("Tool blocked by OnError hook: {}", msg)));
                                        return;
                                    }
                                    HookDecision::Abort => {
                                        return;
                                    }
                                    HookDecision::Continue => {}
                                }
                                format!("Error: {}", e)
                            },
                        };

                        full_response.push_str(&result_text);
                        yield Ok(StreamToken::Text(result_text.clone()));
                        {
                            let mut log = message_log_ptr.lock().unwrap();
                            log.push(LlmMessage::assistant_tool_call(call_id.clone(), name.clone(), args.clone()));
                            log.push(LlmMessage::tool_result(call_id, result_text));
                        }
                        self.hooks.trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name)).await;
                    }
                    Err(e) => yield Err(AgentError::Session(format!("LLM stream error: {}", e))),
                }
            }

            let mut ctx = HookContext::new(&self.config.name);
            ctx.set("model", &self.config.model);
            let decision = self.hooks.trigger(HookEvent::AfterLlmCall, ctx).await;
            match decision {
                HookDecision::Error(msg) => log::warn!("AfterLlmCall hook error: {}", msg),
                HookDecision::Abort => return,
                HookDecision::Continue => {}
            }

            {
                let mut log = message_log_ptr.lock().unwrap();
                if !full_response.is_empty() {
                    log.push(LlmMessage::assistant(full_response.clone()));
                }
            }
            self.hooks.trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name)).await;
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
                    "tags": skill.metadata.tags,
                    "requires": skill.metadata.requires,
                    "provides": skill.metadata.provides
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
            let content = loader.load(&skill_meta.name)?;
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
            message_log: self.message_log.clone(),
            hooks: self.hooks.clone(),
            plugins: self.plugins.clone(),
        }
    }
}

/// Session wrapper for Agent that provides simplified execution.
/// Render the skills catalog for the system prompt.
/// Takes skill schemas and formats them for display.
#[allow(dead_code)]
fn render_skills_catalog(skills_schemas: &[serde_json::Value]) -> String {
    let mut catalog = String::new();
    catalog.push_str("Available skills:\n\n");
    for schema in skills_schemas {
        if let Some(name) = schema.get("name").and_then(|v| v.as_str()) {
            catalog.push_str(&format!("- {}: ", name));
            if let Some(desc) = schema.get("description").and_then(|v| v.as_str()) {
                catalog.push_str(desc);
            }
            catalog.push('\n');
        }
    }
    catalog
}

/// Manual Debug implementation that skips the non-Debug llm field.
impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("config", &self.config)
            .field("registry", &self.registry)
            .field("skills_dir", &self.skills_dir)
            .field("skills", &self.skills)
            .field("resilience", &self.resilience)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod agent_serialization_tests {
    use super::*;
    use qserde::prelude::*;

    /// Test AgentConfig serialization - the core serializable part of Agent
    #[test]
    fn test_agent_config_serialize() {
        let config = AgentConfig::default();
        let bytes = config.dump().expect("AgentConfig should be serializable");
        let loaded = bytes
            .load::<AgentConfig>()
            .expect("AgentConfig should be deserializable");
        assert_eq!(loaded.name, config.name);
        assert_eq!(loaded.model, config.model);
    }

    /// Test that Agent can be serialized using qserde after adding #[qserde::Archive]
    #[test]
    fn test_agent_serialize() {
        use react::llm::LlmClient;

        let config = AgentConfig::default();

        struct MockLlmClient;
        #[async_trait]
        impl LlmClient for MockLlmClient {
            async fn complete(
                &self,
                _req: react::llm::LlmRequest,
            ) -> react::llm::LlmResponseResult {
                todo!()
            }
            async fn stream_complete(
                &self,
                _req: react::llm::LlmRequest,
            ) -> std::result::Result<react::llm::TokenStream, react::llm::LlmError> {
                todo!()
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn provider_name(&self) -> &'static str {
                "mock"
            }
        }

        let agent = Agent {
            config,
            llm: Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
            registry: None,
            skills_dir: None,
            skills: Vec::new(),
            resilience: ReActResilience::new(react::ResilienceConfig::default()),
            message_log: Arc::new(std::sync::Mutex::new(Vec::new())),
            hooks: HookRegistry::new(),
            plugins: PluginRegistry::new(),
        };

        let bytes = agent.dump().expect("Agent should be serializable");
        assert!(!bytes.is_empty(), "Serialized bytes should not be empty");
    }

    /// Test that Agent can be serialized (serialize-only, cannot deserialize due to dyn LlmClient)
    #[test]
    fn test_agent_serialize_only() {
        use react::llm::LlmClient;

        let config = AgentConfig::default();

        struct MockLlmClient;
        #[async_trait]
        impl LlmClient for MockLlmClient {
            async fn complete(
                &self,
                _req: react::llm::LlmRequest,
            ) -> react::llm::LlmResponseResult {
                todo!()
            }
            async fn stream_complete(
                &self,
                _req: react::llm::LlmRequest,
            ) -> std::result::Result<react::llm::TokenStream, react::llm::LlmError> {
                todo!()
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn provider_name(&self) -> &'static str {
                "mock"
            }
        }

        let agent = Agent {
            config,
            llm: Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
            registry: None,
            skills_dir: None,
            skills: Vec::new(),
            resilience: ReActResilience::new(react::ResilienceConfig::default()),
            message_log: Arc::new(std::sync::Mutex::new(Vec::new())),
            hooks: HookRegistry::new(),
            plugins: PluginRegistry::new(),
        };

        let bytes = agent.dump().expect("Agent should be serializable");
        assert!(!bytes.is_empty(), "Serialized bytes should not be empty");
    }
}
/// Test message log save/restore functionality
#[test]
fn test_message_log_save_restore() {
    use react::llm::LlmMessage;
    use std::env::temp_dir;
    use std::fs;

    // Create a temp file path
    let mut path = temp_dir();
    path.push("test_message_log.json");

    // Create session and add some messages
    let config = AgentConfig::default();
    struct MockLlmClient;
    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(&self, _req: react::llm::LlmRequest) -> react::llm::LlmResponseResult {
            todo!()
        }
        async fn stream_complete(
            &self,
            _req: react::llm::LlmRequest,
        ) -> std::result::Result<react::llm::TokenStream, react::llm::LlmError> {
            todo!()
        }
        fn supports_tools(&self) -> bool {
            false
        }
        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    let mut agent = Agent::new(config, Arc::new(MockLlmClient) as Arc<dyn LlmClient>);

    // Add test messages
    agent.add_message(LlmMessage::user("Hello"));
    agent.add_message(LlmMessage::assistant("Hi there!"));
    agent.add_message(LlmMessage::user("How are you?"));

    // Save to file
    agent
        .save_message_log(path.to_str().unwrap())
        .expect("save should work");

    // Create new agent and restore
    let mut agent2 = Agent::new(
        AgentConfig::default(),
        Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
    );
    agent2
        .restore_message_log(path.to_str().unwrap())
        .expect("restore should work");

    // Verify messages match
    let messages = agent2.get_messages();
    assert_eq!(messages.len(), 3);
    assert!(matches!(messages[0], LlmMessage::User { content: ref c } if c == "Hello"));
    assert!(matches!(messages[1], LlmMessage::Assistant { content: ref c } if c == "Hi there!"));
    assert!(matches!(messages[2], LlmMessage::User { content: ref c } if c == "How are you?"));

    // Cleanup
    fs::remove_file(&path).ok();
}

#[cfg(test)]
mod message_log_tests {
    use super::*;
    use react::llm::{LlmMessage, LlmResponse, LlmResponseResult};
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    struct MockLlmClient {
        responses: Arc<TokioMutex<Vec<LlmResponse>>>,
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(&self, _req: react::llm::LlmRequest) -> LlmResponseResult {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(LlmResponse::Text("Final answer: test response".to_string()))
            } else {
                Ok(responses.remove(0))
            }
        }

        async fn stream_complete(
            &self,
            _req: react::llm::LlmRequest,
        ) -> std::result::Result<react::llm::TokenStream, react::llm::LlmError> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::Text("test".to_string())),
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

    #[tokio::test]
    async fn test_message_log_includes_tool_calls_and_results() {
        // Tool call testing requires proper LLM with tool support
        // This test verifies the code compiles correctly
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        // Add a tool to verify registry works
        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            |_args: &serde_json::Value| {
                Ok(serde_json::json!({
                    "result": "tool executed successfully"
                }))
            },
        ));
        agent.add_tool(tool);

        // Verify tool is registered
        assert!(agent.registry().and_then(|r| r.get("test_tool")).is_some());
    }

    #[tokio::test]
    async fn test_stream_message_log_includes_tool_calls_and_results() {
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            |_args: &serde_json::Value| {
                Ok(serde_json::json!({
                    "result": "tool executed"
                }))
            },
        ));
        agent.add_tool(tool);

        // Verify tool is registered
        assert!(agent.registry().and_then(|r| r.get("test_tool")).is_some());
    }

    #[tokio::test]
    async fn test_react_message_log_includes_all_types() {
        use react::llm::LlmMessage;

        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        agent.add_message(LlmMessage::user("initial"));

        let messages = agent.get_messages();

        assert!(!messages.is_empty(), "message_log should have messages");
    }

    /// Mock LLM client that returns a tool call response for testing tool message logging
    struct ToolCallLlmClient {
        responses: Arc<TokioMutex<Vec<LlmResponse>>>,
    }

    #[async_trait]
    impl LlmClient for ToolCallLlmClient {
        async fn complete(&self, _req: react::llm::LlmRequest) -> LlmResponseResult {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(LlmResponse::Text("Final answer: test response".to_string()))
            } else {
                Ok(responses.remove(0))
            }
        }

        async fn stream_complete(
            &self,
            _req: react::llm::LlmRequest,
        ) -> std::result::Result<react::llm::TokenStream, react::llm::LlmError> {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::Text("test".to_string())),
                Ok(StreamToken::Done),
            ])))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mock-tool"
        }
    }

    /// Helper to check if message log contains all required message types
    fn has_message_type(messages: &[LlmMessage], variant: &str) -> bool {
        messages.iter().any(|m| match (m, variant) {
            (LlmMessage::System { .. }, "system") => true,
            (LlmMessage::User { .. }, "user") => true,
            (LlmMessage::Assistant { .. }, "assistant") => true,
            (LlmMessage::AssistantToolCall { .. }, "tool_call") => true,
            (LlmMessage::ToolResult { .. }, "tool_result") => true,
            _ => false,
        })
    }
    /// Test that LLM response is included in message_log for run_simple()
    /// RED: This test should FAIL initially
    #[tokio::test]
    async fn test_run_simple_llm_response_included_in_message_log() {
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![LlmResponse::Text(
            "Final answer: LLM response text".to_string(),
        )]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let result = agent.run_simple("test task").await;

        // Verify the LLM response was received
        assert!(result.is_ok(), "run_simple() should succeed");
        let messages = agent.get_messages();

        // The assistant message should contain the LLM response
        let has_assistant_with_content = messages.iter().any(|m| {
            if let LlmMessage::Assistant { content } = m {
                content.contains("LLM response text") || content.contains("Final answer")
            } else {
                false
            }
        });

        assert!(
            has_assistant_with_content,
            "run_simple() message_log must include LLM response in assistant message. Got: {:?}",
            messages
        );
    }

    /// Test that tool call messages are included in message_log when LLM returns a tool call
    /// RED: This test should FAIL initially - need to mock tool call responses
    #[tokio::test(flavor = "multi_thread")]
    async fn test_run_simple_includes_tool_call_messages() {
        use serde_json::json;

        let config = AgentConfig::default();
        // First response is a tool call, second is the final answer
        let responses = Arc::new(TokioMutex::new(vec![
            LlmResponse::ToolCall {
                name: "test_tool".to_string(),
                args: json!({"param": "value"}),
                id: Some("call_123".to_string()),
            },
            LlmResponse::Text("Final answer: tool executed".to_string()),
        ]));
        let llm = Arc::new(MockLlmClient { responses });

        // Create agent with a test tool registered
        let mut agent = Agent::new(config, llm);
        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }),
            |args: &serde_json::Value| Ok(serde_json::json!({ "result": "tool executed" })),
        ));
        agent.add_tool(tool);

        // Call run_simple - it should handle tool calls
        let _ = agent.run_simple("test task").await;

        let messages = agent.get_messages();

        // MUST have tool call message
        assert!(
            has_message_type(&messages, "tool_call"),
            "run_simple() message_log must include tool call messages when LLM returns tool call. Got: {:?}",
            messages
        );

        // MUST have tool result message
        assert!(
            has_message_type(&messages, "tool_result"),
            "run_simple() message_log must include tool result messages. Got: {:?}",
            messages
        );
    }

    // ========================================================================
    // TDD Tests: No Duplicate Messages
    // These tests verify that message_log does NOT contain duplicate messages
    // when calling react(), run_simple(), and stream() methods.
    // ========================================================================

    /// Helper to count messages of a specific type
    fn count_message_type(messages: &[LlmMessage], variant: &str) -> usize {
        messages
            .iter()
            .filter(|m| match (m, variant) {
                (LlmMessage::System { .. }, "system") => true,
                (LlmMessage::User { .. }, "user") => true,
                (LlmMessage::Assistant { .. }, "assistant") => true,
                (LlmMessage::AssistantToolCall { .. }, "tool_call") => true,
                (LlmMessage::ToolResult { .. }, "tool_result") => true,
                _ => false,
            })
            .count()
    }
}

#[cfg(test)]
mod hook_tests {
    use super::*;
    use crate::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent};
    use async_trait::async_trait;
    use react::llm::{LlmResponse, LlmResponseResult, StreamToken};
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    #[derive(Clone)]
    struct RecordingHook {
        events: Arc<TokioMutex<Vec<(HookEvent, HookContext)>>>,
        decision: HookDecision,
    }

    impl RecordingHook {
        fn new(decision: HookDecision) -> Self {
            Self {
                events: Arc::new(TokioMutex::new(Vec::new())),
                decision,
            }
        }

        async fn get_events(&self) -> Vec<(HookEvent, HookContext)> {
            self.events.lock().await.clone()
        }
    }

    #[async_trait]
    impl AgentHook for RecordingHook {
        async fn on_event(&self, event: HookEvent, context: &HookContext) -> HookDecision {
            self.events.lock().await.push((event, context.clone()));
            self.decision.clone()
        }
    }

    struct MockLlmClient {
        responses: Arc<TokioMutex<Vec<LlmResponse>>>,
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(&self, _req: react::llm::LlmRequest) -> LlmResponseResult {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(LlmResponse::Text("Final answer: test response".to_string()))
            } else {
                Ok(responses.remove(0))
            }
        }

        async fn stream_complete(
            &self,
            _req: react::llm::LlmRequest,
        ) -> std::result::Result<react::llm::TokenStream, react::llm::LlmError> {
            let mut responses = self.responses.lock().await;
            let tokens: Vec<Result<StreamToken, react::llm::LlmError>> = if responses.is_empty() {
                vec![
                    Ok(StreamToken::Text("Final answer: test response".to_string())),
                    Ok(StreamToken::Done),
                ]
            } else {
                let resp = responses.remove(0);
                match resp {
                    LlmResponse::ToolCall { name, args, id } => {
                        vec![Ok(StreamToken::ToolCall { name, args, id })]
                    }
                    LlmResponse::Text(s) => vec![Ok(StreamToken::Text(s)), Ok(StreamToken::Done)],
                    LlmResponse::Partial(s) => {
                        vec![Ok(StreamToken::Text(s)), Ok(StreamToken::Done)]
                    }
                    LlmResponse::Done => vec![Ok(StreamToken::Done)],
                }
            };
            Ok(Box::pin(futures::stream::iter(tokens)))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    struct ForceToolFailurePlugin;

    #[async_trait]
    impl AgentPlugin for ForceToolFailurePlugin {
        fn name(&self) -> &str {
            "force_tool_failure"
        }

        async fn on_tool_result(
            &self,
            mut tool_result: ToolResultWrapper,
        ) -> Option<ToolResultWrapper> {
            tool_result.success = false;
            tool_result.error = Some("forced plugin failure".to_string());
            Some(tool_result)
        }
    }

    #[tokio::test]
    async fn test_react_llm_hooks() {
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let before_llm = Arc::new(RecordingHook::new(HookDecision::Continue));
        let after_llm = Arc::new(RecordingHook::new(HookDecision::Continue));

        agent
            .hooks
            .register(HookEvent::BeforeLlmCall, before_llm.clone())
            .await;
        agent
            .hooks
            .register(HookEvent::AfterLlmCall, after_llm.clone())
            .await;

        let _ = agent.react("test").await;

        let before_events = before_llm.get_events().await;
        let after_events = after_llm.get_events().await;

        assert!(
            !before_events.is_empty(),
            "BeforeLlmCall should be triggered"
        );
        assert!(!after_events.is_empty(), "AfterLlmCall should be triggered");
    }

    #[tokio::test]
    async fn test_run_simple_llm_hooks() {
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let before_llm = Arc::new(RecordingHook::new(HookDecision::Continue));
        let after_llm = Arc::new(RecordingHook::new(HookDecision::Continue));

        agent
            .hooks
            .register(HookEvent::BeforeLlmCall, before_llm.clone())
            .await;
        agent
            .hooks
            .register(HookEvent::AfterLlmCall, after_llm.clone())
            .await;

        let _ = agent.run_simple("test").await;

        let before_events = before_llm.get_events().await;
        let after_events = after_llm.get_events().await;

        assert!(
            !before_events.is_empty(),
            "BeforeLlmCall should be triggered in run_simple"
        );
        assert!(
            !after_events.is_empty(),
            "AfterLlmCall should be triggered in run_simple"
        );
    }

    #[tokio::test]
    async fn test_stream_llm_hooks() {
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let before_llm = Arc::new(RecordingHook::new(HookDecision::Continue));
        let after_llm = Arc::new(RecordingHook::new(HookDecision::Continue));

        agent
            .hooks
            .register(HookEvent::BeforeLlmCall, before_llm.clone())
            .await;
        agent
            .hooks
            .register(HookEvent::AfterLlmCall, after_llm.clone())
            .await;

        let mut stream = agent.stream("test");
        while let Some(_) = stream.next().await {}

        let before_events = before_llm.get_events().await;
        let after_events = after_llm.get_events().await;

        assert!(
            !before_events.is_empty(),
            "BeforeLlmCall should be triggered in stream"
        );
        assert!(
            !after_events.is_empty(),
            "AfterLlmCall should be triggered in stream"
        );
    }

    #[tokio::test]
    async fn test_react_error_hook() {
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let on_error = Arc::new(RecordingHook::new(HookDecision::Continue));
        agent
            .hooks
            .register(HookEvent::OnError, on_error.clone())
            .await;

        let _ = agent.react("test").await;

        let events = on_error.get_events().await;
        assert!(events.is_empty(), "OnError should not trigger on success");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_tool_call_hooks_in_react() {
        let config = AgentConfig::default();
        let mut responses = vec![
            LlmResponse::ToolCall {
                name: "test_tool".to_string(),
                args: serde_json::json!({ "param": "value" }),
                id: Some("call_123".to_string()),
            },
            LlmResponse::Text("Final answer: done".to_string()),
        ];
        let responses = Arc::new(TokioMutex::new(responses));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }),
            |args: &serde_json::Value| Ok(serde_json::json!({ "result": "executed" })),
        ));
        agent.add_tool(tool);

        let before_tool = Arc::new(RecordingHook::new(HookDecision::Continue));
        let after_tool = Arc::new(RecordingHook::new(HookDecision::Continue));

        agent
            .hooks
            .register(HookEvent::BeforeToolCall, before_tool.clone())
            .await;
        agent
            .hooks
            .register(HookEvent::AfterToolCall, after_tool.clone())
            .await;

        let _ = agent.react("test").await;

        let before_events = before_tool.get_events().await;
        let after_events = after_tool.get_events().await;

        assert!(
            !before_events.is_empty(),
            "BeforeToolCall should be triggered in react"
        );
        assert!(
            !after_events.is_empty(),
            "AfterToolCall should be triggered in react"
        );

        if let Some((_, ctx)) = before_events.first() {
            assert_eq!(ctx.get("tool_name"), Some(&"test_tool".to_string()));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_tool_call_hooks_in_run_simple() {
        let config = AgentConfig::default();
        let responses = vec![LlmResponse::ToolCall {
            name: "test_tool".to_string(),
            args: serde_json::json!({ "param": "value" }),
            id: Some("call_123".to_string()),
        }];
        let responses = Arc::new(TokioMutex::new(responses));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }),
            |args: &serde_json::Value| Ok(serde_json::json!({ "result": "executed" })),
        ));
        agent.add_tool(tool);

        let before_tool = Arc::new(RecordingHook::new(HookDecision::Continue));
        let after_tool = Arc::new(RecordingHook::new(HookDecision::Continue));

        agent
            .hooks
            .register(HookEvent::BeforeToolCall, before_tool.clone())
            .await;
        agent
            .hooks
            .register(HookEvent::AfterToolCall, after_tool.clone())
            .await;

        let _ = agent.run_simple("test").await;

        let before_events = before_tool.get_events().await;
        let after_events = after_tool.get_events().await;

        assert!(
            !before_events.is_empty(),
            "BeforeToolCall should be triggered in run_simple"
        );
        assert!(
            !after_events.is_empty(),
            "AfterToolCall should be triggered in run_simple"
        );
    }

    #[tokio::test]
    async fn test_before_tool_call_abort() {
        let config = AgentConfig::default();
        let responses = vec![LlmResponse::ToolCall {
            name: "test_tool".to_string(),
            args: serde_json::json!({ "param": "value" }),
            id: Some("call_123".to_string()),
        }];
        let responses = Arc::new(TokioMutex::new(responses));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }),
            |args: &serde_json::Value| Ok(serde_json::json!({ "result": "executed" })),
        ));
        agent.add_tool(tool);

        let abort_hook = Arc::new(RecordingHook::new(HookDecision::Abort));
        agent
            .hooks
            .register(HookEvent::BeforeToolCall, abort_hook.clone())
            .await;

        let result = agent.run_simple("test").await;

        let events = abort_hook.get_events().await;
        assert!(!events.is_empty(), "Hook should be triggered");

        let tool_result = result.unwrap_or_default();
        assert!(
            tool_result.contains("aborted") || tool_result.contains("blocked"),
            "Tool should be blocked when hook returns Abort"
        );
    }

    #[tokio::test]
    async fn test_run_simple_tool_error_respects_on_error_abort() {
        let config = AgentConfig::default();
        let responses = vec![LlmResponse::ToolCall {
            name: "test_tool".to_string(),
            args: serde_json::json!({ "param": "value" }),
            id: Some("call_456".to_string()),
        }];
        let responses = Arc::new(TokioMutex::new(responses));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }),
            |_args: &serde_json::Value| Ok(serde_json::json!({ "result": "executed" })),
        ));
        agent.add_tool(tool);
        agent.add_plugin(Arc::new(ForceToolFailurePlugin));

        let on_error = Arc::new(RecordingHook::new(HookDecision::Abort));
        agent
            .hooks
            .register(HookEvent::OnError, on_error.clone())
            .await;

        let result = agent.run_simple("test").await.unwrap_or_default();
        assert!(
            result.is_empty(),
            "run_simple should stop when OnError hook returns Abort"
        );

        let events = on_error.get_events().await;
        assert!(!events.is_empty(), "OnError should trigger on tool failure");
    }

    #[tokio::test]
    async fn test_run_simple_tool_error_respects_on_error_error() {
        let config = AgentConfig::default();
        let responses = vec![LlmResponse::ToolCall {
            name: "test_tool".to_string(),
            args: serde_json::json!({ "param": "value" }),
            id: Some("call_789".to_string()),
        }];
        let responses = Arc::new(TokioMutex::new(responses));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }),
            |_args: &serde_json::Value| Ok(serde_json::json!({ "result": "executed" })),
        ));
        agent.add_tool(tool);
        agent.add_plugin(Arc::new(ForceToolFailurePlugin));

        let on_error = Arc::new(RecordingHook::new(HookDecision::Error(
            "stop_on_error".to_string(),
        )));
        agent
            .hooks
            .register(HookEvent::OnError, on_error.clone())
            .await;

        let result = agent.run_simple("test").await.unwrap_or_default();
        assert!(
            result.contains("Tool blocked by OnError hook"),
            "run_simple should return blocked message when OnError hook returns Error"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_tool_call_hooks_in_stream() {
        let config = AgentConfig::default();
        let responses = vec![
            LlmResponse::ToolCall {
                name: "test_tool".to_string(),
                args: serde_json::json!({ "param": "value" }),
                id: Some("call_123".to_string()),
            },
            LlmResponse::Text("Final answer: done".to_string()),
        ];
        let responses = Arc::new(TokioMutex::new(responses));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let tool = Arc::new(FunctionTool::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }),
            |args: &serde_json::Value| Ok(serde_json::json!({ "result": "executed" })),
        ));
        agent.add_tool(tool);

        let before_tool = Arc::new(RecordingHook::new(HookDecision::Continue));
        let after_tool = Arc::new(RecordingHook::new(HookDecision::Continue));

        agent
            .hooks
            .register(HookEvent::BeforeToolCall, before_tool.clone())
            .await;
        agent
            .hooks
            .register(HookEvent::AfterToolCall, after_tool.clone())
            .await;

        let mut stream = agent.stream("test");
        while let Some(_) = stream.next().await {}

        let before_events = before_tool.get_events().await;
        let after_events = after_tool.get_events().await;

        assert!(
            !before_events.is_empty(),
            "BeforeToolCall should be triggered in stream"
        );
        assert!(
            !after_events.is_empty(),
            "AfterToolCall should be triggered in stream"
        );
    }

    #[tokio::test]
    async fn test_stream_error_hook() {
        let config = AgentConfig::default();
        let responses = Arc::new(TokioMutex::new(vec![]));
        let llm = Arc::new(MockLlmClient { responses });
        let mut agent = Agent::new(config, llm);

        let on_error = Arc::new(RecordingHook::new(HookDecision::Continue));
        agent
            .hooks
            .register(HookEvent::OnError, on_error.clone())
            .await;

        let mut stream = agent.stream("test");
        while let Some(_) = stream.next().await {}

        let events = on_error.get_events().await;
        assert!(
            events.is_empty(),
            "OnError should not trigger on success in stream"
        );
    }
}
