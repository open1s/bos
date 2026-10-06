//! Tests for the file-backed memory store.

use agent::memory::{FileMemory, MemoryItem, MemoryStore};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn add_persists_and_reopen_preserves_items() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.jsonl");

    let memory = FileMemory::open(&path).await.unwrap();
    assert!(memory.is_empty().await);
    let item = memory
        .add(
            "the staging deploy needs VPN".to_string(),
            Some(json!({ "source": "runbook" })),
        )
        .await;

    let reopened = FileMemory::open(&path).await.unwrap();
    let items = reopened.all().await;
    assert_eq!(items, vec![item]);
    assert_eq!(items[0].metadata, Some(json!({ "source": "runbook" })));
    assert_eq!(reopened.path(), path.as_path());
}

#[tokio::test]
async fn search_ranking_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.jsonl");
    {
        let memory = FileMemory::open(&path).await.unwrap();
        memory.add("rust ownership".to_string(), None).await;
        memory.add("python asyncio".to_string(), None).await;
        memory.add("rust async runtimes".to_string(), None).await;
    }
    let reopened = FileMemory::open(&path).await.unwrap();
    let hits = reopened.search("rust async", 2).await;
    let contents: Vec<&str> = hits.iter().map(|hit| hit.content.as_str()).collect();
    assert_eq!(contents, vec!["rust async runtimes", "rust ownership"]);
}

#[tokio::test]
async fn remove_and_clear_are_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.jsonl");
    let memory = FileMemory::open(&path).await.unwrap();
    let keep = memory.add("keep me".to_string(), None).await;
    let drop = memory.add("drop me".to_string(), None).await;

    assert!(memory.remove(&drop.id).await);
    assert!(!memory.remove(&drop.id).await);
    assert_eq!(
        FileMemory::open(&path).await.unwrap().all().await,
        vec![keep]
    );

    memory.clear().await;
    assert!(FileMemory::open(&path).await.unwrap().is_empty().await);
}

#[tokio::test]
async fn open_rejects_a_corrupt_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.jsonl");
    tokio::fs::write(&path, "not json\n").await.unwrap();

    let error = FileMemory::open(&path).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains(":1:"), "{error}");
}

#[tokio::test]
async fn file_memory_works_behind_a_trait_object() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.jsonl");
    let store: Arc<dyn MemoryStore> = Arc::new(FileMemory::open(&path).await.unwrap());
    let item = store
        .add("shared behind a trait object".to_string(), None)
        .await;
    assert_eq!(store.len().await, 1);
    assert_eq!(store.all().await, vec![item]);
}

#[tokio::test]
async fn blank_lines_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.jsonl");
    let item = MemoryItem::new("headers and blank lines", None);
    let line = serde_json::to_string(&item).unwrap();
    tokio::fs::write(&path, format!("\n{line}\n\n"))
        .await
        .unwrap();

    let memory = FileMemory::open(&path).await.unwrap();
    assert_eq!(memory.all().await, vec![item]);
}
