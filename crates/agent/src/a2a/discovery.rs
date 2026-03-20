use std::sync::Arc;
use std::time::Duration;
use zenoh::Session;

use super::AgentIdentity;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Endpoint {
    pub protocol: String,
    pub address: String,
}

impl Endpoint {
    pub fn new(protocol: String, address: String) -> Self {
        Self { protocol, address }
    }

    pub fn zenoh(topic: String) -> Self {
        Self {
            protocol: "zenoh".to_string(),
            address: topic,
        }
    }

    pub fn http(url: String) -> Self {
        Self {
            protocol: "http".to_string(),
            address: url,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentCard {
    pub agent_id: AgentIdentity,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub supported_protocols: Vec<String>,
    pub endpoints: Vec<Endpoint>,
    pub skills: Vec<String>,
    pub status: AgentStatus,
}

impl AgentCard {
    pub fn new(
        agent_id: AgentIdentity,
        name: String,
        description: String,
    ) -> Self {
        Self {
            agent_id,
            name,
            description,
            capabilities: Vec::new(),
            supported_protocols: Vec::new(),
            endpoints: Vec::new(),
            skills: Vec::new(),
            status: AgentStatus::Online,
        }
    }

    pub fn with_capability(mut self, name: String, description: String) -> Self {
        self.capabilities.push(Capability { name, description });
        self
    }

    pub fn with_protocol(mut self, protocol: impl Into<String>) -> Self {
        self.supported_protocols.push(protocol.into());
        self
    }

    pub fn with_endpoint(mut self, endpoint: Endpoint) -> Self {
        self.endpoints.push(endpoint);
        self
    }

    pub fn with_skill(mut self, skill: String) -> Self {
        self.skills.push(skill);
        self
    }

    pub fn with_status(mut self, status: AgentStatus) -> Self {
        self.status = status;
        self
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Capability {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentStatus {
    Online,
    Busy,
    Offline,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Busy => "busy",
            Self::Offline => "offline",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "online" => Some(Self::Online),
            "busy" => Some(Self::Busy),
            "offline" => Some(Self::Offline),
            _ => None,
        }
    }
}

pub struct A2ADiscovery {
    session: Arc<Session>,
    timeout: Duration,
}

impl A2ADiscovery {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn with_timeout(session: Arc<Session>, timeout: Duration) -> Self {
        Self { session, timeout }
    }

    pub async fn announce(&self, card: &AgentCard) -> Result<(), crate::error::AgentError> {
        let topic = "agent/discovery/announce";
        let publisher = self.session.declare_publisher(topic).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;
        let data = serde_json::to_vec(card)
            .map_err(|e| crate::error::AgentError::Config(e.to_string()))?;
        publisher.put(data).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;
        Ok(())
    }

    pub async fn discover(&self, capability_filter: Option<&str>) -> Result<Vec<AgentCard>, crate::error::AgentError> {
        let topic = "agent/discovery/announce";
        let subscriber = self.session.declare_subscriber(topic).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;

        let deadline = std::time::Instant::now() + self.timeout;
        let mut cards = Vec::new();

        while std::time::Instant::now() < deadline {
            if let Ok(sample) = subscriber.recv() {
                if let Ok(card) = serde_json::from_slice::<AgentCard>(&sample.payload().to_bytes()) {
                    match capability_filter {
                        Some(filter) => {
                            if card.capabilities.iter().any(|c| c.name.contains(filter)) {
                                cards.push(card);
                            }
                        }
                        None => cards.push(card),
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Ok(cards)
    }

    pub async fn discover_by_protocol(
        &self,
        protocol: &str,
    ) -> Result<Vec<AgentCard>, crate::error::AgentError> {
        let all_cards = self.discover(None).await?;
        Ok(all_cards
            .into_iter()
            .filter(|card| card.supported_protocols.contains(&protocol.to_string()))
            .collect())
    }

    pub async fn discover_by_skill(
        &self,
        skill: &str,
    ) -> Result<Vec<AgentCard>, crate::error::AgentError> {
        let all_cards = self.discover(None).await?;
        Ok(all_cards
            .into_iter()
            .filter(|card| card.skills.contains(&skill.to_string()))
            .collect())
    }

    pub async fn discover_by_status(
        &self,
        status: AgentStatus,
    ) -> Result<Vec<AgentCard>, crate::error::AgentError> {
        let all_cards = self.discover(None).await?;
        Ok(all_cards.into_iter().filter(|card| card.status == status).collect())
    }

    pub async fn get_agent(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentCard>, crate::error::AgentError> {
        let all_cards = self.discover(None).await?;
        Ok(all_cards.into_iter().find(|card| card.agent_id.id == agent_id))
    }

    pub async fn publish_health(
        &self,
        agent_id: &str,
        status: AgentStatus,
    ) -> Result<(), crate::error::AgentError> {
        let topic = format!("agent/discovery/health/{}", agent_id);
        let publisher = self.session.declare_publisher(&topic).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;

        let health = serde_json::json!({
            "agent_id": agent_id,
            "status": status,
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        });

        let data = serde_json::to_vec(&health)
            .map_err(|e| crate::error::AgentError::Config(e.to_string()))?;
        publisher.put(data).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;
        Ok(())
    }

    pub async fn subscribe_health(
        &self,
    ) -> Result<(), crate::error::AgentError> {
        Ok(())
    }
}