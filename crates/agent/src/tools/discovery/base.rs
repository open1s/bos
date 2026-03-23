//! Unified Tool Discovery Interface
//!
//! This module provides a unified interface for discovering tools from multiple sources:
//! - Local tools (already registered in registry)
//! - Zenoh RPC services (remote tools on other agents)
//! - MCP servers (Model Context Protocol)
//! - A2A protocol (Agent-to-Agent tool calls)

use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;
use crate::tools::ToolRegistry;

/// Source identifier for a discovered tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolSource {
    /// Local tool (already registered)
    Local,
    /// Tool exposed via Zenoh RPC service
    ZenohRpc {
        service_name: String,
        agent_id: Option<String>,
    },
    /// Tool from an MCP server
    Mcp {
        server_name: String,
        server_command: String,
    },
    /// Tool from another agent via A2A protocol
    A2A {
        agent_id: String,
        agent_name: String,
        endpoint: String,
    },
    /// Tool from a skill
    Skill {
        skill_name: String,
    },
}

impl ToolSource {
    pub fn namespace(&self) -> String {
        match self {
            ToolSource::Local => "local".to_string(),
            ToolSource::ZenohRpc { service_name, .. } => format!("rpc/{}", service_name),
            ToolSource::Mcp { server_name, .. } => format!("mcp/{}", server_name),
            ToolSource::A2A { agent_id, .. } => format!("a2a/{}", agent_id),
            ToolSource::Skill { skill_name } => format!("skill/{}", skill_name),
        }
    }
}

/// Metadata for a discovered tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub source: ToolSource,
}

impl DiscoveredTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        source: ToolSource,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            source,
        }
    }

    pub fn namespaced_name(&self) -> String {
        format!("{}/{}", self.source.namespace(), self.name)
    }

    pub fn to_openai_format(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.namespaced_name(),
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }
}

/// Trait for discovering tools from various sources.
#[async_trait]
pub trait ToolDiscovery: Send + Sync {
    async fn discover_all(&self) -> Result<Vec<DiscoveredTool>, ToolError>;

    async fn search(&self, query: &str) -> Result<Vec<DiscoveredTool>, ToolError> {
        let all = self.discover_all().await?;
        let query_lower = query.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|t| {
                t.name.to_lowercase().contains(&query_lower)
                    || t.description.to_lowercase().contains(&query_lower)
            })
            .collect())
    }

    fn source_name(&self) -> &'static str;
}

/// Discovery for local tools (already registered in registry).
pub struct LocalDiscovery {
    registry: Arc<ToolRegistry>,
}

impl LocalDiscovery {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolDiscovery for LocalDiscovery {
    async fn discover_all(&self) -> Result<Vec<DiscoveredTool>, ToolError> {
        let tools = self.registry.list();
        Ok(tools
            .into_iter()
            .filter_map(|name| {
                self.registry.get(&name).map(|tool| {
                    let desc = tool.description();
                    DiscoveredTool {
                        name,
                        description: desc.short,
                        input_schema: tool.json_schema(),
                        source: ToolSource::Local,
                    }
                })
            })
            .collect())
    }

    fn source_name(&self) -> &'static str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_source_namespace() {
        let local = ToolSource::Local;
        assert_eq!(local.namespace(), "local");

        let rpc = ToolSource::ZenohRpc {
            service_name: "agent/bob/tools".to_string(),
            agent_id: Some("bob".to_string()),
        };
        assert_eq!(rpc.namespace(), "rpc/agent/bob/tools");

        let mcp = ToolSource::Mcp {
            server_name: "filesystem".to_string(),
            server_command: "npx".to_string(),
        };
        assert_eq!(mcp.namespace(), "mcp/filesystem");

        let a2a = ToolSource::A2A {
            agent_id: "agent-123".to_string(),
            agent_name: "Calculator".to_string(),
            endpoint: "zenoh://agent/agent-123".to_string(),
        };
        assert_eq!(a2a.namespace(), "a2a/agent-123");
    }

    #[test]
    fn test_discovered_tool_namespaced_name() {
        let tool = DiscoveredTool {
            name: "add".to_string(),
            description: "Add two numbers".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            source: ToolSource::ZenohRpc {
                service_name: "calculator".to_string(),
                agent_id: None,
            },
        };
        assert_eq!(tool.namespaced_name(), "rpc/calculator/add");
    }

    #[test]
    fn test_discovered_tool_openai_format() {
        let tool = DiscoveredTool {
            name: "multiply".to_string(),
            description: "Multiply two numbers".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "a": {"type": "number"},
                    "b": {"type": "number"}
                }
            }),
            source: ToolSource::Local,
        };

        let format = tool.to_openai_format();
        assert_eq!(format["type"], "function");
        assert_eq!(format["function"]["name"], "local/multiply");
        assert_eq!(format["function"]["description"], "Multiply two numbers");
    }
}
