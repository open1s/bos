//! The host adapter between the ferris memory contract and the kernel.
//!
//! [`KernelMemoryService`] implements `ferris_sdk::memory::MemoryRecords`
//! over the Durable Record Kernel: the same commit guarantees, the same scope
//! enforcement, the same typed failures the host itself uses. Extensions never
//! get a bypass - they get the kernel, bound to their activation's scope and
//! attributed to their identity (docs/ferris-memory-records.md).

use std::path::Path;
use std::sync::Arc;

use agent::memory::{
    FileMemory, MemoryError, MemoryRecord, MemoryRepository, MemoryScope, MemoryStore,
    RecallRequest,
};
use ferris_sdk::capability::{CallScope, Capability, PolicyEngine};
use ferris_sdk::memory::{
    MEMORY_CONTRACT, MEMORY_ERR_CONFLICT, MEMORY_ERR_CORRUPT, MEMORY_ERR_INVALID,
    MEMORY_ERR_NOT_FOUND, MEMORY_ERR_SCOPE, MEMORY_ERR_STORAGE, MEMORY_ERR_UNAVAILABLE,
    MEMORY_SERVICE, MemoryRecordDto, MemoryRecords, RecallHitDto, RecallOutcomeDto,
    RecallRequestDto, ScoreFeatureDto, memory_error,
};
use ferris_sdk::types::{ExtensionError, TrustLevel};

use crate::memory_scope::MemoryBinding;

/// Map a kernel failure to the contract's stable error codes.
fn map_error(error: MemoryError) -> ExtensionError {
    let code = match &error {
        MemoryError::Storage(_) => MEMORY_ERR_STORAGE,
        MemoryError::Serialization(_) => MEMORY_ERR_INVALID,
        MemoryError::Corrupt { .. } => MEMORY_ERR_CORRUPT,
        MemoryError::NotFound { .. } => MEMORY_ERR_NOT_FOUND,
        MemoryError::ScopeDenied { .. } => MEMORY_ERR_SCOPE,
        MemoryError::Conflict(_) => MEMORY_ERR_CONFLICT,
        MemoryError::Unavailable(_) => MEMORY_ERR_UNAVAILABLE,
    };
    memory_error(code, error.to_string())
}

/// Kernel record -> contract DTO through the shared wire format.
fn to_dto(record: MemoryRecord) -> Result<MemoryRecordDto, ExtensionError> {
    let value = serde_json::to_value(&record)
        .map_err(|e| map_error(MemoryError::Serialization(e.to_string())))?;
    serde_json::from_value(value).map_err(|e| memory_error(MEMORY_ERR_INVALID, e.to_string()))
}

/// Contract DTO -> kernel record through the shared wire format.
fn from_dto(dto: MemoryRecordDto) -> Result<MemoryRecord, ExtensionError> {
    let value =
        serde_json::to_value(&dto).map_err(|e| memory_error(MEMORY_ERR_INVALID, e.to_string()))?;
    serde_json::from_value(value).map_err(|e| memory_error(MEMORY_ERR_INVALID, e.to_string()))
}

/// The kernel-backed memory service bound to one activation.
///
/// Reads are viewer-scoped before ranking; writes land at the viewer's scope
/// and require the `memory.write` grant, re-checked here at the service
/// boundary (defense in depth beyond acquisition). Every committed record
/// gains an `extension:<id>` evidence reference, so extension-written
/// knowledge is always attributable.
pub(crate) struct KernelMemoryService {
    store: Arc<FileMemory>,
    actor: String,
    write_allowed: bool,
}

impl std::fmt::Debug for KernelMemoryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KernelMemoryService")
            .field("actor", &self.actor)
            .field("write_allowed", &self.write_allowed)
            .finish_non_exhaustive()
    }
}

impl KernelMemoryService {
    /// Bind a store handle to an extension identity and write grant.
    pub(crate) fn new(
        store: Arc<FileMemory>,
        actor: impl Into<String>,
        write_allowed: bool,
    ) -> Self {
        Self {
            store,
            actor: actor.into(),
            write_allowed,
        }
    }

    fn check_write(&self) -> Result<(), ExtensionError> {
        if self.write_allowed {
            Ok(())
        } else {
            Err(memory_error(
                MEMORY_ERR_SCOPE,
                format!("{} does not hold the memory.write grant", self.actor),
            ))
        }
    }

