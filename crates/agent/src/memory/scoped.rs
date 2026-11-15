//! Scope-composed memory over several coordinated stores (issue #340).
//!
//! BSH resolves a session's memory into a workspace store plus the user store;
//! [`ScopedMemory`] presents them as ONE [`MemoryStore`]: writes land in the
//! primary store at the writer's scope, reads merge every store's viewer-visible
//! records and rank them together under one budget. Each constituent store
//! enforces its own scope on both paths, so composition cannot widen access.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::error::MemoryError;
use super::file::FileMemory;
use super::retrieval::{self, RecallOutcome, RecallRequest};
use super::{MemoryItem, MemoryStore, MetadataFilter};

/// Several scoped stores presented as one memory surface.
pub struct ScopedMemory {
    stores: Vec<Arc<FileMemory>>,
    primary: usize,
}

impl std::fmt::Debug for ScopedMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedMemory")
            .field("stores", &self.stores.len())
            .field("primary", &self.primary)
            .finish_non_exhaustive()
    }
}

impl ScopedMemory {
    /// Compose `stores`, writing to the first one by default.
    ///
    /// Returns [`MemoryError::Unavailable`] for an empty list: a composition
    /// with nowhere to write is a configuration error, not a silent no-op.
    pub fn new(stores: Vec<Arc<FileMemory>>) -> Result<Self, MemoryError> {
        if stores.is_empty() {
            return Err(MemoryError::Unavailable(
                "a scoped memory needs at least one store".to_string(),
            ));
        }
        Ok(Self { stores, primary: 0 })
    }

    /// Choose the write target by index (clamped to the store list).
    #[must_use]
    pub fn with_primary(mut self, index: usize) -> Self {
        self.primary = index.min(self.stores.len() - 1);
        self
    }

    /// The constituent stores, in composition order.
    #[must_use]
    pub fn stores(&self) -> &[Arc<FileMemory>] {
        &self.stores
    }

    fn merged_records(&self) -> Vec<super::MemoryRecord> {
        let mut records = Vec::new();
        for store in &self.stores {
            records.extend(store.scoped_records());
        }
        records
    }
}

#[async_trait]
impl MemoryStore for ScopedMemory {
    async fn add(&self, content: String, metadata: Option<Value>) -> MemoryItem {
        self.stores[self.primary].add(content, metadata).await
    }

    async fn try_add(
        &self,
        content: String,
        metadata: Option<Value>,
    ) -> Result<MemoryItem, MemoryError> {
        self.stores[self.primary].try_add(content, metadata).await
    }

    async fn all(&self) -> Vec<MemoryItem> {
        let mut items: Vec<MemoryItem> = Vec::new();
        for store in &self.stores {
            items.extend(store.all().await);
        }
        items.sort_by_key(|a| a.created_at_ms);
        items
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
        let items = self.all().await;
        let filtered: Vec<&MemoryItem> = items
            .iter()
            .filter(|item| filter.is_none_or(|filter| filter.matches(item)))
            .collect();
        super::rank(&filtered, query, limit)
    }

    async fn recall(&self, query: &str, request: &RecallRequest) -> RecallOutcome {
        // Each store already filtered by its own viewer; rank the union once
        // so BM25 statistics cover the whole visible corpus.
        let records = self.merged_records();
        let mut request = request.clone();
        request.viewer = None;
        retrieval::recall(&records, query, &request)
    }

    async fn remove(&self, id: &str) -> bool {
        match self.try_remove(id).await {
            Ok(removed) => removed,
            Err(error) => {
                log::error!("memory: scoped remove was not committed: {error}");
                false
            }
        }
    }

