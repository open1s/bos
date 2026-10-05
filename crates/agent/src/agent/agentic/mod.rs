use crate::agent::context::{AgentReActApp, AgentReactContext, AgentSession};
use crate::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent, HookRegistry};
use crate::agent::plugin::{AgentPlugin, PluginRegistry, StreamTokenWrapper};
use crate::session::AgentState;
use crate::tools::FunctionTool;
use crate::{AgentError, LlmClient, StreamToken, Tool, ToolRegistry};
use async_trait::async_trait;
use bus::Bus;
use futures::{Stream, StreamExt};
use log::warn;
use react::LlmMessage;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;

use react::engine::{ReActEngine, ReActEngineBuilder};
use react::llm::vendor::{DeepSeekVendor, LlmRouter, NvidiaVendor, OpenAiClient, OpenRouterVendor};
use react::llm::{
    Content, LlmError as ReactLlmError, LlmResponse as ReactLlmResponse,
    TokenStream as ReactTokenStream, TokenStream,
};
use react::tool::registry::{AsyncTool, ToolVariant};
use react::tool::{Tool as ReactToolTrait, ToolError as ReactToolError};
use react::{CircuitBreakerConfig, LlmRequest, RateLimiterConfig, ReActResilience};

mod adapters;
mod engine;
mod llm;

use adapters::{AsyncExtensibleToolAdapter, ExtensibleToolAdapter};
pub use llm::{build_vendor, LlmProvider};

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
    /// API protocol selection: `"chat"` (default, `/chat/completions`) or
    /// `"responses"` (`/v1/responses`). Kept as a string so it archives cleanly
    /// under qserde/rkyv.
    pub api_mode: String,
    /// Reasoning effort for reasoning models: `"low"`, `"medium"` (default), or
    /// `"high"`. Kept as an optional string so it archives cleanly under qserde/rkyv.
    pub reasoning_effort: Option<String>,
    /// Circuit breaker configuration for resilience
    pub circuit_breaker: Option<CircuitBreakerConfig>,
    /// Rate limiter configuration for resilience
    pub rate_limit: Option<RateLimiterConfig>,
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
            api_mode: "chat".to_string(),
            reasoning_effort: None,
            circuit_breaker: None,
            rate_limit: None,
        }
    }
}