    /// Stamp attribution, scope and commit-readiness onto an incoming record.
    ///
    /// The scope is HOST-BOUND: whatever the caller declared, the record lands
    /// at this activation's viewer scope, so an extension can never publish
    /// wider than the session it runs in. The caller's declared scope is
    /// advisory; the binding is the guarantee.
    fn prepare(&self, dto: MemoryRecordDto) -> Result<MemoryRecord, ExtensionError> {
        let mut record = from_dto(dto)?;
        if record.id.trim().is_empty() {
            record.id = uuid::Uuid::new_v4().to_string();
        }
        record.scope = self.store.write_scope().clone();
        record.status = agent::memory::Lifecycle::Active;
        let reference = format!("extension:{}", self.actor);
        if !record.provenance.evidence.contains(&reference) {
            record.provenance.evidence.push(reference);
        }
        Ok(record)
    }
}

#[async_trait::async_trait]
impl MemoryRecords for KernelMemoryService {
    async fn records(&self) -> Result<Vec<MemoryRecordDto>, ExtensionError> {
        self.store
            .visible_records()
            .into_iter()
            .map(to_dto)
            .collect()
    }

    async fn recall(&self, request: RecallRequestDto) -> Result<RecallOutcomeDto, ExtensionError> {
        let kinds = request.kinds.map(|kinds| {
            kinds
                .into_iter()
                .filter_map(|kind| {
                    serde_json::to_value(kind).ok().and_then(|value| {
                        serde_json::from_value::<agent::memory::MemoryKind>(value).ok()
                    })
                })
                .collect::<Vec<_>>()
        });
        let outcome = self
            .store
            .recall(
                &request.query,
                &RecallRequest {
                    limit: request.limit,
                    token_budget: request.token_budget,
                    kinds,
                    now_ms: request.now_ms,
                    ..Default::default()
                },
            )
            .await;
        let hits = outcome
            .hits
            .into_iter()
            .map(|hit| {
                Ok(RecallHitDto {
                    record: to_dto(hit.record)?,
                    score: hit.score,
                    features: hit
                        .features
                        .into_iter()
                        .map(|feature| ScoreFeatureDto {
                            name: feature.name.to_string(),
                            value: feature.value,
                        })
                        .collect(),
                    token_estimate: hit.token_estimate,
                })
            })
            .collect::<Result<Vec<_>, ExtensionError>>()?;
        Ok(RecallOutcomeDto {
            hits,
            skip: outcome.skip,
            truncated: outcome.truncated,
        })
    }

    async fn add(&self, record: MemoryRecordDto) -> Result<MemoryRecordDto, ExtensionError> {
        self.check_write()?;
        let record = self.prepare(record)?;
        let committed = self
            .store
            .repository()
            .add_record(record)
            .await
            .map_err(map_error)?;
        to_dto(committed)
    }

    async fn update(
        &self,
        id: &str,
        content: String,
        metadata: Option<serde_json::Value>,
    ) -> Result<MemoryRecordDto, ExtensionError> {
        self.check_write()?;
        let updated = self
            .store
            .repository()
            .update_record(id, content, metadata)
            .await
            .map_err(map_error)?;
        to_dto(updated)
    }

    async fn supersede(
        &self,
        id: &str,
        successor: MemoryRecordDto,
    ) -> Result<MemoryRecordDto, ExtensionError> {
        self.check_write()?;
        let successor = self.prepare(successor)?;
        let committed = self
            .store
            .repository()
            .supersede(id, successor)
            .await
            .map_err(map_error)?;
        to_dto(committed)
    }

    async fn remove(&self, id: &str) -> Result<bool, ExtensionError> {
        self.check_write()?;
        self.store.repository().remove(id).await.map_err(map_error)
    }

    async fn clear(&self) -> Result<(), ExtensionError> {
        self.check_write()?;
        self.store.repository().clear().await.map_err(map_error)
    }
}

