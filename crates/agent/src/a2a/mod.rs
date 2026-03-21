//! A2A (Agent-to-Agent) Protocol Implementation
//!
//! This module provides the core types and functionality for agent-to-agent
//! communication in the BrainOS framework.

pub mod envelope;
pub mod task;
pub mod discovery;
pub mod client;
pub mod idempotency;

#[cfg(test)]
mod tests;

pub mod topics;

pub use envelope::{A2AMessage, A2AContent, AgentIdentity};
pub use task::{Task, TaskState, TaskStatus};
pub use discovery::{AgentCard, AgentStatus, Capability, Endpoint, A2ADiscovery};
pub use client::A2AClient;
pub use idempotency::{IdempotencyStore, ProcessedResult};