//! File-backed memory on the durable record kernel (issues #341, #342, #344).
//!
//! [`FileMemory`] is the compatibility handle; [`FileRepository`] is the
//! commit-guaranteed API behind it. Both route through ONE shared store per
//! canonical storage identity: a process-wide registry keyed by
//! [`hostkit::coord::canonical_identity`] gives every handle to the same file
//! the same cache and the same async write gate, and every commit additionally
//! takes the cross-process [`hostkit::coord::FileLock`], re-reads the durable
//! snapshot, applies the mutation and commits through
//! [`hostkit::durable::commit`] BEFORE publishing to the cache. A failed
//! commit leaves live and durable state identical and returns a typed error.
//!
//! Compatibility: the legacy [`MemoryStore`] methods stay best-effort adapters
//! (they log and degrade exactly as before, so existing consumers keep their
//! signatures); the `try_*` methods and [`FileRepository`] carry the commit
//! guarantee. New production callers use the fallible surface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use hostkit::coord::{FileLock, canonical_identity};
use hostkit::durable::{self, CommitGate, Loaded, NoFaults, Privacy};
use serde_json::Value;
use tokio::sync::Mutex;

use super::error::MemoryError;
use super::record::{
    EvidenceSource, Lifecycle, MemoryKind, MemoryRecord, MemoryScope, Provenance, parse_record,
};
use super::repository::MemoryRepository;
use super::retrieval::{self, RecallOutcome, RecallRequest};
use super::{MemoryItem, MemoryStore, MetadataFilter};

/// The process-wide registry of shared stores, keyed by canonical identity.
static REGISTRY: OnceLock<RwLock<HashMap<PathBuf, std::sync::Weak<SharedStore>>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<PathBuf, std::sync::Weak<SharedStore>>> {
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// One coordinated store per canonical storage identity (issue #341).
///
/// Every handle to the same file shares this state: one cache, one async write
/// gate, one capacity. The cross-process lock is taken per commit, so two
/// processes also serialize their read-modify-write cycles.
pub(crate) struct SharedStore {
    path: PathBuf,
    records: RwLock<Vec<MemoryRecord>>,
    write_gate: Mutex<()>,
    max_items: RwLock<Option<usize>>,
    default_scope: MemoryScope,
    gate: RwLock<Arc<dyn CommitGate>>,
}

impl std::fmt::Debug for SharedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SharedStore {
    /// Load the durable snapshot once, at identity creation.
    fn load(path: PathBuf, default_scope: &MemoryScope) -> Result<Self, MemoryError> {
        let records = read_durable(&path, default_scope)?;
        Ok(Self {
            path,
            records: RwLock::new(records),
            write_gate: Mutex::new(()),
            max_items: RwLock::new(None),
            default_scope: default_scope.clone(),
            gate: RwLock::new(Arc::new(NoFaults)),
        })
    }

    fn snapshot(&self) -> Vec<MemoryRecord> {
        self.records
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The commit pipeline: serialize within the process, lock across
    /// processes, re-read the durable truth, apply, commit, then publish.
    async fn mutate<T>(
        &self,
        apply: impl FnOnce(&mut Vec<MemoryRecord>) -> Result<T, MemoryError>,
    ) -> Result<T, MemoryError> {
        let _serial = self.write_gate.lock().await;
        let _cross_process = FileLock::acquire(&self.path)?;
        // Under the lock the file is the authority: another process may have
        // committed since our last read. Re-reading here is what makes the
        // merge conflict-safe instead of last-writer-wins.
        let mut records = read_durable(&self.path, &self.default_scope)?;
        let outcome = apply(&mut records)?;
        self.enforce_capacity(&mut records);
        let body = serialize_records(&self.path, &records)?;
        let gate = self
            .gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        durable::commit(
            &self.path,
            body.as_bytes(),
            Privacy::OwnerOnly,
            gate.as_ref(),
        )?;
        // Publish only after the durable commit succeeded (issue #342).
        *self
            .records
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = records;
        Ok(outcome)
    }

    fn enforce_capacity(&self, records: &mut Vec<MemoryRecord>) {
        let max_items = *self
            .max_items
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(max_items) = max_items else {
            return;
        };
        if records.len() <= max_items {
            return;
        }
        // Evict the oldest by creation time, whatever their status: the cap
        // bounds the FILE, and tombstones are merge markers, not payload.
        records.sort_by_key(|a| a.created_at_ms);
        let overflow = records.len() - max_items;
        records.drain(..overflow);
    }

    /// Re-read the durable snapshot into the cache (cross-process freshness).
    fn refresh(&self) -> Result<(), MemoryError> {
        let records = read_durable(&self.path, &self.default_scope)?;
        *self
            .records
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = records;
        Ok(())
    }
}

/// Read and parse the durable snapshot, fail-closed (issues #339/#342).
fn read_durable(
    path: &Path,
    default_scope: &MemoryScope,
) -> Result<Vec<MemoryRecord>, MemoryError> {
    match durable::load(path)? {
        Loaded::Missing => Ok(Vec::new()),
        Loaded::Present(bytes) => {
            let text = String::from_utf8(bytes).map_err(|error| MemoryError::Corrupt {
                path: path.display().to_string(),
                detail: format!("not UTF-8: {error}"),
            })?;
            parse_records(path, &text, default_scope)
        }
    }
}

fn parse_records(
    path: &Path,
    text: &str,
    default_scope: &MemoryScope,
) -> Result<Vec<MemoryRecord>, MemoryError> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record = parse_record(line, default_scope).map_err(|detail| MemoryError::Corrupt {
            path: path.display().to_string(),
            detail: format!("{}: {detail}", index + 1),
        })?;
        records.push(record);
    }
    Ok(records)
}

