use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use zenoh::Session;

use crate::a2a::{A2ADiscovery, AgentCard, AgentStatus};

use super::base::{ToolDiscovery, DiscoveredTool, ToolSource};

/// Configuration for A2A tool discovery.
#[derive(Debug, Clone)]
pub struct A2AToolDiscoveryConfig {
    /// Filter by agent capability
    pub capability_filter: Option<String>,
    /// Filter by agent skill
    pub skill_filter: Option<String>,
    /// Only discover online agents
    pub online_only: bool,
    /// Discovery timeout
    pub timeout: Duration,
}

impl Default for A2AToolDiscoveryConfig {
    fn default() -> Self {
        Self {
            capability_filter: None,
            skill_filter: None,
            online_only: true,
            timeout: Duration::from_secs(5),
        }
    }
}

/// Discovery for tools from other agents via A2A protocol.
///
/// Uses `A2ADiscovery` to find agents and extracts tool information
/// from their `AgentCard` capabilities.
pub struct A2AToolDiscovery {
    session: Arc<Session>,
    config: A2AToolDiscoveryConfig,
}

impl A2AToolDiscovery {
    /// Create a new A2A tool discovery with default config.
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            config: A2AToolDiscoveryConfig::default(),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(session: Arc<Session>, config: A2AToolDiscoveryConfig) -> Self {
        Self { session, config }
    }

    /// Filter by capability.
    pub fn capability(mut self, capability: impl Into<String>) -> Self {
        self.config.capability_filter = Some(capability.into());
        self
    }

    /// Filter by skill.
    pub fn skill(mut self, skill: impl Into<String>) -> Self {
        self.config.skill_filter = Some(skill.into());
        self
    }

    /// Include offline agents.
    pub fn include_offline(mut self) -> Self {
        self.config.online_only = false;
        self
    }

    /// Set discovery timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    fn extract_tools_from_card(card: &AgentCard) -> Vec<DiscoveredTool> {
        let mut tools = Vec::new();
        
        // Convert capabilities to tool definitions
        for cap in &card.capabilities {
            // Generate a schema for the capability
            // Real implementation would query the agent for actual tool schemas
            let input_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": format!("Input for {}", cap.name)
                    }
                },
                "required": ["input"]
            });

            let tool = DiscoveredTool {
                name: cap.name.clone(),
                description: cap.description.clone(),
                input_schema,
                source: ToolSource::A2A {
                    agent_id: card.agent_id.id.clone(),
                    agent_name: card.name.clone(),
                    endpoint: card.endpoints.first()
                        .map(|e| e.address.clone())
                        .unwrap_or_default(),
                },
            };
            tools.push(tool);
        }

        // Also create tools from skills (skills often expose tools)
        for skill in &card.skills {
            let input_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": format!("Query for {} skill", skill)
                    }
                },
                "required": ["query"]
            });

            let tool = DiscoveredTool {
                name: format!("skill_{}", skill),
                description: format!("Tool from {} skill", skill),
                input_schema,
                source: ToolSource::A2A {
                    agent_id: card.agent_id.id.clone(),
                    agent_name: card.name.clone(),
                    endpoint: card.endpoints.first()
                        .map(|e| e.address.clone())
                        .unwrap_or_default(),
                },
            };
            tools.push(tool);
        }

        tools
    }
}

#[async_trait]
impl ToolDiscovery for A2AToolDiscovery {
    async fn discover_all(&self) -> Result<Vec<DiscoveredTool>, crate::error::ToolError> {
        let discovery = A2ADiscovery::with_timeout(
            self.session.clone(),
            self.config.timeout,
        );

        // Apply filters
        let cards = if let Some(ref capability) = self.config.capability_filter {
            discovery.discover(Some(capability)).await
                .map_err(|e| crate::error::ToolError::ExecutionFailed(e.to_string()))?
        } else if let Some(ref skill) = self.config.skill_filter {
            discovery.discover_by_skill(skill).await
                .map_err(|e| crate::error::ToolError::ExecutionFailed(e.to_string()))?
        } else {
            discovery.discover(None).await
                .map_err(|e| crate::error::ToolError::ExecutionFailed(e.to_string()))?
        };

        // Filter by status if needed
        let filtered_cards: Vec<AgentCard> = if self.config.online_only {
            cards.into_iter()
                .filter(|c| c.status == AgentStatus::Online)
                .collect()
        } else {
            cards
        };

        // Extract tools from all discovered agents
        Ok(filtered_cards
            .iter()
            .flat_map(|card| Self::extract_tools_from_card(card))
            .collect())
    }

    fn source_name(&self) -> &'static str {
        "a2a"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::{AgentIdentity, Endpoint, Capability};

    fn create_test_card(agent_id: &str, name: &str) -> AgentCard {
        AgentCard::new(
            AgentIdentity::new(agent_id.to_string(), name.to_string(), "1.0.0".to_string()),
            name.to_string(),
            format!("{} agent", name),
        )
        .with_capability("calculate".to_string(), "Perform calculations".to_string())
        .with_skill("math".to_string())
        .with_endpoint(Endpoint::zenoh(format!("agent/{}", agent_id)))
        .with_status(AgentStatus::Online)
    }

    #[test]
    fn test_extract_tools_from_card() {
        let card = create_test_card("test-agent", "TestAgent");
        let tools = A2AToolDiscovery::extract_tools_from_card(&card);
        
        // Should have tools from capabilities and skills
        assert!(!tools.is_empty());
        
        // Check capability-based tool
        let cap_tool = tools.iter().find(|t| t.name == "calculate");
        assert!(cap_tool.is_some());
        
        // Check skill-based tool
        let skill_tool = tools.iter().find(|t| t.name == "skill_math");
        assert!(skill_tool.is_some());
    }

    #[test]
    fn test_tool_source_extraction() {
        let card = create_test_card("calc-agent", "Calculator");
        let tools = A2AToolDiscovery::extract_tools_from_card(&card);
        
        let tool = &tools[0];
        match &tool.source {
            ToolSource::A2A { agent_id, agent_name, .. } => {
                assert_eq!(agent_id, "calc-agent");
                assert_eq!(agent_name, "Calculator");
            }
            _ => panic!("Expected A2A source"),
        }
    }

    #[test]
    fn test_config_default() {
        let config = A2AToolDiscoveryConfig::default();
        assert!(config.capability_filter.is_none());
        assert!(config.skill_filter.is_none());
        assert!(config.online_only);
        assert_eq!(config.timeout, Duration::from_secs(5));
    }
}
