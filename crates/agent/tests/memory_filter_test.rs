//! Tests for metadata-filtered recall and the FileMemory capacity cap.

use agent::memory::{FileMemory, InMemoryMemory, MemoryStore, MetadataFilter};
use agent::{Agent, AgentConfig};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn search_filtered_keeps_only_matching_metadata() {
    let memory = InMemoryMemory::new();
    memory
        .add(
            "deploy to staging".to_string(),
            Some(json!({ "env": "staging" })),
        )
        .await;
    memory
        .add("deploy to prod".to_string(), Some(json!({ "env": "prod" })))
        .await;
    memory.add("deploy notes".to_string(), None).await;

    let filter = MetadataFilter::new("env", json!("staging"));
    let hits = memory.search_filtered("deploy", 5, Some(&filter)).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].content, "deploy to staging");

    assert_eq!(memory.search_filtered("deploy", 5, None).await.len(), 3);
}

#[tokio::test]
async fn filter_equality_is_type_strict() {
    let memory = InMemoryMemory::new();
    memory
        .add("counted".to_string(), Some(json!({ "n": 1 })))
        .await;
    memory.add("no metadata".to_string(), None).await;

    let number = MetadataFilter::new("n", json!(1));
    assert_eq!(
        memory
            .search_filtered("counted no metadata", 5, Some(&number))
            .await
            .len(),
        1
    );

    // A string "1" must not match the number 1, and missing metadata never matches.
    let string = MetadataFilter::new("n", json!("1"));
    assert!(memory
        .search_filtered("counted no metadata", 5, Some(&string))
        .await
        .is_empty());
}

#[tokio::test]
async fn file_memory_caps_itself_and_persists_the_eviction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.jsonl");
    let memory = FileMemory::open(&path).await.unwrap().with_max_items(2);
    assert_eq!(memory.max_items(), Some(2));

    memory.add("first".to_string(), None).await;
    memory.add("second".to_string(), None).await;
    memory.add("third".to_string(), None).await;

    let contents: Vec<String> = memory.all().await.into_iter().map(|i| i.content).collect();
    assert_eq!(contents, vec!["second", "third"]);

    let reopened = FileMemory::open(&path).await.unwrap();
    let contents: Vec<String> = reopened
        .all()
        .await
        .into_iter()
        .map(|i| i.content)
        .collect();
    assert_eq!(contents, vec!["second", "third"]);
}

#[tokio::test]
async fn agent_recall_honours_the_metadata_filter() {
    let memory = Arc::new(InMemoryMemory::new());
    memory
        .add(
            "staging facts".to_string(),
            Some(json!({ "env": "staging" })),
        )
        .await;
    memory
        .add("prod facts".to_string(), Some(json!({ "env": "prod" })))
        .await;

    let mut agent = Agent::from_config(AgentConfig::default()).with_memory(memory);
    agent.set_memory_filter(Some(MetadataFilter::new("env", json!("prod"))));
    let context = agent.recalled_context("facts").await.unwrap();
    assert!(context.contains("prod facts"), "{context}");
    assert!(!context.contains("staging facts"), "{context}");

    agent.set_memory_filter(None);
    let context = agent.recalled_context("facts").await.unwrap();
    assert!(context.contains("staging facts") && context.contains("prod facts"));
}