fn serialize_records(path: &Path, records: &[MemoryRecord]) -> Result<String, MemoryError> {
    let mut body = String::new();
    for record in records {
        let line = record
            .to_line()
            .map_err(|error| MemoryError::Serialization(format!("{}: {error}", path.display())))?;
        body.push_str(&line);
        body.push('\n');
    }
    Ok(body)
}

/// Get or create the one shared store for a canonical storage identity.
fn shared_for(path: &Path, default_scope: &MemoryScope) -> Result<Arc<SharedStore>, MemoryError> {
    let identity = canonical_identity(path);
    let mut guard = registry()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(weak) = guard.get(&identity)
        && let Some(shared) = weak.upgrade()
    {
        return Ok(shared);
    }
    let shared = Arc::new(SharedStore::load(identity.clone(), default_scope)?);
    // Prune dead identities while we hold the lock: the registry stays bounded
    // by the number of LIVE stores, not by every path ever opened.
    guard.retain(|_, weak| weak.strong_count() > 0);
    guard.insert(identity, Arc::downgrade(&shared));
    Ok(shared)
}

/// A file-backed memory handle on the shared, coordinated store.
///
/// Opening the same canonical path twice yields two handles over ONE store:
/// no stale snapshots, no lost updates (issue #341). The legacy
/// [`MemoryStore`] surface is a best-effort adapter; [`FileMemory::try_add`],
/// [`FileMemory::try_remove`], [`FileMemory::try_clear`] and
/// [`FileMemory::repository`] carry the durable-commit guarantee.
pub struct FileMemory {
    shared: Arc<SharedStore>,
    /// The path the caller opened, kept for the legacy `path()` contract.
    requested_path: PathBuf,
    write_scope: MemoryScope,
    viewer: Option<MemoryScope>,
}

impl std::fmt::Debug for FileMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileMemory")
            .field("path", &self.shared.path)
            .finish_non_exhaustive()
    }
}

