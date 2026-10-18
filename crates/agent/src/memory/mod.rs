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
use std::collections::HashSet;
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

/// Restrict recall to items whose metadata holds `key: value`.
///
/// Comparison is exact JSON equality, so a number filter does not match
/// a string of the same digits. Items without metadata never match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataFilter {
    /// Metadata key to look up.
    pub key: String,
    /// Value the key must equal.
    pub value: Value,
}

impl MetadataFilter {
    /// Build a filter that matches `key == value`.
    #[must_use]
    pub fn new(key: impl Into<String>, value: Value) -> Self {
        Self {
            key: key.into(),
            value,
        }
    }

    /// Whether `item` carries this key with the expected value.
    fn matches(&self, item: &MemoryItem) -> bool {
        item.metadata
            .as_ref()
            .and_then(|metadata| metadata.get(&self.key))
            == Some(&self.value)
    }
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

    /// Like [`MemoryStore::search`], but only considers items whose
    /// metadata matches `filter`. `None` behaves like
    /// [`MemoryStore::search`].
    ///
    /// The default implementation filters [`MemoryStore::all`] and ranks
    /// the survivors, so backends override it only to filter more cheaply.
    async fn search_filtered(
        &self,
        query: &str,
        limit: usize,
        filter: Option<&MetadataFilter>,
    ) -> Vec<MemoryItem> {
        let all = self.all().await;
        let items: Vec<&MemoryItem> = all
            .iter()
            .filter(|item| filter.is_none_or(|filter| filter.matches(item)))
            .collect();
        rank(&items, query, limit)
    }

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

    /// Insert an existing [`MemoryItem`], preserving its id and
    /// timestamp. Used when replicating items from another store.
    ///
    /// Returns `false` when an item with the same id is already stored.
    pub fn insert(&self, item: MemoryItem) -> bool {
        let mut items = self.items.write().unwrap_or_else(|e| e.into_inner());
        if items.iter().any(|existing| existing.id == item.id) {
            return false;
        }
        items.push(item);
        true
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
        self.search_filtered(query, limit, None).await
    }

    async fn search_filtered(
        &self,
        query: &str,
        limit: usize,
        filter: Option<&MetadataFilter>,
    ) -> Vec<MemoryItem> {
        let items = self.items.read().unwrap_or_else(|e| e.into_inner());
        // Rank over borrowed items and clone only the winners: a recall runs
        // on every turn, and copying the whole store first was pure waste.
        let filtered: Vec<&MemoryItem> = items
            .iter()
            .filter(|item| filter.is_none_or(|filter| filter.matches(item)))
            .collect();
        rank(&filtered, query, limit)
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

/// Rank `items` for `query`: one point per query token present in the
/// content, ties toward the more recently created item, and a blank query
/// yields the most recent `limit` items.
fn rank(items: &[&MemoryItem], query: &str, limit: usize) -> Vec<MemoryItem> {
    if limit == 0 {
        return Vec::new();
    }
    // One point per *distinct* query token, as documented: a repeated word in
    // the query must not outvote a second item that matches a different token.
    let mut query_tokens = tokenize(query);
    query_tokens.sort_unstable();
    query_tokens.dedup();
    if query_tokens.is_empty() {
        return items
            .iter()
            .rev()
            .take(limit)
            .map(|item| (*item).clone())
            .collect();
    }
    let mut scored: Vec<(usize, &MemoryItem)> = items
        .iter()
        .filter_map(|item| {
            let content_tokens = tokenize(&item.content);
            let content: HashSet<&str> = content_tokens.iter().map(String::as_str).collect();
            let score = query_tokens
                .iter()
                .filter(|token| content.contains(token.as_str()))
                .count();
            (score > 0).then_some((score, *item))
        })
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

    /// The documented score is one point per *distinct* query token.
    ///
    /// `rust rust go` must score `rust` and `go` equally (one point each), so
    /// the newer item wins the tie; counting the repeated token twice would
    /// rank the older `rust` first.
    #[tokio::test]
    async fn repeated_query_tokens_count_once() {
        let older = MemoryItem {
            id: "older".into(),
            content: "rust".into(),
            metadata: None,
            created_at_ms: 1,
        };
        let newer = MemoryItem {
            id: "newer".into(),
            content: "go".into(),
            metadata: None,
            created_at_ms: 2,
        };
        let memory = InMemoryMemory::from_items(vec![older, newer]);
        let hits = memory.search("rust rust go", 2).await;
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "newer", "distinct tokens tie; recency decides");
    }

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