/// Fluent setters, so a programmatic config reads like the Python/JS APIs:
/// `AgentConfig::default().name("assistant").model("openai/gpt-4o")`.
impl AgentConfig {
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = api_key.into();
        self
    }
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }
    pub fn max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }
    /// `"chat"` (default) or `"responses"`.
    pub fn api_mode(mut self, api_mode: impl Into<String>) -> Self {
        self.api_mode = api_mode.into();
        self
    }
    /// `"low"`, `"medium"` or `"high"`.
    pub fn reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }
    pub fn circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker = Some(config);
        self
    }
    pub fn rate_limit(mut self, config: RateLimiterConfig) -> Self {
        self.rate_limit = Some(config);
        self
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
    metrics: std::sync::Arc<crate::metrics::MetricsCollector>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    hooks: HookRegistry,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    plugins: PluginRegistry,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    bus: Option<Bus>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    engine_cache: std::sync::Mutex<Option<ReActEngine<AgentReActApp>>>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    context_cache: std::sync::Mutex<Option<AgentReactContext>>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    last_stream_tokens: std::sync::Mutex<Option<(u64, u64)>>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    last_stream_tool_calls: std::sync::Mutex<u64>,
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
            metrics: std::sync::Arc::new(crate::metrics::MetricsCollector::new()),
            hooks: HookRegistry::new(),
            plugins: PluginRegistry::new(),
            bus: None,
            engine_cache: std::sync::Mutex::new(None),
            context_cache: std::sync::Mutex::new(None),
            last_stream_tokens: std::sync::Mutex::new(None),
            last_stream_tool_calls: std::sync::Mutex::new(0),
        }
    }

    /// Create an agent from a programmatic [`AgentConfig`], constructing the
    /// LLM provider from `model` / `base_url` / `api_key`.
    ///
    /// This is the one-call path for code that configures an agent directly
    /// instead of loading TOML; it does not read the home config.
    ///
    /// ```
    /// use agent::{Agent, AgentConfig};
    /// let agent = Agent::from_config(AgentConfig::default().name("assistant"));
    /// assert_eq!(agent.config().name, "assistant");
    /// ```
    pub fn from_config(config: AgentConfig) -> Self {
        let mut llm = LlmProvider::new();
        let (vendor_name, vendor) = build_vendor(&config);
        llm.register_vendor(vendor_name, vendor);
        Self::new(config, Arc::new(llm))
    }

    /// Set the bus for tool event publishing.
    pub fn with_bus(mut self, bus: Bus) -> Self {
        self.bus = Some(bus);
        self
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

    pub fn session(&self) -> std::sync::MutexGuard<'_, AgentSession> {
        self.session.lock().unwrap()
    }

    pub fn session_mut(&mut self) -> std::sync::MutexGuard<'_, AgentSession> {
        self.session.lock().unwrap()
    }

    pub fn metrics(&self) -> crate::metrics::CallMetrics {
        self.metrics.snapshot()
    }

    pub fn record_stream_call(
        &self,
        wall_time: std::time::Duration,
        engine_time: std::time::Duration,
        resilience_time: std::time::Duration,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        self.metrics.record_call(
            wall_time,
            engine_time,
            resilience_time,
            input_tokens,
            output_tokens,
        );
    }

    pub fn record_llm_error(&self) {
        self.metrics.record_llm_error();
    }

    pub fn record_tool_calls(&self, count: u64, time: std::time::Duration) {
        self.metrics.record_tool_calls(count, time);
    }

    pub fn last_token_usage(&self) -> Option<(u64, u64)> {
        *self.last_stream_tokens.lock().unwrap()
    }

    pub fn last_stream_tool_calls(&self) -> u64 {
        *self.last_stream_tool_calls.lock().unwrap()
    }

    pub fn tool_invocation_count(&self) -> u64 {
        let cache = self.engine_cache.lock().unwrap();
        cache.as_ref().map(|e| e.tool_call_count()).unwrap_or(0)
    }

    pub fn reset_metrics(&self) {
        self.metrics.reset()
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
        self.engine_cache.lock().unwrap().take();
        self.context_cache.lock().unwrap().take();
    }

    /// Signal the agent to stop the current execution.
    /// Returns true if there was a running execution to stop.
    pub fn stop(&self) {
        let cached_engine = {
            let mut cache = self.engine_cache.lock().unwrap();
            cache.take()
        };

        if let Some(mut e) = cached_engine {
            e.stop();
        }
    }

    /// Mark a tool name as an MCP-registered tool.
    pub fn mark_mcp_tool(&mut self, name: &str) {
        if let Some(ref mut reg) = self.registry {
            Arc::make_mut(reg).mark_mcp_tool(name);
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
        self.engine_cache.lock().unwrap().take();
        self.context_cache.lock().unwrap().take();
        Ok(())
    }

    /// Register an async tool and return explicit error on failure.
    pub fn try_add_async_tool(
        &mut self,
        tool: Arc<dyn react::tool::registry::AsyncTool>,
    ) -> Result<(), crate::ToolError> {
        if let Some(ref mut reg) = self.registry {
            Arc::make_mut(reg).register_async(tool)?;
        } else {
            let mut reg = ToolRegistry::new();
            reg.register_async(tool)?;
            self.registry = Some(Arc::new(reg));
        }
        self.engine_cache.lock().unwrap().take();
        self.context_cache.lock().unwrap().take();
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
        self.engine_cache.lock().unwrap().take();
        self.context_cache.lock().unwrap().take();
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

            let namespaced_name = format!("{}_{}", namespace, tool.name);
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
            let namespaced_name = format!("{}_{}", namespace, tool_name);
            reg_mut.mark_mcp_tool(&namespaced_name);
            let mcp_tool: std::sync::Arc<dyn react::tool::registry::AsyncTool> =
                std::sync::Arc::new(McpToolAdapter::new(
                    client.clone(),
                    namespaced_name.clone(),
                    tool_name.clone(),
                    tool.description.clone(),
                    schema,
                ));
            reg_mut.register_async(mcp_tool).map_err(|e| {
                crate::mcp::McpError::Protocol(format!(
                    "Failed to register MCP tool '{}': {}",
                    namespaced_name, e
                ))
            })?;
        }
        self.engine_cache.lock().unwrap().take();
        self.context_cache.lock().unwrap().take();
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
            metrics: self.metrics.clone(),
            hooks: self.hooks.clone(),
            plugins: self.plugins.clone(),
            bus: self.bus.clone(),
            engine_cache: std::sync::Mutex::new(None),
            context_cache: std::sync::Mutex::new(None),
            last_stream_tokens: std::sync::Mutex::new(None),
            last_stream_tool_calls: std::sync::Mutex::new(0),
        }
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests;

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