/// Which memory capabilities one consumer holds right now.
pub(crate) fn grants(
    policy: &PolicyEngine,
    trust: TrustLevel,
    consumer: &str,
    workspace: Option<&Path>,
) -> (bool, bool) {
    let call_scope = CallScope {
        workspace: workspace.map(Path::to_path_buf),
        path: workspace.map(Path::to_path_buf),
        ..CallScope::default()
    };
    let read = policy
        .authorize_call(consumer, trust, Capability::MemoryRead, &call_scope)
        .is_ok();
    let write = policy
        .authorize_call(consumer, trust, Capability::MemoryWrite, &call_scope)
        .is_ok();
    (read, write)
}

/// Open the bound store handle and build the service for one activation.
///
/// The handle joins the kernel's canonical-identity registry, so the service
/// shares the very store the agent and the memory pane use - never a stale
/// copy. Failure is logged and reported as absent: an extension that cannot
/// reach memory simply does not get the service.
pub(crate) fn build_service(
    binding: &MemoryBinding,
    session_id: &str,
    actor: &str,
    write_allowed: bool,
) -> Option<Arc<ferris_sdk::memory::MemoryServiceHandle>> {
    let viewer = if session_id.trim().is_empty() {
        binding.storage_scope.clone()
    } else {
        MemoryScope::session(binding.workspace_id.clone(), session_id)
    };
    let path = binding.workspace_store.clone();
    let storage_scope = binding.storage_scope.clone();
    let opened = super::block_on_tokio_scoped(async move {
        FileMemory::open_scoped(&path, storage_scope, Some(viewer)).await
    });
    match opened {
        Ok(store) => Some(Arc::new(ferris_sdk::memory::MemoryServiceHandle(Arc::new(
            KernelMemoryService::new(Arc::new(store), actor, write_allowed),
        )))),
        Err(error) => {
            log::warn!("extension {actor}: the memory service was not provided: {error}");
            None
        }
    }
}