impl FileMemory {
    /// Open (or create) a store backed by `path`, unscoped.
    ///
    /// A missing file yields an empty store. A file whose lines do not parse
    /// returns [`std::io::ErrorKind::InvalidData`], naming the offending line,
    /// rather than silently dropping data. Legacy bare-item lines migrate to
    /// records on read (issue #344).
    pub async fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::open_scoped(path, MemoryScope::user("local"), None)
            .await
            .map_err(|error| match error {
                MemoryError::Corrupt { path, detail } => {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{path}:{detail}"))
                }
                other => std::io::Error::other(other.to_string()),
            })
    }

    /// Open a store with an explicit storage scope and viewer (issue #340).
    ///
    /// `default_scope` is the store's identity: legacy lines migrate into it
    /// and it bounds what the store holds. The WRITE scope is the viewer's own
    /// scope when one is given (a session writes session records, never
    /// workspace-wide ones), else the storage scope. Reads are restricted to
    /// the viewer's permitted chain.
    pub async fn open_scoped(
        path: impl AsRef<Path>,
        default_scope: MemoryScope,
        viewer: Option<MemoryScope>,
    ) -> Result<Self, MemoryError> {
        let shared = shared_for(path.as_ref(), &default_scope)?;
        let write_scope = viewer.clone().unwrap_or_else(|| default_scope.clone());
        Ok(Self {
            shared,
            requested_path: path.as_ref().to_path_buf(),
            write_scope,
            viewer,
        })
    }

    /// The backing file path, as opened (the legacy contract).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.requested_path
    }

    /// The canonical storage identity every handle to this file shares
    /// (issue #341): symlinks and relative prefixes resolve to one key.
    #[must_use]
    pub fn storage_identity(&self) -> &Path {
        &self.shared.path
    }

    /// The scope unscoped writes land in.
    #[must_use]
    pub fn write_scope(&self) -> &MemoryScope {
        &self.write_scope
    }

    /// Cap the store at `max_items`, evicting the oldest on overflow.
    ///
    /// The cap belongs to the shared store: every handle to the same canonical
    /// path observes it (issue #341).
    #[must_use]
    pub fn with_max_items(self, max_items: usize) -> Self {
        *self
            .shared
            .max_items
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(max_items);
        self
    }

    /// The configured item cap, if any.
    #[must_use]
    pub fn max_items(&self) -> Option<usize> {
        *self
            .shared
            .max_items
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Install a commit gate for fault injection (tests and chaos drills).
    ///
    /// The gate is shared by every handle to this storage identity, so an
    /// injected failure is observed by all of them.
    #[must_use]
    pub fn with_commit_gate(self, gate: Arc<dyn CommitGate>) -> Self {
        *self
            .shared
            .gate
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = gate;
        self
    }

    /// The commit-guaranteed repository behind this handle (issue #342).
    #[must_use]
    pub fn repository(&self) -> FileRepository {
        FileRepository {
            shared: Arc::clone(&self.shared),
            write_scope: self.write_scope.clone(),
            viewer: self.viewer.clone(),
        }
    }

    /// Re-read the durable snapshot (cross-process read freshness).
    pub async fn refresh(&self) -> Result<(), MemoryError> {
        let _serial = self.shared.write_gate.lock().await;
        self.shared.refresh()
    }

    /// The cached records, oldest first, including history but not tombstones.
    #[must_use]
    pub fn records(&self) -> Vec<MemoryRecord> {
        self.shared
            .snapshot()
            .into_iter()
            .filter(|record| record.status != Lifecycle::Tombstoned)
            .collect()
    }

    /// Records visible to this handle's viewer (issue #340): history included,
    /// tombstones excluded, scope enforced. The public read seam hosts and
    /// extension adapters use when they need full records, not bare items.
    #[must_use]
    pub fn visible_records(&self) -> Vec<MemoryRecord> {
        self.scoped_records()
    }

    /// Records visible to this handle's viewer, for scoped composition.
    pub(crate) fn scoped_records(&self) -> Vec<MemoryRecord> {
        self.shared
            .snapshot()
            .into_iter()
            .filter(|record| {
                record.status != Lifecycle::Tombstoned
                    && self
                        .viewer
                        .as_ref()
                        .is_none_or(|viewer| viewer.permits(&record.scope))
            })
            .collect()
    }

    fn visible_items(&self) -> Vec<MemoryItem> {
        self.scoped_records()
            .into_iter()
            .map(|record| record.to_item())
            .collect()
    }
}

