//! Integration tests for the agent memory store.

use agent::memory::{InMemoryMemory, MemoryItem, MemoryStore};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn add_assigns_identity_and_returns_the_item() {
    let memory = InMemoryMemory::new();
    let item = memory.add("the sky is blue".to_string(), None).await;
    assert!(!item.id.is_empty(), "id must be assigned");
    assert_eq!(item.content, "the sky is blue");
    assert!(item.metadata.is_none());
    assert_eq!(memory.len().await, 1);
    assert!(!memory.is_empty().await);
}

#[tokio::test]
async fn metadata_round_trips() {
    let memory = InMemoryMemory::new();
    let item = memory
        .add(
            "deploy notes".to_string(),
            Some(json!({ "source": "runbook" })),
        )
        .await;
    let stored = memory.all().await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], item);
    assert_eq!(stored[0].metadata.as_ref().unwrap()["source"], "runbook");
}

#[tokio::test]
async fn search_ranks_by_keyword_overlap() {
    let memory = InMemoryMemory::new();
    memory
        .add("rust ownership and borrowing".to_string(), None)
        .await;
    memory
        .add("python asyncio event loop".to_string(), None)
        .await;
    memory.add("rust async runtimes".to_string(), None).await;

    let hits = memory.search("rust async", 2).await;
    assert_eq!(hits.len(), 2);
    // Matching both tokens ranks above matching only "rust".
    assert_eq!(hits[0].content, "rust async runtimes");
    assert_eq!(hits[1].content, "rust ownership and borrowing");
}

#[tokio::test]
async fn search_ignores_non_matches_and_respects_limit() {
    let memory = InMemoryMemory::new();
    memory.add("alpha".to_string(), None).await;
    memory.add("beta".to_string(), None).await;
    assert!(memory.search("gamma", 5).await.is_empty());
    assert!(memory.search("alpha", 0).await.is_empty());
}

#[tokio::test]
async fn empty_query_returns_most_recent_first() {
    let memory = InMemoryMemory::new();
    memory.add("first".to_string(), None).await;
    memory.add("second".to_string(), None).await;
    let recent = memory.search("   ", 1).await;
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].content, "second");
}

#[tokio::test]
async fn remove_and_clear() {
    let memory = InMemoryMemory::new();
    let a = memory.add("a".to_string(), None).await;
    memory.add("b".to_string(), None).await;
    assert!(memory.remove(&a.id).await);
    assert!(!memory.remove(&a.id).await, "second removal is a no-op");
    assert_eq!(memory.len().await, 1);
    memory.clear().await;
    assert!(memory.is_empty().await);
}

#[tokio::test]
async fn store_is_object_safe() {
    let memory: Arc<dyn MemoryStore> = Arc::new(InMemoryMemory::new());
    memory.add("x".to_string(), None).await;
    assert_eq!(memory.len().await, 1);
}

#[test]
fn item_can_be_built_and_enriched_directly() {
    let item = MemoryItem::new("hello", None).with_metadata(json!({ "k": 1 }));
    assert_eq!(item.content, "hello");
    assert_eq!(item.metadata.unwrap()["k"], 1);
}
