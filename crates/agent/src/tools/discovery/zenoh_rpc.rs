use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use zenoh::Session;

use bus::DiscoveryRegistry;

use super::base::{ToolDiscovery, DiscoveredTool, ToolSource};

/// Configuration for Zenoh RPC discovery.
#[derive(Debug, Clone)]
pub struct ZenohRpcDiscoveryConfig {
    /// Prefix filter for service names (e.g., "agent/")
    pub service_prefix: Option<String>,
    /// Discovery timeout
    pub timeout: Duration,
}

impl Default for ZenohRpcDiscoveryConfig {
    fn default() -> Self {
        Self {
            service_prefix: None,
            timeout: Duration::from_secs(2),
        }
    }
}

/// Discovery for tools exposed via Zenoh RPC services.
///
/// Uses `DiscoveryRegistry` to find advertised RPC services and
/// extracts tool information from service metadata.
pub struct ZenohRpcDiscovery {
    session: Arc<Session>,
    config: ZenohRpcDiscoveryConfig,
}

impl ZenohRpcDiscovery {
    /// Create a new Zenoh RPC discovery with default config.
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            config: ZenohRpcDiscoveryConfig::default(),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(session: Arc<Session>, config: ZenohRpcDiscoveryConfig) -> Self {
        Self { session, config }
    }

    /// Set service prefix filter.
    pub fn service_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.config.service_prefix = Some(prefix.into());
        self
    }

    /// Set discovery timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    fn parse_tool_name(topic_prefix: &str) -> Option<(String, String)> {
        // Expected format: "agent/{agent_id}/tools/{tool_name}" or "rpc/{service_name}/{tool_name}"
        let parts: Vec<&str> = topic_prefix.split('/').collect();
        
        // Format: agent/{agent_id}/tools/{tool_name}
        if parts.len() >= 4 && parts[0] == "agent" && parts[2] == "tools" {
            let agent_id = parts[1].to_string();
            let tool_name = parts[3].to_string();
            return Some((agent_id, tool_name));
        }
        
        // Format: rpc/{service_name}/{tool_name}
        if parts.len() >= 3 && parts[0] == "rpc" {
            let service_name = parts[1].to_string();
            let tool_name = parts[2..].join("/");
            return Some((service_name, tool_name));
        }
        
        None
    }
}

#[async_trait]
impl ToolDiscovery for ZenohRpcDiscovery {
    async fn discover_all(&self) -> Result<Vec<DiscoveredTool>, crate::error::ToolError> {
        let registry = DiscoveryRegistry::new()
            .session(self.session.clone())
            .timeout(self.config.timeout);

        let services = registry.list_services().await
            .map_err(|e| crate::error::ToolError::ExecutionFailed(e.to_string()))?;

        let mut tools = Vec::new();
        
        for service in services {
            // Apply prefix filter if configured
            if let Some(ref prefix) = self.config.service_prefix {
                if !service.topic_prefix.starts_with(prefix) {
                    continue;
                }
            }

            // Parse tool name from topic prefix
            if let Some((agent_or_service, tool_name)) = Self::parse_tool_name(&service.topic_prefix) {
                // Create a discovered tool with default schema
                // Actual schema would need to be queried from the service
                let tool = DiscoveredTool {
                    name: tool_name.clone(),
                    description: format!("Remote tool: {} (via {})", tool_name, service.service_name),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "description": "Remote tool - schema unknown, contact service for details"
                    }),
                    source: ToolSource::ZenohRpc {
                        service_name: service.service_name.clone(),
                        agent_id: if service.topic_prefix.starts_with("agent/") {
                            Some(agent_or_service.clone())
                        } else {
                            None
                        },
                    },
                };
                tools.push(tool);
            }
        }

        Ok(tools)
    }

    fn source_name(&self) -> &'static str {
        "zenoh_rpc"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_name_agent_format() {
        let result = ZenohRpcDiscovery::parse_tool_name("agent/bob/tools/add");
        assert_eq!(result, Some(("bob".to_string(), "add".to_string())));
    }

    #[test]
    fn test_parse_tool_name_rpc_format() {
        let result = ZenohRpcDiscovery::parse_tool_name("rpc/calculator/add");
        assert_eq!(result, Some(("calculator".to_string(), "add".to_string())));
    }

    #[test]
    fn test_parse_tool_name_invalid() {
        assert!(ZenohRpcDiscovery::parse_tool_name("invalid/format").is_none());
        assert!(ZenohRpcDiscovery::parse_tool_name("agent/bob/invalid/add").is_none());
    }

    #[test]
    fn test_config_default() {
        let config = ZenohRpcDiscoveryConfig::default();
        assert!(config.service_prefix.is_none());
        assert_eq!(config.timeout, Duration::from_secs(2));
    }
}
