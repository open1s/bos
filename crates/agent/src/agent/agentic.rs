use crate::agent::context::MessageContext;
use crate::agent::format_tool_result_content;
use crate::tools::FunctionTool;
use crate::{
    AgentError, LlmClient, LlmRequest, LlmResponse, OpenAiMessage, StreamToken, Tool, ToolRegistry,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use log::{info, warn};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

use react::engine::ReActEngineBuilder;
use react::llm::{
    LlmClient as ReactLlmTrait, LlmContext, LlmError as ReactLlmError,
    LlmRequest as ReactLlmRequest, LlmResponse as ReactLlmResponse,
    StreamToken as ReactStreamToken, TokenStream as ReactTokenStream,
};
use react::tool::{Tool as ReactToolTrait, ToolError as ReactToolError};
use react::{RateLimiterConfig, ReActResilience};

#[derive(Debug, Clone)]
struct ToolGuardRule {
    tool_name: String,
    forbidden_markers: Vec<String>,
    rejection_text: String,
}

/// Outcome of handling a tool call in the run_loop.
/// Used to signal whether the loop should continue or proceed normally.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolCallOutcome {
    /// Tool was executed successfully
    Executed,
    /// Tool was blocked by skill guard rules
    Blocked,
    /// A skill was loaded (no tool execution)
    SkillLoaded,
}

// ============================================================================
// ReAct Adapters - Bridge between Agent and React crate
// ============================================================================

struct ReactToolAdapter {
    inner: Arc<dyn Tool + Send + Sync>,
}

impl ReactToolAdapter {
    fn new(inner: Arc<dyn Tool + Send + Sync>) -> Self {
        Self { inner }
    }
}

impl ReactToolTrait for ReactToolAdapter {
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
        let result =
            tokio::task::block_in_place(|| futures::executor::block_on(self.inner.execute(input)));
        result.map_err(|e| ReactToolError::Failed(e.to_string()))
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

// ============================================================================
// Simplified Agent API - Builder Pattern
// ============================================================================

/// Agent builder for fluent configuration.
#[derive(Debug, Clone)]
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
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            tools: Vec::new(),
            skills_dir: None,
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