/// The service identity the host provides, for declaration checks.
pub(crate) fn service_identity() -> (&'static str, &'static str, semver::Version) {
    (
        MEMORY_SERVICE,
        MEMORY_CONTRACT,
        semver::Version::new(1, 0, 0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::memory::{MemoryKind, Provenance};
    use ferris_sdk::memory::{
        ConfidenceDto, ConfidenceLevelDto, EvidenceSourceDto, LifecycleDto, MemoryKindDto,
        MemoryScopeDto, ProvenanceDto, RelationsDto,
    };

    fn dto(content: &str) -> MemoryRecordDto {
        MemoryRecordDto {
            schema: 2,
            id: String::new(),
            revision: 1,
            kind: MemoryKindDto::Fact,
            scope: MemoryScopeDto::Session {
                workspace: "w".into(),
                session: "s1".into(),
            },
            content: content.into(),
            metadata: None,
            provenance: ProvenanceDto {
                source: EvidenceSourceDto::ToolEvidence,
                session: Some("s1".into()),
                turn: Some("t1".into()),
                evidence: Vec::new(),
            },
            confidence: ConfidenceDto {
                level: ConfidenceLevelDto::Medium,
                verified: false,
            },
            status: LifecycleDto::Active,
            created_at_ms: 0,
            updated_at_ms: 0,
            expires_at_ms: None,
            relations: RelationsDto::default(),
        }
    }

    async fn service(dir: &Path, write_allowed: bool) -> KernelMemoryService {
        let store = FileMemory::open_scoped(
            dir.join("ws.jsonl"),
            MemoryScope::workspace("w"),
            Some(MemoryScope::session("w", "s1")),
        )
        .await
        .unwrap();
        KernelMemoryService::new(Arc::new(store), "com.example.test", write_allowed)
    }

    /// The DTO and the kernel record share one wire format: a JSON round-trip
    /// in both directions is the conformance guard against drift.
    #[test]
    fn dto_and_kernel_record_round_trip_through_json() {
        let record = MemoryRecord::new(
            MemoryKind::Decision,
            MemoryScope::session("w", "s1"),
            "use the kernel",
            Provenance {
                source: agent::memory::EvidenceSource::UserStatement,
                session: Some("s1".into()),
                turn: Some("t2".into()),
                evidence: vec!["turn:s1/t2".into()],
            },
        );
        let kernel_json = serde_json::to_value(&record).unwrap();
        let dto: MemoryRecordDto = serde_json::from_value(kernel_json.clone()).unwrap();
        let back = to_dto(from_dto(dto).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), kernel_json);
    }

    #[tokio::test]
    async fn the_service_commits_recalls_and_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path(), true).await;
        let committed = service
            .add(dto("the deploy gate runs clippy"))
            .await
            .unwrap();
        assert!(!committed.id.is_empty());
        assert!(
            committed
                .provenance
                .evidence
                .contains(&"extension:com.example.test".to_string())
        );
        // Hybrid recall over the same kernel the agent uses.
        let outcome = service
            .recall(RecallRequestDto {
                query: "clippy".into(),
                limit: 5,
                token_budget: None,
                kinds: None,
                now_ms: 0,
            })
            .await
            .unwrap();
        assert_eq!(outcome.hits.len(), 1);
        assert!(outcome.hits[0].features.iter().any(|f| f.name == "bm25"));
    }

    #[tokio::test]
    async fn update_supersede_remove_and_clear_follow_the_kernel() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path(), true).await;
        let committed = service.add(dto("first")).await.unwrap();
        let updated = service
            .update(&committed.id, "second".into(), None)
            .await
            .unwrap();
        assert_eq!(updated.revision, 2);
        let successor = service
            .supersede(&committed.id, dto("third"))
            .await
            .unwrap();
        assert_eq!(successor.relations.supersedes, vec![committed.id.clone()]);
        let records = service.records().await.unwrap();
        let previous = records.iter().find(|r| r.id == committed.id).unwrap();
        assert_eq!(previous.status, LifecycleDto::Superseded);
        assert!(service.remove(&committed.id).await.unwrap());
        assert!(!service.remove(&committed.id).await.unwrap());
        service.clear().await.unwrap();
        assert!(service.records().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn without_the_write_grant_every_mutation_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path(), false).await;
        let error = service.add(dto("nope")).await.expect_err("read-only");
        assert!(error.to_string().contains(MEMORY_ERR_SCOPE));
        assert!(service.update("x", "y".into(), None).await.is_err());
        assert!(service.supersede("x", dto("y")).await.is_err());
        assert!(service.remove("x").await.is_err());
        assert!(service.clear().await.is_err());
        // Reads still work.
        assert!(service.records().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn writes_are_bound_to_the_viewer_scope() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path(), true).await;
        // A caller declaring another session's scope cannot publish there:
        // the host binds the record to the activation's viewer scope.
        let mut foreign = dto("declared for another session");
        foreign.scope = MemoryScopeDto::Session {
            workspace: "w".into(),
            session: "s2".into(),
        };
        let committed = service.add(foreign).await.unwrap();
        assert_eq!(
            committed.scope,
            MemoryScopeDto::Session {
                workspace: "w".into(),
                session: "s1".into()
            }
        );
        // A workspace-wide declaration is narrowed the same way.
        let mut wide = dto("declared workspace wide");
        wide.scope = MemoryScopeDto::Workspace {
            workspace: "w".into(),
        };
        let committed = service.add(wide).await.unwrap();
        assert_eq!(
            committed.scope,
            MemoryScopeDto::Session {
                workspace: "w".into(),
                session: "s1".into()
            }
        );
        // Reads stay viewer-scoped: another session's records are invisible.
        let other = FileMemory::open_scoped(
            dir.path().join("ws.jsonl"),
            MemoryScope::workspace("w"),
            Some(MemoryScope::session("w", "s2")),
        )
        .await
        .unwrap();
        let mut hidden = agent::memory::MemoryRecord::new(
            agent::memory::MemoryKind::Fact,
            MemoryScope::session("w", "s2"),
            "s2 private",
            agent::memory::Provenance::default(),
        );
        hidden.id = "hidden".into();
        other.repository().add_record(hidden).await.unwrap();
        let records = service.records().await.unwrap();
        assert!(records.iter().all(|r| r.id != "hidden"));
    }

    #[tokio::test]
    async fn kernel_failures_map_to_stable_codes() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path(), true).await;
        let error = service
            .update("missing", "x".into(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(MEMORY_ERR_NOT_FOUND));
        let committed = service.add(dto("dup")).await.unwrap();
        let mut duplicate = dto("dup again");
        duplicate.id = committed.id.clone();
        let error = service.add(duplicate).await.unwrap_err();
        assert!(error.to_string().contains(MEMORY_ERR_CONFLICT));
    }
}
