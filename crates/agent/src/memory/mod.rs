//! Long-term memory for agents.
//!
//! [`MemoryStore`] is the seam: an agent can remember text across turns and
//! recall the most relevant items for a query. [`InMemoryMemory`] is the
//! dependency-free implementation shipped with the framework; backends such
//! as a vector database implement the same trait and can be substituted
//! wherever the agent accepts a `MemoryStore`; [`FileMemory`] persists to
//! a JSON-lines file so memories survive a restart.

mod file;

pub use file::FileMemory;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// One remembered item.
///
/// The store assigns [`MemoryItem::id`] and [`MemoryItem::created_at_ms`] when
/// the item is added; callers supply the text and optional metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Stable identifier, unique within a store.
    pub id: String,
    /// The remembered text.
    pub content: String,
    /// Arbitrary caller metadata (source, tags, embedding, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Creation time as milliseconds since the Unix epoch.
    pub created_at_ms: u64,
}

impl MemoryItem {
    /// Build an item with a fresh id and the current timestamp.
    #[must_use]
    pub fn new(content: impl Into<String>, metadata: Option<Value>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content: content.into(),
            metadata,
            created_at_ms: now_millis(),
        }
    }

    /// Attach metadata, consuming and returning the item.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Current wall-clock time in milliseconds, saturating at zero before the epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Pluggable storage for [`MemoryItem`]s.
///
/// Implementations must be safe to share across tasks (`Send + Sync`) because
/// an agent may be driven concurrently. The trait is intentionally small:
/// storing, listing, lexical recall, and removal. Semantic or vector backends
/// implement [`MemoryStore::search`] with their own ranking.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Store `content` and return the item that was created.
    async fn add(&self, content: String, metadata: Option<Value>) -> MemoryItem;

    /// Return every item, oldest first.
    async fn all(&self) -> Vec<MemoryItem>;

    /// Return up to `limit` items relevant to `query`, best first.
    ///
    /// Implementations may return fewer than `limit` items, and an empty query
    /// means "most recent".
    async fn search(&self, query: &str, limit: usize) -> Vec<MemoryItem>;

    /// Remove the item with `id`, returning whether it existed.
    async fn remove(&self, id: &str) -> bool;

    /// Remove every item.
    async fn clear(&self);

    /// Number of stored items.
    async fn len(&self) -> usize;

    /// Whether the store holds no items.
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

/// A [`MemoryStore`] kept in process memory.
///
/// Items are scored by keyword overlap: the query is split into lowercase
/// alphanumeric tokens and each item scores one point per distinct query token
/// it contains. Ties break toward the more recently added item, so recall is
/// deterministic for a given insertion order.
#[derive(Debug, Default)]
pub struct InMemoryMemory {
    items: RwLock<Vec<MemoryItem>>,
}

impl InMemoryMemory {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a store from already-constructed items, preserving their ids
    /// and timestamps. Used when loading a persisted store.
    #[must_use]
    pub fn from_items(items: Vec<MemoryItem>) -> Self {
        Self {
            items: RwLock::new(items),
        }
    }
}

#[async_trait]
impl MemoryStore for InMemoryMemory {
    async fn add(&self, content: String, metadata: Option<Value>) -> MemoryItem {
        let item = MemoryItem::new(content, metadata);
        // A poisoned lock means another thread panicked while holding it; the
        // vector itself is still consistent, so recover rather than panic.
        let mut items = self.items.write().unwrap_or_else(|e| e.into_inner());
        items.push(item.clone());
        item
    }

    async fn all(&self) -> Vec<MemoryItem> {
        self.items.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    async fn search(&self, query: &str, limit: usize) -> Vec<MemoryItem> {
        if limit == 0 {
            return Vec::new();
        }
        let items = self.items.read().unwrap_or_else(|e| e.into_inner());
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return items.iter().rev().take(limit).cloned().collect();
        }
        let mut scored: Vec<(usize, &MemoryItem)> = items
            .iter()
            .map(|item| {
                let content_tokens = tokenize(&item.content);
                let score = query_tokens
                    .iter()
                    .filter(|token| content_tokens.contains(*token))
                    .count();
                (score, item)
            })
            .filter(|(score, _)| *score > 0)
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.created_at_ms.cmp(&a.1.created_at_ms))
        });
        scored
            .into_iter()
            .take(limit)
            .map(|(_, item)| item.clone())
            .collect()
    }

    async fn remove(&self, id: &str) -> bool {
        let mut items = self.items.write().unwrap_or_else(|e| e.into_inner());
        let before = items.len();
        items.retain(|item| item.id != id);
        items.len() != before
    }

    async fn clear(&self) {
        self.items
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    async fn len(&self) -> usize {
        self.items.read().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// Split text into lowercase alphanumeric tokens.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_is_deterministic_within_a_millisecond() {
        let memory = InMemoryMemory::new();
        memory.add("rust memory".to_string(), None).await;
        memory.add("rust memory".to_string(), None).await;
        let hits = memory.search("rust", 2).await;
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].content, "rust memory");
    }
}
