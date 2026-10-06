//! Tests for memory recall through the Agent.

use agent::memory::{InMemoryMemory, MemoryStore, MetadataFilter};
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

#[tokio::test]
async fn remember_recall_forget_round_trip() {
    let memory = Arc::new(InMemoryMemory::new());
    let agent = Agent::from_config(AgentConfig::default()).with_memory(memory.clone());

    let item = agent
        .remember("the deploy runbook lives in the wiki")
        .await
        .expect("memory is attached");
    assert_eq!(memory.len().await, 1);
    assert_eq!(agent.recall("deploy runbook").await.len(), 1);

    assert!(agent.forget(&item.id).await);
    assert!(!agent.forget(&item.id).await);
    assert!(agent.recall("deploy runbook").await.is_empty());
    assert!(memory.is_empty().await);
}

#[tokio::test]
async fn remember_with_metadata_and_recall_filter() {
    let memory = Arc::new(InMemoryMemory::new());
    let mut agent = Agent::from_config(AgentConfig::default()).with_memory(memory);
    agent
        .remember_with(
            "staging deploy needs VPN",
            Some(serde_json::json!({ "env": "staging" })),
        )
        .await;
    agent
        .remember_with(
            "prod deploy needs approval",
            Some(serde_json::json!({ "env": "prod" })),
        )
        .await;

    agent.set_memory_filter(Some(MetadataFilter::new("env", serde_json::json!("prod"))));
    let hits = agent.recall("deploy").await;
    assert_eq!(hits.len(), 1);
    assert!(hits[0].content.contains("prod"));

    assert!(agent.forget_all().await);
    assert!(!agent.forget_all().await);
}

#[tokio::test]
async fn memory_methods_are_no_ops_without_a_store() {
    let agent = Agent::from_config(AgentConfig::default());
    assert!(agent.remember("anything").await.is_none());
    assert!(!agent.forget("id").await);
    assert!(!agent.forget_all().await);
    assert!(agent.recall("anything").await.is_empty());
}

#[tokio::test]
async fn insert_preserves_id_and_rejects_duplicates() {
    let source = InMemoryMemory::new();
    let item = source.add("shared fact".to_string(), None).await;

    let target = InMemoryMemory::new();
    assert!(target.insert(item.clone()));
    assert!(!target.insert(item.clone()));

    let stored = target.all().await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, item.id);
    assert_eq!(stored[0].created_at_ms, item.created_at_ms);
}
