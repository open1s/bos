//! Tests for memory recall through the Agent.

use agent::memory::{InMemoryMemory, MemoryStore};
use agent::prelude::{Agent, AgentConfig};
use std::sync::Arc;

#[tokio::test]
async fn agent_recalls_attached_memories_into_context() {
    let memory = Arc::new(InMemoryMemory::new());
    memory
        .add("The staging deploy needs VPN".to_string(), None)
        .await;
    memory
        .add("Production runs on Kubernetes".to_string(), None)
        .await;
    let agent = Agent::from_config(AgentConfig::default()).with_memory(memory);
    let context = agent
        .recalled_context("how do I deploy to staging?")
        .await
        .expect("a matching memory");
    assert!(context.contains("VPN"));
    assert!(
        !context.contains("Kubernetes"),
        "unrelated memory leaked in"
    );
}

#[tokio::test]
async fn agent_without_memory_recalls_nothing() {
    let agent = Agent::from_config(AgentConfig::default());
    assert!(agent.recalled_context("anything at all").await.is_none());
}

#[tokio::test]
async fn recall_limit_is_configurable() {
    let memory = Arc::new(InMemoryMemory::new());
    for i in 0..5 {
        memory.add(format!("rust note {i}"), None).await;
    }
    let mut agent = Agent::from_config(AgentConfig::default()).with_memory(memory);
    agent.set_memory_recall_limit(2);
    let context = agent.recalled_context("rust").await.unwrap();
    assert!(context.starts_with("Relevant memory:"));
    assert_eq!(context.matches("\n- ").count(), 2);
}

#[tokio::test]
async fn blank_query_recalls_nothing() {
    let memory = Arc::new(InMemoryMemory::new());
    memory.add("rust note".to_string(), None).await;
    let agent = Agent::from_config(AgentConfig::default()).with_memory(memory);
    assert!(agent.recalled_context("   ").await.is_none());
}