    async fn try_remove(&self, id: &str) -> Result<bool, MemoryError> {
        for store in &self.stores {
            if store.try_remove(id).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn clear(&self) {
        if let Err(error) = self.try_clear().await {
            log::error!("memory: scoped clear was not committed: {error}");
        }
    }

    async fn try_clear(&self) -> Result<(), MemoryError> {
        for store in &self.stores {
            store.try_clear().await?;
        }
        Ok(())
    }

    async fn len(&self) -> usize {
        let mut total = 0;
        for store in &self.stores {
            total += store.len().await;
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::record::{MemoryKind, MemoryRecord, MemoryScope, Provenance};
    use crate::memory::repository::MemoryRepository;

    #[tokio::test]
    async fn workspaces_cannot_recall_each_others_private_records() {
        let dir = tempfile::tempdir().unwrap();
        let ws_a = Arc::new(
            FileMemory::open_scoped(
                dir.path().join("a.jsonl"),
                MemoryScope::workspace("a"),
                Some(MemoryScope::workspace("a")),
            )
            .await
            .unwrap(),
        );
        let ws_b = Arc::new(
            FileMemory::open_scoped(
                dir.path().join("b.jsonl"),
                MemoryScope::workspace("b"),
                Some(MemoryScope::workspace("b")),
            )
            .await
            .unwrap(),
        );
        ws_a.try_add("部署流程 alpha".to_string(), None)
            .await
            .unwrap();
        ws_b.try_add("部署流程 beta".to_string(), None)
            .await
            .unwrap();
        let view_a = ScopedMemory::new(vec![Arc::clone(&ws_a)]).unwrap();
        let hits = view_a.recall("部署流程", &RecallRequest::default()).await;
        assert_eq!(hits.hits.len(), 1);
        assert!(hits.hits[0].record.content.contains("alpha"));
    }

    #[tokio::test]
    async fn user_and_workspace_stores_compose_with_one_budget() {
        let dir = tempfile::tempdir().unwrap();
        let user = Arc::new(
            FileMemory::open_scoped(
                dir.path().join("user.jsonl"),
                MemoryScope::user("u"),
                Some(MemoryScope::user("u")),
            )
            .await
            .unwrap(),
        );
        let workspace = Arc::new(
            FileMemory::open_scoped(
                dir.path().join("ws.jsonl"),
                MemoryScope::workspace("w"),
                Some(MemoryScope::session("w", "s")),
            )
            .await
            .unwrap(),
        );
        user.try_add("the user likes rust".to_string(), None)
            .await
            .unwrap();
        workspace
            .try_add("rust workspace conventions".to_string(), None)
            .await
            .unwrap();
        let composed = ScopedMemory::new(vec![Arc::clone(&workspace), Arc::clone(&user)])
            .unwrap()
            .with_primary(0);
        let outcome = composed.recall("rust", &RecallRequest::default()).await;
        assert_eq!(outcome.hits.len(), 2, "both scopes recall");
        // Writes land in the primary (workspace) store only.
        composed
            .try_add("new note".to_string(), None)
            .await
            .unwrap();
        assert_eq!(workspace.all().await.len(), 2);
        assert_eq!(user.all().await.len(), 1);
        // Removal finds the record in whichever store holds it.
        let user_item = user.all().await[0].id.clone();
        assert!(composed.try_remove(&user_item).await.unwrap());
        assert_eq!(user.all().await.len(), 0);
    }

    #[tokio::test]
    async fn session_records_carry_their_scope_through_composition() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            FileMemory::open_scoped(
                dir.path().join("ws.jsonl"),
                MemoryScope::workspace("w"),
                Some(MemoryScope::session("w", "s1")),
            )
            .await
            .unwrap(),
        );
        let repo = store.repository();
        let record = MemoryRecord::new(
            MemoryKind::Decision,
            MemoryScope::session("w", "s1"),
            "session decision",
            Provenance::default(),
        );
        repo.add_record(record).await.unwrap();
        let composed = ScopedMemory::new(vec![Arc::clone(&store)]).unwrap();
        let outcome = composed
            .recall("session decision", &RecallRequest::default())
            .await;
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(
            outcome.hits[0].record.scope,
            MemoryScope::session("w", "s1")
        );
    }

    #[tokio::test]
    async fn an_empty_composition_is_refused() {
        assert!(ScopedMemory::new(Vec::new()).is_err());
    }
}
