use std::sync::Arc;
use zenoh::Session;

use super::{ToolRegistry, Tool, ToolError};
use super::discovery::{ToolDiscovery, DiscoveredTool, ToolSource};
use super::a2a_client::A2AToolClient;
use crate::mcp::McpToolAdapter;
use crate::mcp::McpClient;

impl Default for UnifiedRegistryConfig {
    fn default() -> Self {
        Self {
            auto_discover: false,
            use_namespace: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnifiedRegistryConfig {
    pub auto_discover: bool,
    pub use_namespace: bool,
}

/// Registry that aggregates tools from multiple discovery sources.
///
/// Provides a unified interface for discovering and registering tools
/// from local, RPC, MCP, and A2A sources.
pub struct UnifiedToolRegistry {
    registry: ToolRegistry,
    discovery_sources: Vec<Arc<dyn ToolDiscovery>>,
    zenoh_session: Option<Arc<Session>>,
    mcp_clients: Vec<Arc<McpClient>>,
    config: UnifiedRegistryConfig,
}

impl UnifiedToolRegistry {
    /// Create a new unified registry with default config.
    pub fn new() -> Self {
        Self {
            registry: ToolRegistry::new(),
            discovery_sources: Vec::new(),
            zenoh_session: None,
            mcp_clients: Vec::new(),
            config: UnifiedRegistryConfig::default(),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(config: UnifiedRegistryConfig) -> Self {
        Self {
            registry: ToolRegistry::new(),
            discovery_sources: Vec::new(),
            zenoh_session: None,
            mcp_clients: Vec::new(),
            config,
        }
    }

    /// Set the Zenoh session for RPC and A2A discovery.
    pub fn with_zenoh_session(mut self, session: Arc<Session>) -> Self {
        self.zenoh_session = Some(session);
        self
    }

    /// Add a discovery source.
    pub fn add_discovery_source(mut self, source: Arc<dyn ToolDiscovery>) -> Self {
        self.discovery_sources.push(source);
        self
    }

    /// Add an MCP client for tool discovery.
    pub fn add_mcp_client(mut self, client: Arc<McpClient>) -> Self {
        self.mcp_clients.push(client);
        self
    }

    /// Get the underlying ToolRegistry.
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Get mutable access to the ToolRegistry.
    pub fn registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.registry
    }

    /// Discover and register tools from all sources.
    pub async fn discover_and_register_all(&mut self) -> Result<Vec<String>, ToolError> {
        let mut all_discovered: Vec<DiscoveredTool> = Vec::new();
        
        for source in &self.discovery_sources {
            let tools = source.discover_all().await?;
            all_discovered.extend(tools);
        }

        let mut registered_names = Vec::new();
        for discovered in all_discovered {
            let name = self.register_discovered_tool(discovered).await?;
            registered_names.push(name);
        }

        Ok(registered_names)
    }

    /// Register a discovered tool with the registry.
    async fn register_discovered_tool(&mut self, tool: DiscoveredTool) -> Result<String, ToolError> {
        let tool_impl: Arc<dyn Tool> = match &tool.source {
            ToolSource::Local => {
                // Local tools should already be registered
                return Err(ToolError::ExecutionFailed(
                    "Local tools should be registered directly".to_string()
                ));
            }
            ToolSource::ZenohRpc { service_name, agent_id: _ } => {
                let session = self.zenoh_session.as_ref().ok_or_else(|| {
                    ToolError::ExecutionFailed("Zenoh session not configured".to_string())
                })?;
                
                Arc::new(super::BusToolClient::new(
                    session.clone(),
                    service_name.clone(),
                    tool.name.clone(),
                ))
            }
            ToolSource::Mcp { server_name, .. } => {
                let client = self.mcp_clients.iter()
                    .find(|_c| {
                        // Match by server name - would need proper tracking
                        true
                    })
                    .ok_or_else(|| {
                        ToolError::ExecutionFailed(format!("MCP client not found for {}", server_name))
                    })?;
                
                Arc::new(McpToolAdapter::new(
                    client.clone(),
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.input_schema.clone(),
                ))
            }
            ToolSource::A2A { agent_id, agent_name, .. } => {
                let session = self.zenoh_session.as_ref().ok_or_else(|| {
                    ToolError::ExecutionFailed("Zenoh session not configured".to_string())
                })?;
                
                let target = crate::a2a::AgentIdentity::new(
                    agent_id.clone(),
                    agent_name.clone(),
                    "1.0.0".to_string(),
                );
                
                Arc::new(A2AToolClient::new(
                    session.clone(),
                    target,
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.input_schema.clone(),
                ))
            }
            ToolSource::Skill { skill_name } => {
                // Skills would need a different handler
                return Err(ToolError::ExecutionFailed(format!(
                    "Skill tools not yet supported: {}", skill_name
                )));
            }
        };

        let name = if self.config.use_namespace {
            tool.namespaced_name()
        } else {
            tool.name.clone()
        };

        if self.config.use_namespace {
            let namespace = tool.source.namespace();
            self.registry.register_with_namespace(tool_impl, &namespace)?;
        } else {
            self.registry.register(tool_impl)?;
        }

        Ok(name)
    }

    /// Register a local tool directly.
    pub fn register_local(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        self.registry.register(tool)
    }

    /// Register a local tool with namespace.
    pub fn register_local_with_namespace(
        &mut self,
        tool: Arc<dyn Tool>,
        namespace: &str,
    ) -> Result<(), ToolError> {
        self.registry.register_with_namespace(tool, namespace)
    }

    /// Convert all registered tools to OpenAI format.
    pub fn to_openai_format(&self) -> Vec<serde_json::Value> {
        self.registry.to_openai_format()
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.registry.get(name)
    }

    /// Execute a tool by name.
    pub async fn execute(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        self.registry.execute(name, args).await
    }

    /// List all registered tool names.
    pub fn list(&self) -> Vec<String> {
        self.registry.list()
    }
}

impl Default for UnifiedToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_registry_creation() {
        let registry = UnifiedToolRegistry::new();
        assert!(registry.discovery_sources.is_empty());
        assert!(registry.mcp_clients.is_empty());
        assert!(registry.zenoh_session.is_none());
    }

    #[test]
    fn test_config_default() {
        let config = UnifiedRegistryConfig::default();
        assert!(!config.auto_discover);
        assert!(config.use_namespace);
    }

    #[test]
    fn test_empty_registry_list() {
        let registry = UnifiedToolRegistry::new();
        let tools = registry.list();
        assert!(tools.is_empty());
    }

    #[test]
    fn test_empty_openai_format() {
        let registry = UnifiedToolRegistry::new();
        let format = registry.to_openai_format();
        assert!(format.is_empty());
    }
}