#[async_trait]
impl MemoryStore for FileMemory {
    /// Legacy best-effort adapter: on a commit failure the error is logged and
    /// the returned item is NOT stored. Use [`FileMemory::try_add`] or the
    /// repository for the commit guarantee (issue #342).
    async fn add(&self, content: String, metadata: Option<Value>) -> MemoryItem {
        let receipt = MemoryItem::new(content.clone(), metadata.clone());
        match self.try_add(content, metadata).await {
            Ok(item) => item,
            Err(error) => {
                log::error!(
                    "memory: add was not committed for {}: {} ({})",
                    self.shared.path.display(),
                    error,
                    error.label()
                );
                receipt
            }
        }
    }

    async fn try_add(
        &self,
        content: String,
        metadata: Option<Value>,
    ) -> Result<MemoryItem, MemoryError> {
        // Raw exchange retention keeps its own kind, separate from knowledge
        // extraction (issue #343): the same inference the migration uses.
        let kind = if content.starts_with("User: ") && content.contains("\nAssistant: ") {
            MemoryKind::Exchange
        } else {
            MemoryKind::Note
        };
        let mut record = MemoryRecord::new(
            kind,
            self.write_scope.clone(),
            content,
            Provenance {
                source: EvidenceSource::System,
                ..Default::default()
            },
        );
        record.metadata = metadata;
        let committed = self.repository().add_record(record).await?;
        Ok(committed.to_item())
    }

    async fn all(&self) -> Vec<MemoryItem> {
        self.visible_items()
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
        // Scope enforcement applies to the legacy search path too (issue #340):
        // the viewer filter runs before ranking, never as a UI afterthought.
        let items = self.visible_items();
        let filtered: Vec<&MemoryItem> = items
            .iter()
            .filter(|item| filter.is_none_or(|filter| filter.matches(item)))
            .collect();
        super::rank(&filtered, query, limit)
    }

    async fn recall(&self, query: &str, request: &RecallRequest) -> RecallOutcome {
        let mut request = request.clone();
        if request.viewer.is_none() {
            request.viewer = self.viewer.clone();
        }
        retrieval::recall(&self.shared.snapshot(), query, &request)
    }

    async fn remove(&self, id: &str) -> bool {
        match self.try_remove(id).await {
            Ok(removed) => removed,
            Err(error) => {
                log::error!(
                    "memory: remove was not committed for {}: {error}",
                    self.shared.path.display()
                );
                false
            }
        }
    }

    async fn try_remove(&self, id: &str) -> Result<bool, MemoryError> {
        self.repository().remove(id).await
    }

    async fn clear(&self) {
        if let Err(error) = self.try_clear().await {
            log::error!(
                "memory: clear was not committed for {}: {error}",
                self.shared.path.display()
            );
        }
    }

    async fn try_clear(&self) -> Result<(), MemoryError> {
        self.repository().clear().await
    }

    async fn len(&self) -> usize {
        self.scoped_records().len()
    }
}

/// The commit-guaranteed repository over one shared file store (issue #342).
#[derive(Debug, Clone)]
pub struct FileRepository {
    shared: Arc<SharedStore>,
    write_scope: MemoryScope,
    viewer: Option<MemoryScope>,
}

impl FileRepository {
    fn check_write(&self, scope: &MemoryScope) -> Result<(), MemoryError> {
        if self.write_scope.may_write(scope) {
            Ok(())
        } else {
            Err(MemoryError::ScopeDenied {
                reason: format!(
                    "writer at {:?} may not write a record at {scope:?}",
                    self.write_scope
                ),
            })
        }
    }
}

#[async_trait]
impl MemoryRepository for FileRepository {
    async fn add_record(&self, record: MemoryRecord) -> Result<MemoryRecord, MemoryError> {
        self.check_write(&record.scope)?;
        if !matches!(record.status, Lifecycle::Active | Lifecycle::Candidate) {
            return Err(MemoryError::Conflict(format!(
                "record {} cannot be added in status {:?}",
                record.id, record.status
            )));
        }
        let added = record.clone();
        self.shared
            .mutate(move |records| {
                if records.iter().any(|existing| {
                    existing.id == record.id && existing.status != Lifecycle::Tombstoned
                }) {
                    return Err(MemoryError::Conflict(format!(
                        "record {} already exists",
                        record.id
                    )));
                }
                records.push(record);
                Ok(())
            })
            .await?;
        Ok(added)
    }

