//! Pins the shared memory-ranking fixture to the canonical Rust store.

use agent::memory::{InMemoryMemory, MemoryStore};
use serde_json::Value;

#[tokio::test]
async fn ranking_matches_the_shared_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/memory_ranking.json")).expect("valid fixture");
    let memory = InMemoryMemory::new();
    for item in fixture["items"].as_array().unwrap() {
        memory.add(item.as_str().unwrap().to_string(), None).await;
    }
    let query = fixture["query"].as_str().unwrap();
    let limit = fixture["limit"].as_u64().unwrap() as usize;
    let hits = memory.search(query, limit).await;
    let got: Vec<&str> = hits.iter().map(|hit| hit.content.as_str()).collect();
    let expected: Vec<&str> = fixture["expected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(got, expected);
}
