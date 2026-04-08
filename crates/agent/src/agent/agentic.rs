use crate::tools::FunctionTool;
use crate::{
    AgentError, LlmClient, LlmResponse, StreamToken, Tool, ToolRegistry,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use log::{info, warn};
use qserde::{archive, Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;

use react::engine::ReActEngineBuilder;
use react::llm::{
    LlmClient as ReactLlmTrait, LlmError as ReactLlmError,
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


// Note: Removed rkyv/qserde serialization attempts as they require implementing 
// Archive on all nested types (AgentConfig, ReActResilience, SkillContent).
// For proper serialization, consider using serde with bincode, or implement
// rkyv::Archive for all dependent types first.
// The manual Clone impl exists at line 843 for this reason.

/// Agent is the main abstraction for AI agents with LLM integration,
/// tool registries, and skill management.
/// Serialization via qserde (rkyv) - llm field is skipped as it's a trait object.
#[derive(Debug)]
#[archive(crate_path = qserde)]
pub struct Agent {
    config: AgentConfig,
    /// The LLM client - not serializable (trait object)
    llm: Arc<dyn LlmClient>,
    /// Tool registry for available tools
    registry: Option<Arc<ToolRegistry>>,
    /// Directory for loading skills
    skills_dir: Option<std::path::PathBuf>,
    /// Loaded skills
    skills: Vec<crate::skills::SkillContent>,
    /// Resilience configuration for ReAct loop
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