    async fn update_record(
        &self,
        id: &str,
        content: String,
        metadata: Option<Value>,
    ) -> Result<MemoryRecord, MemoryError> {
        let id = id.to_string();
        self.shared
            .mutate(move |records| {
                let Some(record) = records
                    .iter_mut()
                    .find(|record| record.id == id && record.status != Lifecycle::Tombstoned)
                else {
                    return Err(MemoryError::NotFound { id: id.clone() });
                };
                record.content = content;
                record.metadata = metadata;
                record.revision += 1;
                record.updated_at_ms = super::now_millis();
                Ok(record.clone())
            })
            .await
    }

    async fn supersede(
        &self,
        id: &str,
        successor: MemoryRecord,
    ) -> Result<MemoryRecord, MemoryError> {
        self.check_write(&successor.scope)?;
        let id = id.to_string();
        let mut successor = successor;
        successor.relations.supersedes.push(id.clone());
        let committed = successor.clone();
        self.shared
            .mutate(move |records| {
                let Some(previous) = records
                    .iter_mut()
                    .find(|record| record.id == id && record.status != Lifecycle::Tombstoned)
                else {
                    return Err(MemoryError::NotFound { id: id.clone() });
                };
                previous.status = Lifecycle::Superseded;
                previous.updated_at_ms = super::now_millis();
                records.push(successor);
                Ok(())
            })
            .await?;
        Ok(committed)
    }

    async fn remove(&self, id: &str) -> Result<bool, MemoryError> {
        let id = id.to_string();
        self.shared
            .mutate(move |records| {
                let Some(record) = records
                    .iter_mut()
                    .find(|record| record.id == id && record.status != Lifecycle::Tombstoned)
                else {
                    return Ok(false);
                };
                record.status = Lifecycle::Tombstoned;
                record.updated_at_ms = super::now_millis();
                Ok(true)
            })
            .await
    }

    async fn clear(&self) -> Result<(), MemoryError> {
        self.shared
            .mutate(|records| {
                records.clear();
                Ok(())
            })
            .await
    }

    async fn records(&self) -> Result<Vec<MemoryRecord>, MemoryError> {
        Ok(self.shared.snapshot())
    }

    async fn recall(
        &self,
        query: &str,
        request: &RecallRequest,
    ) -> Result<RecallOutcome, MemoryError> {
        let mut request = request.clone();
        if request.viewer.is_none() {
            request.viewer = self.viewer.clone();
        }
        Ok(retrieval::recall(&self.shared.snapshot(), query, &request))
    }

