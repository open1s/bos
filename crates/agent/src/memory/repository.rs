//! The fallible repository API (issue #342).
//!
//! [`MemoryRepository`] is the commit-guaranteed surface: every mutation
//! returns a typed [`MemoryError`] and reports success only after the durable
//! commit succeeded. The legacy [`super::MemoryStore`] trait remains as the
//! compatibility adapter for existing consumers (including the Python/Node
//! bindings); new callers - BSH, capture, governance - use this one.

use async_trait::async_trait;
use serde_json::Value;

use super::error::MemoryError;
use super::record::MemoryRecord;
use super::retrieval::{RecallOutcome, RecallRequest};

/// A commit-guaranteed memory repository.
///
/// Implementations must be safe to share across tasks (`Send + Sync`).
/// Every mutation reaches the configured durability boundary before returning
/// `Ok`; on failure the in-memory state stays consistent with the durable
/// state and the error is typed.
#[async_trait]
pub trait MemoryRepository: Send + Sync {
    /// Commit a new record. The caller's scope must permit the record's scope.
    async fn add_record(&self, record: MemoryRecord) -> Result<MemoryRecord, MemoryError>;

    /// Update content/metadata in place, bumping the revision.
    async fn update_record(
        &self,
        id: &str,
        content: String,
        metadata: Option<Value>,
    ) -> Result<MemoryRecord, MemoryError>;

    /// Replace a record with a successor, keeping the previous revision as
    /// `Superseded` with an explicit relation (issue #344).
    async fn supersede(
        &self,
        id: &str,
        successor: MemoryRecord,
    ) -> Result<MemoryRecord, MemoryError>;

    /// Tombstone a record, returning whether it existed. Tombstones survive
    /// commits so cross-process merges cannot resurrect deleted records.
    async fn remove(&self, id: &str) -> Result<bool, MemoryError>;

    /// Remove every record, committing an empty store.
    async fn clear(&self) -> Result<(), MemoryError>;

    /// Every stored record (including history), oldest first.
    async fn records(&self) -> Result<Vec<MemoryRecord>, MemoryError>;

    /// Hybrid recall with scope enforcement before ranking (issue #345).
    async fn recall(
        &self,
        query: &str,
        request: &RecallRequest,
    ) -> Result<RecallOutcome, MemoryError>;

    /// Re-read the durable snapshot, for cross-process read freshness.
    async fn refresh(&self) -> Result<(), MemoryError>;
}
