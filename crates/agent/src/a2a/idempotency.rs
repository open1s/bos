use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::TaskState;

#[derive(Debug, Clone)]
pub struct ProcessedResult {
    pub task_id: String,
    pub state: TaskState,
    pub result: Option<serde_json::Value>,
    pub timestamp: u64,
}

pub struct IdempotencyStore {
    processed: RwLock<HashMap<String, CacheEntry>>,
    default_ttl: Duration,
}

struct CacheEntry {
    result: ProcessedResult,
    expires_at: Instant,
}

impl IdempotencyStore {
    pub fn new() -> Self {
        Self {
            processed: RwLock::new(HashMap::new()),
            default_ttl: Duration::from_secs(300),
        }
    }

    pub async fn check(&self, key: &str) -> Option<ProcessedResult> {
        let store = self.processed.read().await;
        store.get(key).and_then(|entry| {
            if entry.expires_at > Instant::now() {
                Some(entry.result.clone())
            } else {
                None
            }
        })
    }

    pub async fn record(&self, key: String, result: ProcessedResult) {
        let mut store = self.processed.write().await;
        store.insert(key, CacheEntry {
            result,
            expires_at: Instant::now() + self.default_ttl,
        });
    }

    pub async fn cleanup(&self) {
        let mut store = self.processed.write().await;
        let now = Instant::now();
        store.retain(|_, entry| entry.expires_at > now);
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}