    async fn refresh(&self) -> Result<(), MemoryError> {
        let _serial = self.shared.write_gate.lock().await;
        self.shared.refresh()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::record::{Confidence, ConfidenceLevel};
    use tokio::sync::Barrier;

    fn temp_path(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        (dir, path)
    }

    #[tokio::test]
    async fn legacy_lines_migrate_and_round_trip() {
        let (_dir, path) = temp_path("memory.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"id":"old-1","content":"legacy note","created_at_ms":7}"#,
                "\n"
            ),
        )
        .unwrap();
        let memory = FileMemory::open(&path).await.unwrap();
        let items = memory.all().await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "old-1");
        assert_eq!(items[0].content, "legacy note");
        // The next commit rewrites the file in the v2 schema, losslessly.
        memory.add("new note".to_string(), None).await;
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"schema\":2"));
        assert!(text.contains("legacy note"));
        assert!(text.contains("new note"));
    }

    #[tokio::test]
    async fn a_corrupt_store_fails_closed_and_is_preserved() {
        let (_dir, path) = temp_path("memory.jsonl");
        std::fs::write(&path, "{not json}\n").unwrap();
        let error = FileMemory::open(&path).await.expect_err("corrupt store");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json}\n");
    }

    /// Issue #342: a failed commit propagates a typed error and leaves both
    /// the cache and the file at the pre-mutation state.
    #[tokio::test]
    async fn failed_commits_propagate_and_preserve_state() {
        struct FailWrite;
        impl CommitGate for FailWrite {
            fn pass(&self, stage: durable::IoStage, _path: &Path) -> std::io::Result<()> {
                if stage == durable::IoStage::Write {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected",
                    ))
                } else {
                    Ok(())
                }
            }
        }
        let (_dir, path) = temp_path("memory.jsonl");
        let memory = FileMemory::open(&path).await.unwrap();
        memory.try_add("committed".to_string(), None).await.unwrap();
        let memory = memory.with_commit_gate(Arc::new(FailWrite));
        let error = memory
            .try_add("uncommitted".to_string(), None)
            .await
            .expect_err("the injected write must fail");
        assert!(matches!(error, MemoryError::Storage(_)));
        assert!(error.is_retryable());
        assert_eq!(error.label(), "storage");
        // Cache and file agree: only the committed record exists.
        assert_eq!(memory.all().await.len(), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("committed"));
        assert!(!text.contains("uncommitted"));
        // Removal and clear fail the same way.
        let id = memory.all().await[0].id.clone();
        assert!(memory.try_remove(&id).await.is_err());
        assert!(memory.try_clear().await.is_err());
        assert_eq!(memory.all().await.len(), 1);
    }

    /// Issue #341: two independent handles to one path share one coordinator;
    /// forced interleaving cannot lose a committed record.
    #[tokio::test]
    async fn two_handles_to_one_path_do_not_lose_updates() {
        let (_dir, path) = temp_path("memory.jsonl");
        let a = FileMemory::open(&path).await.unwrap();
        let b = FileMemory::open(&path).await.unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut tasks = Vec::new();
        for (handle, tag) in [(a, "a"), (b, "b")] {
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                for index in 0..25 {
                    barrier.wait().await;
                    handle
                        .try_add(format!("{tag}-{index}"), None)
                        .await
                        .unwrap();
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        // Every committed record survives, in the file and through either handle.
        let fresh = FileMemory::open(&path).await.unwrap();
        let items = fresh.all().await;
        assert_eq!(items.len(), 50, "no committed record may be lost");
        for tag in ["a", "b"] {
            for index in 0..25 {
                assert!(
                    items
                        .iter()
                        .any(|item| item.content == format!("{tag}-{index}")),
                    "missing {tag}-{index}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn canonical_aliases_share_one_store() {
        let (dir, path) = temp_path("memory.jsonl");
        let alias = dir.path().join("alias.jsonl");
        std::fs::write(&path, b"").unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let a = FileMemory::open(&path).await.unwrap();
        let b = FileMemory::open(&alias).await.unwrap();
        a.try_add("via-real".to_string(), None).await.unwrap();
        assert_eq!(b.all().await.len(), 1, "aliases must share the coordinator");
        assert_eq!(
            a.storage_identity(),
            b.storage_identity(),
            "one canonical identity"
        );
    }

    #[tokio::test]
    async fn tombstones_survive_commits_and_block_resurrection() {
        let (_dir, path) = temp_path("memory.jsonl");
        let memory = FileMemory::open(&path).await.unwrap();
        let item = memory.try_add("delete me".to_string(), None).await.unwrap();
        assert!(memory.try_remove(&item.id).await.unwrap());
        assert_eq!(memory.all().await.len(), 0);
        // A second handle (fresh cache from disk) must not resurrect it.
        let other = FileMemory::open(&path).await.unwrap();
        assert_eq!(other.all().await.len(), 0);
        memory
            .try_add("after delete".to_string(), None)
            .await
            .unwrap();
        let other = FileMemory::open(&path).await.unwrap();
        let items = other.all().await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "after delete");
    }

    #[tokio::test]
    async fn capacity_evicts_the_oldest_across_handles() {
        let (_dir, path) = temp_path("memory.jsonl");
        let a = FileMemory::open(&path).await.unwrap().with_max_items(3);
        let b = FileMemory::open(&path).await.unwrap();
        for index in 0..5 {
            b.try_add(format!("item-{index}"), None).await.unwrap();
        }
        let items = a.all().await;
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].content, "item-2");
    }

    #[tokio::test]
    async fn repository_updates_bump_revisions_and_supersede_keeps_history() {
        let (_dir, path) = temp_path("memory.jsonl");
        let memory = FileMemory::open(&path).await.unwrap();
        let repo = memory.repository();
        let record = MemoryRecord::new(
            MemoryKind::Decision,
            MemoryScope::user("local"),
            "use tokio",
            Provenance::default(),
        );
        let added = repo.add_record(record).await.unwrap();
        let updated = repo
            .update_record(&added.id, "use the kernel".to_string(), None)
            .await
            .unwrap();
        assert_eq!(updated.revision, 2);
        let successor = MemoryRecord::new(
            MemoryKind::Decision,
            MemoryScope::user("local"),
            "use the kernel v2",
            Provenance::default(),
        );
        let committed = repo.supersede(&added.id, successor).await.unwrap();
        assert_eq!(committed.relations.supersedes, vec![added.id.clone()]);
        let all = repo.records().await.unwrap();
        let previous = all.iter().find(|r| r.id == added.id).unwrap();
        assert_eq!(previous.status, Lifecycle::Superseded);
        assert_eq!(previous.content, "use the kernel", "history is preserved");
    }

    #[tokio::test]
    async fn scope_enforcement_covers_writes_and_recall() {
        let (_dir, path) = temp_path("memory.jsonl");
        let memory = FileMemory::open_scoped(
            &path,
            MemoryScope::workspace("w"),
            Some(MemoryScope::session("w", "s1")),
        )
        .await
        .unwrap();
        let repo = memory.repository();
        // A session writer cannot publish workspace-wide records.
        let wide = MemoryRecord::new(
            MemoryKind::Fact,
            MemoryScope::workspace("w"),
            "wide",
            Provenance::default(),
        );
        assert!(matches!(
            repo.add_record(wide).await,
            Err(MemoryError::ScopeDenied { .. })
        ));
        // Session-scoped records commit and recall.
        let mut mine = MemoryRecord::new(
            MemoryKind::Fact,
            MemoryScope::session("w", "s1"),
            "部署流程 session one",
            Provenance::default(),
        );
        mine.confidence = Confidence {
            level: ConfidenceLevel::Medium,
            verified: false,
        };
        repo.add_record(mine).await.unwrap();
        // Another session's record is invisible even though it matches.
        let mut foreign = MemoryRecord::new(
            MemoryKind::Fact,
            MemoryScope::session("w", "s2"),
            "部署流程 session two",
            Provenance::default(),
        );
        foreign.id = "foreign".to_string();
        // Write it through a repository whose writer scope permits it.
        let writer = FileMemory::open_scoped(&path, MemoryScope::workspace("w"), None)
            .await
            .unwrap();
        writer.repository().add_record(foreign).await.unwrap();
        let outcome = memory.recall("部署流程", &RecallRequest::default()).await;
        assert_eq!(outcome.hits.len(), 1);
        assert!(outcome.hits[0].record.content.contains("session one"));
        assert_eq!(memory.all().await.len(), 1, "legacy search is scoped too");
    }

    #[tokio::test]
    async fn recall_is_explainable_and_budgeted() {
        let (_dir, path) = temp_path("memory.jsonl");
        let memory = FileMemory::open(&path).await.unwrap();
        memory
            .try_add("the deployment pipeline uses cargo".to_string(), None)
            .await
            .unwrap();
        let outcome = memory
            .recall(
                "deployment cargo",
                &RecallRequest {
                    limit: 5,
                    token_budget: Some(100),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(outcome.hits.len(), 1);
        assert!(outcome.hits[0].features.iter().any(|f| f.name == "bm25"));
        assert!(outcome.hits[0].token_estimate > 0);
    }

    #[tokio::test]
    async fn a_restart_recovers_the_last_acknowledged_commit() {
        let (_dir, path) = temp_path("memory.jsonl");
        {
            let memory = FileMemory::open(&path).await.unwrap();
            memory.try_add("durable".to_string(), None).await.unwrap();
        }
        // All handles dropped: the registry entry dies with them.
        let reopened = FileMemory::open(&path).await.unwrap();
        let items = reopened.all().await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "durable");
    }
}