    /// Build the Agent (requires LLM client to be provided separately for now).
    pub fn build_with_llm(self, llm: Arc<dyn LlmClient>) -> Result<Agent, AgentError> {
        let mut registry = ToolRegistry::new();
        for tool in self.tools {
            registry.register(tool)?;
        }

        let resilience = ReActResilience::new(react::ResilienceConfig {
            circuit_breaker: Default::default(),
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
    pub fn build_session(self, llm: Arc<dyn LlmClient>) -> Result<AgentSession, AgentError> {
        let agent = self.build_with_llm(llm)?;
        Ok(AgentSession::new(agent))
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

/// Agent - a stateless, immutable agent configuration.
///
/// Use `Agent::builder()` to create, then call `run()` or `stream()` on the returned session.
pub struct Agent {
    config: AgentConfig,
    llm: Arc<dyn LlmClient>,
    registry: Option<Arc<ToolRegistry>>,
    skills_dir: Option<std::path::PathBuf>,
    skills: Vec<crate::skills::SkillContent>,
    resilience: ReActResilience,
}

impl Agent {
    /// Create a new Agent with the given config and LLM client.
    pub fn new(config: AgentConfig, llm: Arc<dyn LlmClient>) -> Self {
        let resilience = ReActResilience::new(react::ResilienceConfig {
            circuit_breaker: Default::default(),
            rate_limiter: config.rate_limit.clone().unwrap_or_default(),
        });
        Self {
            config,
            llm,
            registry: Some(Arc::new(ToolRegistry::new())),
            skills_dir: None,
            skills: Vec::new(),
            resilience,
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

    /// Run the agent using ReAct engine.
    pub async fn react(&self, task: &str) -> Result<String, AgentError> {
        let react_llm = Box::new(AgentLlmAdapter::new(self.llm.clone()));
        let mut builder = ReActEngineBuilder::new().llm(react_llm);

        if let Some(ref registry) = self.registry {
            for (_name, tool) in registry.iter() {
                let tool_adapter = Box::new(ReactToolAdapter::new(tool.clone()));
                builder = builder.with_tool(tool_adapter);
            }
        }

        // Inject skill catalog and load_skill tool into ReAct engine
        let has_skills = !self.skills.is_empty();
        let skills_schemas = self.get_skills_schemas();
        if has_skills {
            let skill_names: Vec<String> = self
                .skills
                .iter()
                .map(|s| s.metadata.name.clone())
                .collect();

            // Register each skill as a callable tool that returns its instructions
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
                builder = builder.with_tool(Box::new(ReactToolAdapter::new(skill_tool)));
            }

            // Also register load_skill for compatibility
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
            builder = builder.with_tool(Box::new(ReactToolAdapter::new(load_skill_tool)));
        }

        // Build system prompt with skill context
        let mut system_prompt = self.config.system_prompt.clone();
        if has_skills {
            if !system_prompt.is_empty() {
                system_prompt.push_str("\n\n");
            }
            let skills_catalog = AgentSession::render_skills_catalog(&skills_schemas);
            system_prompt.push_str(&format!(
                "Available skills (call load_skill tool to get instructions):\n{}",
                skills_catalog
            ));
        }

        // Add strict format instructions with tool schemas
        if !system_prompt.is_empty() {
            system_prompt.push_str("\n\n");
        }

        // Build tool schema descriptions
        let mut tool_schemas = String::new();
        if let Some(ref registry) = self.registry {
            for (name, tool) in registry.iter() {
                let schema = tool.json_schema();
                tool_schemas.push_str(&format!("- {}: {:?}\n", name, schema));
            }
        }

        if !tool_schemas.is_empty() {
            system_prompt.push_str(&format!(
                "Available tools (MUST follow each tool's schema exactly):\n{}\n\
                Use keyword arguments matching the schema. Final answer: Final Answer: your answer",
                tool_schemas
            ));
        } else {
            system_prompt.push_str("Final answer: Final Answer: your answer");
        }

        builder = builder.resilience(self.resilience.clone());
        builder = builder.llm_timeout(self.config.timeout_secs);
        builder = builder.max_steps(self.config.max_steps);
        builder = builder.model(self.config.model.clone());
        builder = builder.system_prompt(system_prompt);

        let mut engine = builder
            .build()
            .map_err(|e| AgentError::Session(format!("ReAct build error: {}", e)))?;
        let result = engine
            .react(task)
            .await
            .map_err(|e| AgentError::Session(format!("ReAct run error: {}", e)))?;
        Ok(result)
    }

    /// Run the agent using simple loop (no ReAct).
    /// Useful for testing or when ReAct format is not needed.
    pub async fn run_simple(&self, task: &str) -> Result<String, AgentError> {
        let mut session = AgentSession::new(self.clone());
        session.run(task).await
    }

    /// Stream the agent response using simple approach.
    pub fn stream(
        &self,
        task: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send>> {
        let session = AgentSession::new(self.clone());
        session.into_stream(task)
    }

    /// Register a tool.
    pub fn add_tool(&mut self, tool: Arc<dyn Tool>) {
        if let Err(e) = self.try_add_tool(tool) {
            warn!("Failed to register tool: {}", e);
        }
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
        }
    }
}

/// AgentSession - holds execution state, created from Agent.
pub struct AgentSession {
    agent: Agent,
    context: MessageContext,
    session_agent_id: Option<String>,
    session_metadata: Option<crate::session::SessionMetadata>,
}

impl AgentSession {
    const CONTEXT_COMPACTION_HEADER: &'static str = "[Context compaction auto-generated]";

    fn normalize_skill_name(name: &str) -> String {
        name.trim().to_ascii_lowercase().replace('-', "_")
    }

    fn extract_requested_skill(name: &str, args: &serde_json::Value) -> Option<String> {
        if let Some(skill_name) = name.strip_prefix("load_skill_") {
            return Some(skill_name.to_string());
        }

        if name == "load_skill" {
            return args
                .get("name")
                .or_else(|| args.get("skill"))
                .or_else(|| args.get("skill_name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| args.as_str().map(|s| s.to_string()));
        }

        None
    }

    fn resolve_skill_name(
        tool_name: &str,
        args: &serde_json::Value,
        skill_name_index: &HashMap<String, String>,
    ) -> Option<String> {
        if let Some(requested) = Self::extract_requested_skill(tool_name, args) {
            return skill_name_index
                .get(&Self::normalize_skill_name(&requested))
                .cloned();
        }

        // Some providers call a skill name directly as a tool (e.g. "calculator").
        skill_name_index
            .get(&Self::normalize_skill_name(tool_name))
            .cloned()
    }

    fn infer_skill_for_tool_call(
        tool_name: &str,
        loaded_skills: &[String],
        skills_content: &[(String, String)],
    ) -> Option<String> {
        let marker = format!("`{}`", tool_name);
        let mut candidates = skills_content
            .iter()
            .filter(|(skill_name, instructions)| {
                !loaded_skills.contains(skill_name) && instructions.contains(&marker)
            })
            .map(|(skill_name, _)| skill_name.clone());

        let first = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        Some(first)
    }

    fn parse_skill_guard_rules(
        skills_content: &[(String, String)],
    ) -> HashMap<String, Vec<ToolGuardRule>> {
        fn extract_quoted(s: &str) -> Option<String> {
            let start = s.find('"')?;
            let rest = &s[start + 1..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        }

        fn extract_tool_name_from_never_call(line: &str) -> Option<String> {
            let marker = "NEVER call `";
            let start = line.find(marker)? + marker.len();
            let tail = &line[start..];
            let end = tail.find('`')?;
            Some(tail[..end].to_string())
        }

        fn collect_markers(sample: &str) -> Vec<String> {
            let mut markers = Vec::new();
            for op in ['*', '/', '-', '+', '×'] {
                if sample.contains(op) {
                    markers.push(op.to_string());
                }
            }
            markers
        }

        let mut out: HashMap<String, Vec<ToolGuardRule>> = HashMap::new();

        for (skill_name, instructions) in skills_content {
            let normalized_skill = Self::normalize_skill_name(skill_name);
            let mut rejection_text: Option<String> = None;
            let mut pending_rejection = false;
            let mut by_tool_markers: HashMap<String, Vec<String>> = HashMap::new();

            for raw in instructions.lines() {
                let line = raw.trim();
                if line.is_empty() {
                    continue;
                }

                if pending_rejection {
                    if let Some(q) = extract_quoted(line) {
                        rejection_text = Some(q);
                        pending_rejection = false;
                    }
                }

                if line.to_ascii_lowercase().contains("respond exactly") {
                    pending_rejection = true;
                }

                if let Some(tool_name) = extract_tool_name_from_never_call(line) {
                    if let Some(sample) = extract_quoted(line) {
                        let markers = collect_markers(&sample);
                        if !markers.is_empty() {
                            by_tool_markers
                                .entry(tool_name)
                                .or_default()
                                .extend(markers);
                        }
                    }
                }
            }

            if by_tool_markers.is_empty() {
                continue;
            }

            let rejection = rejection_text
                .unwrap_or_else(|| "Tool call blocked by loaded skill policy.".to_string());
            let mut rules = Vec::new();
            for (tool_name, mut markers) in by_tool_markers {
                markers.sort();
                markers.dedup();
                rules.push(ToolGuardRule {
                    tool_name,
                    forbidden_markers: markers,
                    rejection_text: rejection.clone(),
                });
            }
            out.insert(normalized_skill, rules);
        }

        out
    }

    fn latest_user_message(context: &MessageContext) -> Option<String> {
        context.messages.iter().rev().find_map(|m| match m {
            react::Message::User { content } => Some(content.clone()),
            _ => None,
        })
    }

    fn blocked_tool_result_for_skill(
        tool_name: &str,
        loaded_skills: &[String],
        latest_user: Option<&str>,
        skill_guard_rules: &HashMap<String, Vec<ToolGuardRule>>,
    ) -> Option<String> {
        let latest_user = latest_user?.to_ascii_lowercase();
        for loaded in loaded_skills {
            let normalized_skill = Self::normalize_skill_name(loaded);
            let Some(rules) = skill_guard_rules.get(&normalized_skill) else {
                continue;
            };

            for rule in rules {
                if rule.tool_name != tool_name {
                    continue;
                }
                if rule
                    .forbidden_markers
                    .iter()
                    .any(|m| latest_user.contains(m))
                {
                    return Some(rule.rejection_text.clone());
                }
            }
        }

        None
    }

    fn render_skills_catalog(schemas: &[serde_json::Value]) -> String {
        let compact: Vec<serde_json::Value> = schemas
            .iter()
            .map(|schema| {
                let name = schema
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let category = schema
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("other");
                let description = schema
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                serde_json::json!({
                    "name": name,
                    "category": category,
                    "description": description
                })
            })
            .collect();

        match serde_yaml::to_string(&compact) {
            Ok(yaml) => yaml.strip_prefix("---\n").unwrap_or(&yaml).to_string(),
            Err(_) => serde_json::to_string_pretty(&compact).unwrap_or_else(|_| "[]".to_string()),
        }
    }

    fn approx_tokens_for_text(text: &str) -> usize {
        // Heuristic: ~4 chars/token for mixed English/code JSON payloads.
        (text.len() / 4).max(1)
    }

    fn approx_context_tokens(context: &MessageContext) -> usize {
        context
            .messages
            .iter()
            .map(|m| match m {
                react::Message::System { content }
                | react::Message::User { content }
                | react::Message::Assistant { content } => {
                    Self::approx_tokens_for_text(content) + 4
                }
                react::Message::AssistantToolCall {
                    tool_call_id,
                    name,
                    args,
                } => {
                    let args_s = serde_json::to_string(args).unwrap_or_default();
                    Self::approx_tokens_for_text(tool_call_id)
                        + Self::approx_tokens_for_text(name)
                        + Self::approx_tokens_for_text(&args_s)
                        + 8
                }
                react::Message::ToolResult {
                    tool_call_id,
                    content,
                } => {
                    Self::approx_tokens_for_text(tool_call_id)
                        + Self::approx_tokens_for_text(content)
                        + 6
                }
            })
            .sum()
    }

    fn compact_line(label: &str, text: &str) -> String {
        const MAX_LINE_CHARS: usize = 220;
        let clipped = if text.chars().count() > MAX_LINE_CHARS {
            let prefix: String = text.chars().take(MAX_LINE_CHARS).collect();
            format!("{}...", prefix)
        } else {
            text.to_string()
        };
        format!("- {}: {}", label, clipped.replace('\n', " "))
    }

    fn build_fallback_compaction_summary(
        compacted_slice: &[react::Message],
        approx_tokens: usize,
        max_summary_chars: usize,
    ) -> String {
        let mut summary = String::new();
        summary.push_str(Self::CONTEXT_COMPACTION_HEADER);
        summary.push_str(&format!(
            "\nCompacted {} earlier messages (~{} tokens) to stay within context budget.\n",
            compacted_slice.len(),
            approx_tokens
        ));

        for message in compacted_slice {
            let line = match message {
                react::Message::System { content } => Self::compact_line("system", content),
                react::Message::User { content } => Self::compact_line("user", content),
                react::Message::Assistant { content } => Self::compact_line("assistant", content),
                react::Message::AssistantToolCall { name, args, .. } => {
                    let args_s = serde_json::to_string(args).unwrap_or_default();
                    Self::compact_line("assistant_tool_call", &format!("{} {}", name, args_s))
                }
                react::Message::ToolResult {
                    tool_call_id,
                    content,
                } => Self::compact_line("tool_result", &format!("{} {}", tool_call_id, content)),
            };
            if summary.len() + line.len() + 1 > max_summary_chars {
                summary.push_str("\n- ... older compacted entries truncated ...");
                break;
            }
            summary.push('\n');
            summary.push_str(&line);
        }
        summary
    }

    fn build_compaction_prompt(
        compacted_slice: &[react::Message],
        max_summary_chars: usize,
    ) -> String {
        let mut transcript = String::new();
        for message in compacted_slice {
            let line = match message {
                react::Message::System { content } => Self::compact_line("system", content),
                react::Message::User { content } => Self::compact_line("user", content),
                react::Message::Assistant { content } => Self::compact_line("assistant", content),
                react::Message::AssistantToolCall { name, args, .. } => {
                    let args_s = serde_json::to_string(args).unwrap_or_default();
                    Self::compact_line("assistant_tool_call", &format!("{} {}", name, args_s))
                }
                react::Message::ToolResult {
                    tool_call_id,
                    content,
                } => Self::compact_line("tool_result", &format!("{} {}", tool_call_id, content)),
            };
            if transcript.len() + line.len() + 1 > max_summary_chars {
                transcript.push_str("\n- ... transcript truncated ...");
                break;
            }
            transcript.push('\n');
            transcript.push_str(&line);
        }
        transcript
    }

    async fn summarize_compacted_messages(
        llm: Arc<dyn LlmClient>,
        resilience: ReActResilience,
        model: String,
        compacted_slice: &[react::Message],
        max_summary_chars: usize,
        summary_max_tokens: u32,
    ) -> Option<String> {
        let prompt = Self::build_compaction_prompt(compacted_slice, max_summary_chars);
        let req = LlmRequest {
            model,
            temperature: Some(0.1),
            top_p: None,
            top_k: None,
            max_tokens: Some(summary_max_tokens),
            context: LlmContext {
                tools: vec![],
                skills: vec![],
                conversations: vec![
                    OpenAiMessage::System {
                        content: "You compress chat history for an agent. Produce a concise factual summary preserving user goals, constraints, decisions, unresolved tasks, and important tool outcomes. Do not invent details.".to_string(),
                    },
                    OpenAiMessage::User {
                        content: format!(
                            "Summarize the following prior conversation for future turns.\nOutput plain text, max 12 bullets.\n{}",
                            prompt
                        ),
                    },
                ],
                rules: vec![],
                instructions: vec![],
            }
        };

        let response = resilience
            .execute(|| {
                let req = req.clone();
                let llm = llm.clone();
                async move { llm.complete(req).await }
            })
            .await
            .ok()?;

        match response {
            LlmResponse::Text(text) | LlmResponse::Partial(text) => {
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
            _ => None,
        }
    }

    async fn maybe_compact_context_with_llm(
        context: &mut MessageContext,
        config: &AgentConfig,
        llm: Arc<dyn LlmClient>,
        resilience: ReActResilience,
        model: String,
    ) {
        let trigger_tokens = (config.context_compaction_threshold_tokens as f32
            * config.context_compaction_trigger_ratio) as usize;
        let approx_tokens = Self::approx_context_tokens(context);

        if approx_tokens < trigger_tokens {
            return;
        }

        if context.messages.len() <= config.context_compaction_keep_recent_messages + 1 {
            return;
        }

        let split_at = context
            .messages
            .len()
            .saturating_sub(config.context_compaction_keep_recent_messages);
        let compacted_slice = &context.messages[..split_at];
        let recent_slice = &context.messages[split_at..];

        let llm_summary = Self::summarize_compacted_messages(
            llm,
            resilience,
            model,
            compacted_slice,
            config.context_compaction_max_summary_chars,
            config.context_compaction_summary_max_tokens,
        )
        .await;
        let summary = llm_summary.map_or_else(
            || {
                Self::build_fallback_compaction_summary(
                    compacted_slice,
                    approx_tokens,
                    config.context_compaction_max_summary_chars,
                )
            },
            |text| {
                format!(
                    "{}\nCompacted {} earlier messages (~{} tokens).\n{}",
                    Self::CONTEXT_COMPACTION_HEADER,
                    compacted_slice.len(),
                    approx_tokens,
                    text
                )
            },
        );

        let mut next = Vec::with_capacity(recent_slice.len() + 1);
        next.push(react::Message::System { content: summary });
        next.extend_from_slice(recent_slice);
        context.messages = next;
    }

    fn new(agent: Agent) -> Self {
        Self {
            agent,
            context: MessageContext::new(),
            session_agent_id: None,
            session_metadata: None,
        }
    }

    fn into_stream(
        mut self,
        task: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send>> {
        use tokio_stream::wrappers::ReceiverStream;
        let task = task.to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        tokio::spawn(async move {
            let mut stream = self.stream(&task);
            while let Some(item) = stream.next().await {
                if tx.send(item).await.is_err() {
                    return;
                }
            }
        });

        Box::pin(ReceiverStream::new(rx))
    }

    /// Export current session into persisted state.
    pub fn export_state(&self, agent_id: impl Into<String>) -> crate::session::AgentState {
        let agent_id = agent_id.into();
        let mut state = if self.session_agent_id.as_deref() == Some(agent_id.as_str()) {
            if let Some(meta) = &self.session_metadata {
                crate::session::AgentState {
                    agent_id: agent_id.clone(),
                    message_log: Vec::new(),
                    context: serde_json::Value::Null,
                    metadata: meta.clone(),
                }
            } else {
                crate::session::SessionSerializer::new_state(agent_id.clone())
            }
        } else {
            crate::session::SessionSerializer::new_state(agent_id.clone())
        };
        state.agent_id = agent_id;
        state.message_log = self.context.messages.clone();
        state.context = serde_json::json!({
            "compaction": {
                "threshold_tokens": self.agent.config.context_compaction_threshold_tokens,
                "trigger_ratio": self.agent.config.context_compaction_trigger_ratio,
                "keep_recent_messages": self.agent.config.context_compaction_keep_recent_messages,
                "max_summary_chars": self.agent.config.context_compaction_max_summary_chars,
                "summary_max_tokens": self.agent.config.context_compaction_summary_max_tokens
            }
        });
        crate::session::SessionSerializer::update_metadata(&mut state);
        state
    }

    /// Restore a session from persisted state.
    pub fn restore_from_state(mut agent: Agent, state: crate::session::AgentState) -> Self {
        if let Some(compaction) = state.context.get("compaction") {
            if let Some(v) = compaction.get("threshold_tokens").and_then(|v| v.as_u64()) {
                agent.config.context_compaction_threshold_tokens = v as usize;
            }
            if let Some(v) = compaction.get("trigger_ratio").and_then(|v| v.as_f64()) {
                agent.config.context_compaction_trigger_ratio = v as f32;
            }
            if let Some(v) = compaction
                .get("keep_recent_messages")
                .and_then(|v| v.as_u64())
            {
                agent.config.context_compaction_keep_recent_messages = v as usize;
            }
            if let Some(v) = compaction.get("max_summary_chars").and_then(|v| v.as_u64()) {
                agent.config.context_compaction_max_summary_chars = v as usize;
            }
            if let Some(v) = compaction
                .get("summary_max_tokens")
                .and_then(|v| v.as_u64())
            {
                agent.config.context_compaction_summary_max_tokens = v as u32;
            }
        }

        Self {
            agent,
            context: MessageContext {
                messages: state.message_log.clone(),
            },
            session_agent_id: Some(state.agent_id),
            session_metadata: Some(state.metadata),
        }
    }

    fn build_system_prompt_for_iteration(
        &self,
        loaded_skills: &[String],
        skills_schemas: &[serde_json::Value],
        skills_content: &[(String, String)],
    ) -> String {
        let mut system_prompt = self.agent.config.system_prompt.clone();

        if !skills_schemas.is_empty() {
            let skills_catalog = Self::render_skills_catalog(skills_schemas);
            let mut loaded_instr = String::new();
            for loaded in loaded_skills {
                if let Some((_, instr)) = skills_content.iter().find(|(n, _)| n == loaded) {
                    loaded_instr.push_str(&format!("\n\n=== Skill: {} ===\n{}\n", loaded, instr));
                }
            }
            system_prompt = format!(
                "{}\n\nAvailable skills catalog (load with tool `load_skill`):\n{}\n\nLoaded skill instructions:\n{}\n",
                system_prompt, skills_catalog, loaded_instr
            );
        }

        system_prompt
    }

    fn build_llm_request_for_iteration(
        &self,
        system_prompt: &str,
        tools: &Option<Arc<Vec<serde_json::Value>>>,
        skills_schemas: &[serde_json::Value],
    ) -> LlmRequest {
        let mut messages = Vec::with_capacity(self.context.len() + 1);
        messages.push(OpenAiMessage::System {
            content: system_prompt.to_string(),
        });
        self.context.extend_api_format(&mut messages);

        let llm_tools: Vec<react::llm::LlmTool> = tools
            .as_ref()
            .map(|t| {
                t.iter()
                    .filter_map(|v| {
                        let func_obj = v.get("function")?;
                        let result =
                            serde_json::from_value::<react::llm::LlmTool>(func_obj.clone());
                        result.ok()
                    })
                    .collect()
            })
            .unwrap_or_default();

        let llm_skills: Vec<react::llm::Skill> = skills_schemas
            .iter()
            .filter_map(|schema| {
                let name = schema.get("name")?.as_str()?.to_string();
                let category = schema.get("category")?.as_str()?.to_string();
                let description = schema.get("description")?.as_str()?.to_string();
                Some(react::llm::Skill {
                    name,
                    category,
                    description,
                })
            })
            .collect();

        LlmRequest {
            model: self.agent.config.model.clone(),
            temperature: Some(self.agent.config.temperature),
            top_p: None,
            top_k: None,
            max_tokens: self.agent.config.max_tokens,
            context: LlmContext {
                tools: llm_tools,
                skills: llm_skills,
                conversations: messages,
                rules: vec![],
                instructions: vec![react::llm::Instruction {
                    name: "end_conversation".to_string(),
                    instruction: "To end the conversation, prepend your response with 'Final Answer:'\nExample: Final Answer: The result is 42".to_string(),
                    description: "How to end the conversation".to_string(),
                    dependon: None,
                }],
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_tool_call(
        &mut self,
        name: String,
        args: serde_json::Value,
        tool_call_id: String,
        loaded_skills: &mut Vec<String>,
        skill_name_index: &HashMap<String, String>,
        skills_content: &[(String, String)],
        skill_guard_rules: &HashMap<String, Vec<ToolGuardRule>>,
        tool_registry: &Option<Arc<ToolRegistry>>,
    ) -> Result<ToolCallOutcome, AgentError> {
        let requested_skill = Self::extract_requested_skill(&name, &args);
        let resolved_skill = Self::resolve_skill_name(&name, &args, skill_name_index)
            .or_else(|| Self::infer_skill_for_tool_call(&name, loaded_skills, skills_content));

        if let Some(skill_name) = resolved_skill {
            let already_loaded = loaded_skills.contains(&skill_name);
            let call_name = "load_skill".to_string();
            let call_args = serde_json::json!({ "name": skill_name.clone() });
            if !already_loaded {
                loaded_skills.push(skill_name.clone());
            }
            self.context
                .add_tool_call(tool_call_id.clone(), call_name.clone(), call_args.clone());
            let msg = if already_loaded {
                format!("Skill '{}' already loaded. DO NOT call load_skill again. Use its instructions to answer.", skill_name)
            } else {
                format!("Loaded skill: {}", skill_name)
            };
            self.context.add_tool_result(tool_call_id, msg);
            return Ok(ToolCallOutcome::SkillLoaded);
        }

        if requested_skill.is_some() {
            let requested = requested_skill.unwrap_or_default();
            self.context
                .add_tool_call(tool_call_id.clone(), name.clone(), args.clone());
            self.context
                .add_tool_result(tool_call_id, format!("Skill not found: {}", requested));
            return Ok(ToolCallOutcome::Executed);
        }

        self.context
            .add_tool_call(tool_call_id.clone(), name.clone(), args.clone());

        if let Some(blocked) = Self::blocked_tool_result_for_skill(
            &name,
            loaded_skills,
            Self::latest_user_message(&self.context).as_deref(),
            skill_guard_rules,
        ) {
            self.context.add_tool_result(tool_call_id, blocked);
            return Ok(ToolCallOutcome::Blocked);
        }

        if let Some(ref registry) = tool_registry {
            let result = registry.execute(&name, &args).await?;
            self.context
                .add_tool_result(tool_call_id, format_tool_result_content(result));
        }

        Ok(ToolCallOutcome::Executed)
    }

    /// Run the agent on a task.
    pub async fn run(&mut self, task: &str) -> Result<String, AgentError> {
        self.context.add_user(task.to_string());
        self.run_loop().await
    }

    /// Stream the agent response.
    pub fn stream(
        &mut self,
        task: &str,
    ) -> impl Stream<Item = Result<StreamToken, AgentError>> + Send + '_ {
        self.context.add_user(task.to_string());
        self.stream_loop()
    }

    /// Internal run loop.
    async fn run_loop(&mut self) -> Result<String, AgentError> {
        const MAX_ITERATIONS: usize = 10;
        let tool_registry = self.agent.registry.clone();
        let tools = tool_registry.as_ref().map(|t| t.to_openai_format_shared());
        let skills_schemas = self.agent.get_skills_schemas();
        let skills_content: Vec<(String, String)> = self
            .agent
            .get_skills_content()
            .into_iter()
            .map(|(n, i)| (n.to_string(), i.to_string()))
            .collect();
        let skill_name_index: HashMap<String, String> = skills_content
            .iter()
            .map(|(name, _)| (Self::normalize_skill_name(name), name.clone()))
            .collect();
        let skill_guard_rules = Self::parse_skill_guard_rules(&skills_content);
        let mut loaded_skills: Vec<String> = Vec::new();
        let mut accumulated_text = String::new();
        let mut recent_tool_calls: Vec<(String, serde_json::Value)> = Vec::new();

        for _ in 0..MAX_ITERATIONS {
            Self::maybe_compact_context_with_llm(
                &mut self.context,
                &self.agent.config,
                self.agent.llm.clone(),
                self.agent.resilience.clone(),
                self.agent.config.model.clone(),
            )
            .await;

            let system_prompt = self.build_system_prompt_for_iteration(
                &loaded_skills,
                &skills_schemas,
                &skills_content,
            );

            let request =
                self.build_llm_request_for_iteration(&system_prompt, &tools, &skills_schemas);

            info!("iteration:\n{:?}\n--------------", request);
            let response = {
                let llm = self.agent.llm.clone();
                let req = request.clone();
                self.agent
                    .resilience
                    .execute(|| async { llm.complete(req.clone()).await })
                    .await
                    .map_err(|e| AgentError::Session(format!("LLM error: {:?}", e)))?
            };
            info!("response:\n{:?}\n-------------", response);

            match response {
                LlmResponse::Text(text) => {
                    accumulated_text.push_str(&text);
                    self.context.add_assistant(text.clone());
                    break;
                }
                LlmResponse::Partial(part) => {
                    accumulated_text.push_str(&part);
                    self.context.add_assistant(part);
                }
                LlmResponse::ToolCall { name, args, id } => {
                    let tool_call_id =
                        id.unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

                    let call_key = (name.clone(), args.clone());
                    if recent_tool_calls.iter().any(|k| k == &call_key) {
                        self.context.add_tool_call(
                            tool_call_id.clone(),
                            name.clone(),
                            args.clone(),
                        );
                        self.context.add_tool_result(
                            tool_call_id,
                            format!("Tool '{}' was already called with these arguments. Please use the previous result to answer.", name),
                        );
                        continue;
                    }
                    recent_tool_calls.push(call_key);
                    if recent_tool_calls.len() > 5 {
                        recent_tool_calls.remove(0);
                    }

                    let _outcome = self
                        .handle_tool_call(
                            name,
                            args,
                            tool_call_id,
                            &mut loaded_skills,
                            &skill_name_index,
                            &skills_content,
                            &skill_guard_rules,
                            &tool_registry,
                        )
                        .await?;
                    continue;
                }
                LlmResponse::Done => break,
            }
        }

        Ok(accumulated_text)
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_tool_call_stream(
        &mut self,
        name: &str,
        args: &serde_json::Value,
        tool_call_id: &str,
        loaded_skills: &mut Vec<String>,
        skill_name_index: &HashMap<String, String>,
        skills_content: &[(String, String)],
        skill_guard_rules: &HashMap<String, Vec<ToolGuardRule>>,
        tool_registry: &Option<Arc<ToolRegistry>>,
    ) -> Result<Option<StreamToken>, AgentError> {
        let requested_skill = Self::extract_requested_skill(name, args);
        let resolved_skill = Self::resolve_skill_name(name, args, skill_name_index)
            .or_else(|| Self::infer_skill_for_tool_call(name, loaded_skills, skills_content));

        if let Some(skill_name) = resolved_skill {
            let already_loaded = loaded_skills.contains(&skill_name);
            let call_name = "load_skill".to_string();
            let call_args = serde_json::json!({ "name": skill_name.clone() });
            if !already_loaded {
                loaded_skills.push(skill_name.clone());
            }
            self.context.add_tool_call(
                tool_call_id.to_string(),
                call_name.clone(),
                call_args.clone(),
            );
            let msg = if already_loaded {
                format!("Skill '{}' already loaded. DO NOT call load_skill again. Use its instructions to answer.", skill_name)
            } else {
                format!("Loaded skill: {}", skill_name)
            };
            self.context.add_tool_result(tool_call_id.to_string(), msg);
            return Ok(Some(StreamToken::ToolCall {
                name: call_name,
                args: call_args,
                id: Some(tool_call_id.to_string()),
            }));
        }

        if requested_skill.is_some() {
            let requested = requested_skill.unwrap_or_default();
            self.context
                .add_tool_call(tool_call_id.to_string(), name.to_string(), args.clone());
            self.context.add_tool_result(
                tool_call_id.to_string(),
                format!("Skill not found: {}", requested),
            );
            return Ok(None);
        }

        self.context
            .add_tool_call(tool_call_id.to_string(), name.to_string(), args.clone());

        if let Some(blocked) = Self::blocked_tool_result_for_skill(
            name,
            loaded_skills,
            Self::latest_user_message(&self.context).as_deref(),
            skill_guard_rules,
        ) {
            self.context
                .add_tool_result(tool_call_id.to_string(), blocked);
            return Ok(None);
        }

        if let Some(ref registry) = tool_registry {
            let result = registry
                .execute(name, args)
                .await
                .map_err(AgentError::Tool)?;
            self.context
                .add_tool_result(tool_call_id.to_string(), format_tool_result_content(result));
        }

        Ok(None)
    }

    fn stream_loop(
        &mut self,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send + '_>> {
        let config = self.agent.config.clone();
        let llm = self.agent.llm.clone();
        let tool_registry = self.agent.registry.clone();
        let tools = tool_registry.as_ref().map(|t| t.to_openai_format_shared());
        let skills_schemas = self.agent.get_skills_schemas();
        let skills_content: Vec<(String, String)> = self
            .agent
            .get_skills_content()
            .into_iter()
            .map(|(n, i)| (n.to_string(), i.to_string()))
            .collect();
        let skill_name_index: HashMap<String, String> = skills_content
            .iter()
            .map(|(name, _)| (Self::normalize_skill_name(name), name.clone()))
            .collect();
        let skill_guard_rules = Self::parse_skill_guard_rules(&skills_content);
        let resilience = self.agent.resilience.clone();

        Box::pin(async_stream::stream! {
                    let mut iteration = 1;
                    let mut loaded_skills: Vec<String> = Vec::new();
                    let mut recent_tool_calls: Vec<(String, serde_json::Value)> = Vec::new();
                    const MAX_ITERATIONS: usize = 10;
                    loop {
                        if iteration > MAX_ITERATIONS {
                            return;
                        }

                        Self::maybe_compact_context_with_llm(
                            &mut self.context,
                            &config,
                            llm.clone(),
                            resilience.clone(),
                            config.model.clone(),
                        )
                        .await;

                        let system_prompt = self.build_system_prompt_for_iteration(
                            &loaded_skills,
                            &skills_schemas,
                            &skills_content,
                        );

                        let request = self.build_llm_request_for_iteration(
                            &system_prompt,
                            &tools,
                            &skills_schemas,
                        );

                            info!(
                                "Iteration:{} LLM Request:\n{:?}\n-----------------\n",
                                iteration, request
                            );
                            iteration += 1;
                            let stream: react::llm::TokenStream = {
                                let llm = llm.clone();
                                let req = request.clone();
                                match resilience
                                    .execute(|| async { llm.stream_complete(req.clone()).await })
                                    .await
                                {
                                    Ok(s) => s,
                                    Err(e) => {
                                        yield Err(AgentError::Session(format!("Stream error: {:?}", e)));
                                        return;
                                    }
                                }
                            };

                            let mut stream = Box::pin(stream);
                            let mut tool_call_made = false;
                            while let Some(token_result) = stream.next().await {
                                match token_result {
                                    Ok(token) => {
                                        match &token {
                                            StreamToken::Text(s) => {
                                                self.context.append_assistant_chunk(s);
                                                yield Ok(StreamToken::Text(s.clone()));
                                            }
                                            StreamToken::ToolCall { name, args, id } => {
                                                tool_call_made = true;
                                                let tool_call_id = id
                                                    .clone()
                                                    .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

                                                let call_key = (name.clone(), args.clone());
                                                if recent_tool_calls.iter().any(|k| k == &call_key) {
                                                    self.context.add_tool_call(tool_call_id.clone(), name.clone(), args.clone());
                                                    self.context.add_tool_result(
                                                        tool_call_id.clone(),
                                                        format!("Tool '{}' was already called with these arguments. Please use the previous result to answer.", name),
                                                    );
                                                    continue;
                                                }
                                                recent_tool_calls.push(call_key);
                                                if recent_tool_calls.len() > 5 {
                                                    recent_tool_calls.remove(0);
                                                }

                                                yield Ok(StreamToken::ToolCall {
                                                    name: name.clone(),
                                                    args: args.clone(),
                                                    id: Some(tool_call_id.clone()),
                                                });

        let outcome = self.handle_tool_call_stream(
                                name,
                                args,
                                &tool_call_id,
                                &mut loaded_skills,
                                &skill_name_index,
                                &skills_content,
                                &skill_guard_rules,
                                &tool_registry,
                            ).await?;

                                                if let Some(out_tok) = outcome {
                                                    yield Ok(out_tok);
                                                }
                                            }
        StreamToken::Done => {
                            if !tool_call_made {
                                return;
                            }
                            tool_call_made = false;
                        }
                                        }
                                    }
                                    Err(e) => {
                                        yield Err(AgentError::Llm(e.into()));
                                        return;
                                    }
                                }
                            }
                        }
                    })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FunctionTool;
    use react::LlmError;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;

    struct MockLlm {
        response: String,
    }

    struct SkillStreamMockLlm {
        calls: AtomicUsize,
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl SkillStreamMockLlm {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    struct SkillNameToolCallMockLlm {
        calls: AtomicUsize,
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl SkillNameToolCallMockLlm {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    struct AddToolCallMockLlm {
        calls: AtomicUsize,
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl AddToolCallMockLlm {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    struct RunLoopAddToolCallMockLlm {
        calls: AtomicUsize,
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl RunLoopAddToolCallMockLlm {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    struct RunLoopLoadThenAddMockLlm {
        calls: AtomicUsize,
    }

    impl RunLoopLoadThenAddMockLlm {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    struct RunLoopDirectAddMockLlm {
        calls: AtomicUsize,
    }

    impl RunLoopDirectAddMockLlm {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    struct StreamLoopDirectAddMockLlm {
        calls: AtomicUsize,
    }

    impl StreamLoopDirectAddMockLlm {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    struct ContextCaptureMockLlm {
        requests: Arc<Mutex<Vec<LlmRequest>>>,
    }

    impl ContextCaptureMockLlm {
        fn new() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse::Text(self.response.clone()))
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            let stream = futures::stream::iter(vec![
                Ok(StreamToken::Text(self.response.clone())),
                Ok(StreamToken::Done),
            ]);
            Ok(Box::pin(stream))
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    #[async_trait]
    impl LlmClient for SkillStreamMockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse::Text("ok".to_string()))
        }

        async fn stream_complete(
            &self,
            req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            self.requests.lock().unwrap().push(req);
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);

            let tokens = if call_index == 0 {
                vec![
                    Ok(StreamToken::ToolCall {
                        name: "load_skill_code_review".to_string(),
                        args: serde_json::json!({}),
                        id: Some("load-1".to_string()),
                    }),
                    Ok(StreamToken::Done),
                ]
            } else {
                vec![
                    Ok(StreamToken::Text("done".to_string())),
                    Ok(StreamToken::Done),
                ]
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

    #[async_trait]
    impl LlmClient for SkillNameToolCallMockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse::Text("ok".to_string()))
        }

        async fn stream_complete(
            &self,
            req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            self.requests.lock().unwrap().push(req);
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);

            let tokens = if call_index == 0 {
                vec![
                    Ok(StreamToken::ToolCall {
                        name: "calculator".to_string(),
                        args: serde_json::json!({}),
                        id: Some("skill-as-tool-1".to_string()),
                    }),
                    Ok(StreamToken::Done),
                ]
            } else {
                vec![
                    Ok(StreamToken::Text("done".to_string())),
                    Ok(StreamToken::Done),
                ]
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

    #[async_trait]
    impl LlmClient for AddToolCallMockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse::Text("ok".to_string()))
        }

        async fn stream_complete(
            &self,
            req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            self.requests.lock().unwrap().push(req);
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);

            let tokens = if call_index == 0 {
                vec![
                    Ok(StreamToken::ToolCall {
                        name: "add".to_string(),
                        args: serde_json::json!({"a": 2, "b": 30}),
                        id: Some("add-1".to_string()),
                    }),
                    Ok(StreamToken::Done),
                ]
            } else {
                vec![
                    Ok(StreamToken::Text("done".to_string())),
                    Ok(StreamToken::Done),
                ]
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

    #[async_trait]
    impl LlmClient for RunLoopAddToolCallMockLlm {
        async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
            self.requests.lock().unwrap().push(req);
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);

            if call_index == 0 {
                Ok(LlmResponse::ToolCall {
                    name: "add".to_string(),
                    args: serde_json::json!({"a": 2, "b": 30}),
                    id: Some("run-add-1".to_string()),
                })
            } else {
                Ok(LlmResponse::Text("done".to_string()))
            }
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    #[async_trait]
    impl LlmClient for RunLoopLoadThenAddMockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);

            match call_index {
                0 => Ok(LlmResponse::ToolCall {
                    name: "load_skill".to_string(),
                    args: serde_json::json!({"name": "calculator"}),
                    id: Some("load-1".to_string()),
                }),
                1 => Ok(LlmResponse::ToolCall {
                    name: "add".to_string(),
                    args: serde_json::json!({"a": 2, "b": 30}),
                    id: Some("add-1".to_string()),
                }),
                _ => Ok(LlmResponse::Text(
                    "Sorry, I can only perform addition with the available tools.".to_string(),
                )),
            }
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    #[async_trait]
    impl LlmClient for RunLoopDirectAddMockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            match call_index {
                0 => Ok(LlmResponse::ToolCall {
                    name: "add".to_string(),
                    args: serde_json::json!({"a": 2, "b": 30}),
                    id: Some("run-direct-add-1".to_string()),
                }),
                _ => Ok(LlmResponse::Text("2 + 30 is 32".to_string())),
            }
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            Ok(Box::pin(futures::stream::iter(vec![Ok(StreamToken::Done)])))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    #[async_trait]
    impl LlmClient for StreamLoopDirectAddMockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse::Text("unused".to_string()))
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            let tokens = if call_index == 0 {
                vec![
                    Ok(StreamToken::ToolCall {
                        name: "add".to_string(),
                        args: serde_json::json!({"a": 2, "b": 30}),
                        id: Some("stream-direct-add-1".to_string()),
                    }),
                    Ok(StreamToken::Done),
                ]
            } else {
                vec![
                    Ok(StreamToken::Text("2 + 30 is 32".to_string())),
                    Ok(StreamToken::Done),
                ]
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

    #[async_trait]
    impl LlmClient for ContextCaptureMockLlm {
        async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
            self.requests.lock().unwrap().push(req);
            Ok(LlmResponse::Text("ok".to_string()))
        }

        async fn stream_complete(
            &self,
            req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            self.requests.lock().unwrap().push(req);
            let stream = futures::stream::iter(vec![
                Ok(StreamToken::Text("ok".to_string())),
                Ok(StreamToken::Done),
            ]);
            Ok(Box::pin(stream))
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    #[tokio::test]
    async fn test_resilience_default_config() {
        let llm = Arc::new(MockLlm {
            response: "test".to_string(),
        });
        let config = AgentConfig::default();
        let agent = Agent::new(config, llm);

        println!("Agent resilience: {:?}", agent.resilience);
        println!(
            "Rate limit remaining: {:?}",
            agent.resilience.rate_limit_remaining()
        );

        // Try to acquire should work with default config
        let result = agent.resilience.try_acquire();
        println!("try_acquire result: {:?}", result);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stream_loop_resolves_skill_alias_and_injects_loaded_instructions() {
        let llm = Arc::new(SkillStreamMockLlm::new());
        let mut agent = Agent::new(AgentConfig::default(), llm.clone());
        agent
            .register_skills_from_dir(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills"),
            )
            .unwrap();

        let mut stream = agent.stream("review this change");
        while stream.next().await.is_some() {}

        let requests = llm.requests.lock().unwrap();
        assert!(
            requests.len() >= 2,
            "expected at least 2 requests, got {}",
            requests.len()
        );

        let second_system_prompt = requests[1]
            .context
            .conversations
            .first()
            .and_then(|m| match m {
                OpenAiMessage::System { content } => Some(content.as_str()),
                _ => None,
            })
            .expect("second request should start with system prompt");

        assert!(second_system_prompt.contains("=== Skill: code-review ==="));
        assert!(
            second_system_prompt.contains("Read all changed files and provide a concise summary.")
        );
    }

    #[tokio::test]
    async fn test_stream_loop_treats_skill_name_tool_call_as_skill_load() {
        let llm = Arc::new(SkillNameToolCallMockLlm::new());
        let mut agent = Agent::new(AgentConfig::default(), llm.clone());
        agent
            .register_skills_from_dir(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills"),
            )
            .unwrap();

        let mut stream = agent.stream("what is 2 * 30");
        let mut saw_canonical_load_skill_token = false;
        while let Some(item) = stream.next().await {
            if let Ok(StreamToken::ToolCall { name, .. }) = item {
                if name == "load_skill" {
                    saw_canonical_load_skill_token = true;
                }
            }
        }

        let requests = llm.requests.lock().unwrap();
        assert!(
            requests.len() >= 2,
            "expected at least 2 requests, got {}",
            requests.len()
        );

        let second_system_prompt = requests[1]
            .context
            .conversations
            .first()
            .and_then(|m| match m {
                OpenAiMessage::System { content } => Some(content.as_str()),
                _ => None,
            })
            .expect("second request should start with system prompt");

        assert!(second_system_prompt.contains("=== Skill: calculator ==="));
        assert!(second_system_prompt.contains("Your ONLY job is addition. Nothing else."));
        assert!(
            saw_canonical_load_skill_token,
            "expected canonical load_skill token to be emitted"
        );
    }

    #[tokio::test]
    async fn test_stream_loop_inferrs_skill_from_tool_name_in_instructions() {
        let llm = Arc::new(AddToolCallMockLlm::new());
        let mut agent = Agent::new(AgentConfig::default(), llm.clone());
        agent
            .register_skills_from_dir(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills"),
            )
            .unwrap();

        let mut stream = agent.stream("what is 2 * 30");
        while stream.next().await.is_some() {}

        let requests = llm.requests.lock().unwrap();
        assert!(
            requests.len() >= 2,
            "expected at least 2 requests, got {}",
            requests.len()
        );

        let second_system_prompt = requests[1]
            .context
            .conversations
            .first()
            .and_then(|m| match m {
                OpenAiMessage::System { content } => Some(content.as_str()),
                _ => None,
            })
            .expect("second request should start with system prompt");

        assert!(second_system_prompt.contains("=== Skill: calculator ==="));
        assert!(second_system_prompt.contains("Your ONLY job is addition. Nothing else."));
    }

    #[tokio::test]
    async fn test_run_loop_inferrs_skill_from_tool_name_in_instructions() {
        let llm = Arc::new(RunLoopAddToolCallMockLlm::new());
        let mut agent = Agent::new(AgentConfig::default(), llm.clone());
        agent
            .register_skills_from_dir(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills"),
            )
            .unwrap();

        let result = agent.run_simple("what is 2 * 30").await.unwrap();
        assert_eq!(result, "done");

        let requests = llm.requests.lock().unwrap();
        assert!(
            requests.len() >= 2,
            "expected at least 2 requests, got {}",
            requests.len()
        );

        let second_system_prompt = requests[1]
            .context
            .conversations
            .first()
            .and_then(|m| match m {
                OpenAiMessage::System { content } => Some(content.as_str()),
                _ => None,
            })
            .expect("second request should start with system prompt");

        assert!(second_system_prompt.contains("=== Skill: calculator ==="));
        assert!(second_system_prompt.contains("Your ONLY job is addition. Nothing else."));
    }

    #[tokio::test]
    async fn test_run_loop_blocks_add_when_calculator_loaded_for_non_addition_query() {
        let llm = Arc::new(RunLoopLoadThenAddMockLlm::new());
        let mut agent = Agent::new(AgentConfig::default(), llm);
        agent
            .register_skills_from_dir(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills"),
            )
            .unwrap();

        let executed = Arc::new(AtomicUsize::new(0));
        let executed_ref = executed.clone();
        let add_tool = Arc::new(FunctionTool::numeric(
            "add",
            "Add two numbers",
            2,
            move |_args| {
                executed_ref.fetch_add(1, Ordering::SeqCst);
                Ok(json!(32.0))
            },
        ));
        agent.add_tool(add_tool);

        let result = agent.run_simple("What is 2 * 30?").await.unwrap();
        assert!(result.to_lowercase().contains("only perform addition"));
        assert_eq!(
            executed.load(Ordering::SeqCst),
            0,
            "add should be blocked by skill policy"
        );
    }

    #[tokio::test]
    async fn test_run_loop_executes_tool_for_valid_addition() {
        let llm = Arc::new(RunLoopDirectAddMockLlm::new());
        let mut agent = Agent::new(AgentConfig::default(), llm);
        let executed = Arc::new(AtomicUsize::new(0));
        let executed_ref = executed.clone();
        let add_tool = Arc::new(FunctionTool::numeric(
            "add",
            "Add two numbers",
            2,
            move |_args| {
                executed_ref.fetch_add(1, Ordering::SeqCst);
                Ok(json!(32.0))
            },
        ));
        agent.add_tool(add_tool);

        let result = agent.run_simple("What is 2 + 30?").await.unwrap();
        assert!(result.contains("32"));
        assert_eq!(
            executed.load(Ordering::SeqCst),
            1,
            "add should execute for valid addition"
        );
    }

    #[tokio::test]
    async fn test_stream_loop_executes_tool_for_valid_addition() {
        let llm = Arc::new(StreamLoopDirectAddMockLlm::new());
        let mut agent = Agent::new(AgentConfig::default(), llm);
        let executed = Arc::new(AtomicUsize::new(0));
        let executed_ref = executed.clone();
        let add_tool = Arc::new(FunctionTool::numeric(
            "add",
            "Add two numbers",
            2,
            move |_args| {
                executed_ref.fetch_add(1, Ordering::SeqCst);
                Ok(json!(32.0))
            },
        ));
        agent.add_tool(add_tool);

        let mut stream = agent.stream("What is 2 + 30?");
        let mut text = String::new();
        let mut saw_add_call = false;
        while let Some(item) = stream.next().await {
            match item.unwrap() {
                StreamToken::Text(s) => text.push_str(&s),
                StreamToken::ToolCall { name, .. } => {
                    if name == "add" {
                        saw_add_call = true;
                    }
                }
                StreamToken::Done => {}
            }
        }

        assert!(saw_add_call, "stream should emit add tool call");
        assert!(text.contains("32"), "stream should return final answer");
        assert_eq!(
            executed.load(Ordering::SeqCst),
            1,
            "add should execute in stream loop"
        );
    }

    #[tokio::test]
    async fn test_register_mcp_tools_initializes_client_if_needed() {
        use crate::mcp::McpClient;

        let llm = Arc::new(MockLlm {
            response: "test".to_string(),
        });
        let mut agent = Agent::new(AgentConfig::default(), llm);

        let mock_server_path = std::path::PathBuf::from("tests/fixtures/mock_mcp_server.py");
        let client = Arc::new(
            McpClient::spawn("python3", &[mock_server_path.to_str().unwrap()])
                .await
                .unwrap(),
        );

        let caps_before = client.get_capabilities().await;
        assert!(caps_before.is_none(), "client should start uninitialized");

        agent.register_mcp_tools(client.clone()).await.unwrap();

        let caps_after = client.get_capabilities().await;
        assert!(caps_after.is_some(), "client should be initialized");
        let registry = agent.registry().expect("registry should exist");
        assert!(
            registry.get("mcp/echo_tool").is_some(),
            "namespaced mcp tool should be added"
        );
    }

    #[tokio::test]
    async fn test_register_mcp_tools_returns_error_on_duplicate_name() {
        use crate::mcp::McpClient;

        let llm = Arc::new(MockLlm {
            response: "test".to_string(),
        });
        let mut agent = Agent::new(AgentConfig::default(), llm);

        let local_echo = Arc::new(FunctionTool::new(
            "mcp/echo_tool",
            "Local echo",
            serde_json::json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"]
            }),
            |_args| Ok(serde_json::json!({"ok": true})),
        ));
        agent.add_tool(local_echo);

        let mock_server_path = std::path::PathBuf::from("tests/fixtures/mock_mcp_server.py");
        let client = Arc::new(
            McpClient::spawn("python3", &[mock_server_path.to_str().unwrap()])
                .await
                .unwrap(),
        );

        let err = agent.register_mcp_tools(client).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to register MCP tool 'echo_tool'"),
            "expected duplicate registration error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_register_mcp_tools_is_atomic_when_preflight_fails() {
        use crate::mcp::McpClient;

        let llm = Arc::new(MockLlm {
            response: "test".to_string(),
        });
        let mut agent = Agent::new(AgentConfig::default(), llm);

        let mock_server_path =
            std::path::PathBuf::from("tests/fixtures/mock_mcp_server_duplicate_tools.py");
        let client = Arc::new(
            McpClient::spawn("python3", &[mock_server_path.to_str().unwrap()])
                .await
                .unwrap(),
        );

        let err = agent.register_mcp_tools(client).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Duplicate MCP tool name in server response"),
            "expected duplicate preflight error, got: {}",
            err
        );

        let registry = agent.registry().expect("registry should exist");
        let any_mcp_registered = registry.list().iter().any(|name| name.starts_with("mcp/"));
        assert!(
            !any_mcp_registered,
            "no mcp tools should be registered on preflight failure"
        );
    }

    #[tokio::test]
    async fn test_run_loop_auto_compacts_context_near_threshold() {
        let llm = Arc::new(ContextCaptureMockLlm::new());
        let agent = Agent::new(AgentConfig::default(), llm.clone());
        let mut session = AgentSession::new(agent);

        let large_turn = "x".repeat(1200);
        for i in 0..120 {
            session
                .context
                .add_user(format!("u{}:{}", i, large_turn.as_str()));
            session
                .context
                .add_assistant(format!("a{}:{}", i, large_turn.as_str()));
        }

        let _ = session.run("final question").await.unwrap();
        let requests = llm.requests.lock().unwrap();
        let saw_compaction_prompt = requests.iter().any(|req| {
            req.context.conversations.iter().any(|m| match m {
                OpenAiMessage::User { content } => {
                    content.contains("Summarize the following prior conversation for future turns")
                }
                _ => false,
            })
        });
        assert!(
            saw_compaction_prompt,
            "expected dedicated LLM compaction prompt request"
        );

        let saw_compaction = requests.iter().any(|req| {
            req.context.conversations.iter().any(|m| match m {
                OpenAiMessage::System { content } => {
                    content.contains(AgentSession::CONTEXT_COMPACTION_HEADER)
                }
                _ => false,
            })
        });
        assert!(saw_compaction, "expected compaction summary in request");
    }

    #[tokio::test]
    async fn test_stream_loop_auto_compacts_context_near_threshold() {
        let llm = Arc::new(ContextCaptureMockLlm::new());
        let agent = Agent::new(AgentConfig::default(), llm.clone());
        let mut session = AgentSession::new(agent);

        let large_turn = "y".repeat(1200);
        for i in 0..120 {
            session
                .context
                .add_user(format!("u{}:{}", i, large_turn.as_str()));
            session
                .context
                .add_assistant(format!("a{}:{}", i, large_turn.as_str()));
        }

        let mut stream = session.stream("stream final question");
        while stream.next().await.is_some() {}

        let requests = llm.requests.lock().unwrap();
        let saw_compaction_prompt = requests.iter().any(|req| {
            req.context.conversations.iter().any(|m| match m {
                OpenAiMessage::User { content } => {
                    content.contains("Summarize the following prior conversation for future turns")
                }
                _ => false,
            })
        });
        assert!(
            saw_compaction_prompt,
            "expected dedicated LLM compaction prompt request in stream loop"
        );

        let saw_compaction = requests.iter().any(|req| {
            req.context.conversations.iter().any(|m| match m {
                OpenAiMessage::System { content } => {
                    content.contains(AgentSession::CONTEXT_COMPACTION_HEADER)
                }
                _ => false,
            })
        });
        assert!(
            saw_compaction,
            "expected compaction summary in stream request"
        );
    }

    #[tokio::test]
    async fn test_stream_updates_session_context_for_export() {
        let llm = Arc::new(MockLlm {
            response: "stream-hi".to_string(),
        });
        let agent = Agent::new(AgentConfig::default(), llm);
        let mut session = AgentSession::new(agent);

        let mut stream = session.stream("hello-stream");
        while stream.next().await.is_some() {}
        drop(stream);

        let exported = session.export_state("stream-persist");
        assert_eq!(
            exported.message_log.len(),
            2,
            "stream should persist user+assistant in session context"
        );
    }

    #[test]
    fn test_restore_then_export_preserves_session_metadata() {
        let llm = Arc::new(MockLlm {
            response: "ok".to_string(),
        });
        let agent = Agent::new(AgentConfig::default(), llm);

        let mut state = crate::session::SessionSerializer::new_state("meta-agent".to_string());
        state.metadata.created_at = 111;
        state.metadata.updated_at = 222;
        state.metadata.expires_at = Some(333);
        state.metadata.labels = vec!["keep-me".to_string()];
        state.metadata.agent_version = "vtest".to_string();

        let session = AgentSession::restore_from_state(agent, state);
        let exported = session.export_state("meta-agent");

        assert_eq!(exported.metadata.created_at, 111);
        assert_eq!(exported.metadata.expires_at, Some(333));
        assert_eq!(exported.metadata.labels, vec!["keep-me".to_string()]);
        assert_eq!(exported.metadata.agent_version, "vtest");
    }

    #[tokio::test]
    async fn test_register_mcp_tools_with_namespace_rejects_invalid_namespace() {
        use crate::mcp::McpClient;
        let llm = Arc::new(MockLlm {
            response: "test".to_string(),
        });
        let mut agent = Agent::new(AgentConfig::default(), llm);

        let mock_server_path = std::path::PathBuf::from("tests/fixtures/mock_mcp_server.py");
        let client = Arc::new(
            McpClient::spawn("python3", &[mock_server_path.to_str().unwrap()])
                .await
                .unwrap(),
        );

        let err = agent
            .register_mcp_tools_with_namespace(client, "bad/ns")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Invalid MCP namespace"),
            "expected invalid namespace error, got: {}",
            err
        );
    }

    #[test]
    fn test_session_export_and_restore_round_trip() {
        let llm = Arc::new(MockLlm {
            response: "ok".to_string(),
        });
        let agent = Agent::new(AgentConfig::default(), llm);
        let mut session = AgentSession::new(agent.clone());

        session.context.add_user("hello".to_string());
        session.context.add_assistant("world".to_string());
        session.context.add_tool_call(
            "tc-1".to_string(),
            "demo_tool".to_string(),
            serde_json::json!({"a": 1}),
        );
        session
            .context
            .add_tool_result("tc-1".to_string(), "done".to_string());

        let state = session.export_state("agent-1");
        assert_eq!(state.agent_id, "agent-1");
        assert_eq!(state.message_log.len(), 4);
        assert_eq!(state.metadata.message_count, 4);

        let restored = AgentSession::restore_from_state(agent, state);
        assert_eq!(restored.context.len(), 4);

        match &restored.context.messages[0] {
            react::Message::User { content } => assert_eq!(content, "hello"),
            other => panic!("unexpected first restored message: {:?}", other),
        }
    }

    #[test]
    fn test_session_export_and_restore_through_serializer() {
        let llm = Arc::new(MockLlm {
            response: "ok".to_string(),
        });
        let agent = Agent::new(AgentConfig::default(), llm);
        let mut session = AgentSession::new(agent.clone());
        session.context.add_user("persist me".to_string());
        session.context.add_assistant("restored".to_string());

        let state = session.export_state("agent-serialize");
        let bytes = crate::session::SessionSerializer::serialize(&state).unwrap();
        let restored_state = crate::session::SessionSerializer::deserialize(&bytes).unwrap();
        let restored = AgentSession::restore_from_state(agent, restored_state);

        assert_eq!(restored.context.len(), 2);
        match &restored.context.messages[1] {
            react::Message::Assistant { content } => assert_eq!(content, "restored"),
            other => panic!("unexpected second restored message: {:?}", other),
        }
    }
}
