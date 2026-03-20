//! Zenoh topic paths for A2A protocol
//!
//! Per 02-CONTEXT.md specification:
//! - `agent/{agent_id}/tasks/incoming` — receive task delegations
//! - `agent/{agent_id}/tasks/{task_id}/status` — publish status updates
//! - `agent/{agent_id}/responses/{correlation_id}` — reply to specific request
//! - `agent/discovery/announce` — AgentCard announcements
//! - `agent/discovery/health/{agent_id}` — Health status updates

/// Topic pattern for receiving task delegations
pub fn tasks_incoming(agent_id: &str) -> String {
    format!("agent/{}/tasks/incoming", agent_id)
}

/// Topic pattern for publishing task status
pub fn task_status(agent_id: &str, task_id: &str) -> String {
    format!("agent/{}/tasks/{}/status", agent_id, task_id)
}

/// Topic pattern for responses to requests
pub fn response(agent_id: &str, correlation_id: &str) -> String {
    format!("agent/{}/responses/{}", agent_id, correlation_id)
}

/// Discovery announcement topic
pub const DISCOVERY_ANNOUNCE: &str = "agent/discovery/announce";

/// Health status topic pattern
pub fn health(agent_id: &str) -> String {
    format!("agent/discovery/health/{}", agent_id)
}
