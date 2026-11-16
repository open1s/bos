//! The memory records contract: extensions on the Durable Record Kernel.
//!
//! Host-neutral mirror of the kernel's schema-2 wire shape (see
//! `docs/ferris-memory-records.md`). Ferris never depends on the agent crate,
//! so the record DTOs live here and the host implements [`MemoryRecords`]
//! over its own kernel; a host-side conformance test pins the JSON round-trip
//! so the two shapes cannot drift.
//!
//! The contract is reachable two ways: typed in-process acquisition through
//! the service registry (name [`MEMORY_SERVICE`], contract
//! [`MEMORY_CONTRACT`]), and the JSON transport ([`dispatch_memory_json`])
//! for out-of-process runtimes. Both enforce the same rule: a mutation is
//! acknowledged only after its durable commit, and scope is checked before
//! ranking or injection.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::ExtensionError;

/// The service name the host provides the kernel under.
pub const MEMORY_SERVICE: &str = "memory";
/// The contract id; service identity is the contract, not a local name.
pub const MEMORY_CONTRACT: &str = "bos.memory-records";
/// The contract version this module defines.
pub const MEMORY_CONTRACT_VERSION: &str = "1.0.0";

/// Stable error code: the caller's scope does not permit the operation.
pub const MEMORY_ERR_SCOPE: &str = "memory.scope_denied";
/// Stable error code: the durable commit failed; the detail names the stage.
pub const MEMORY_ERR_STORAGE: &str = "memory.storage";
/// Stable error code: the addressed record does not exist.
pub const MEMORY_ERR_NOT_FOUND: &str = "memory.not_found";
/// Stable error code: a concurrent commit or duplicate id refused the write.
pub const MEMORY_ERR_CONFLICT: &str = "memory.conflict";
/// Stable error code: the store exists but cannot be read as records.
pub const MEMORY_ERR_CORRUPT: &str = "memory.corrupt";
/// Stable error code: the request or record is malformed.
pub const MEMORY_ERR_INVALID: &str = "memory.invalid";
/// Stable error code: memory is configured but not usable right now.
pub const MEMORY_ERR_UNAVAILABLE: &str = "memory.unavailable";

/// Build a memory contract error with a stable code.
pub fn memory_error(code: &str, message: impl Into<String>) -> ExtensionError {
    ExtensionError::Runtime {
        extension: MEMORY_SERVICE.to_string(),
        message: format!("{code}: {}", message.into()),
    }
}

/// What a memory IS. Mirrors the kernel's `MemoryKind` wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKindDto {
    /// An unclassified note.
    Note,
    /// A raw user/assistant exchange.
    Exchange,
    /// A stated fact.
    Fact,
    /// A preference.
    Preference,
    /// A decision that was made.
    Decision,
    /// A lesson learned.
    Lesson,
    /// A reusable procedure.
    Procedure,
    /// Task state at a point in time.
    TaskState,
}

/// The access scope a record belongs to. Mirrors the kernel's tagged format.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum MemoryScopeDto {
    /// Shared within one user identity.
    User {
        /// The user identity.
        user: String,
    },
    /// Isolated by a stable workspace identity.
    Workspace {
        /// The workspace identity.
        workspace: String,
    },
    /// One session inside a workspace.
    Session {
        /// The workspace identity.
        workspace: String,
        /// The session identity.
        session: String,
    },
    /// One agent inside a session.
    Agent {
        /// The workspace identity.
        workspace: String,
        /// The session identity.
        session: String,
        /// The agent identity.
        agent: String,
    },
}

/// Where a record's content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSourceDto {
    /// An explicit user statement.
    UserStatement,
    /// A model inference.
    ModelInference,
    /// Evidence produced by a tool run (including extensions).
    ToolEvidence,
    /// Migrated legacy data.
    Legacy,
    /// Host/system-authored.
    System,
}

/// Provenance back to the source turn and evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceDto {
    /// The kind of source.
    pub source: EvidenceSourceDto,
    /// The source session, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The source turn, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// Evidence references (never payloads).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// Heuristic evidence quality, NOT a calibrated probability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevelDto {
    /// No quality claim.
    Unverified,
    /// Weak evidence.
    Low,
    /// Direct, unreviewed evidence.
    Medium,
    /// Strong evidence.
    High,
}

/// Confidence semantics for one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfidenceDto {
    /// The heuristic level.
    pub level: ConfidenceLevelDto,
    /// Whether the content was verified.
    pub verified: bool,
}

/// The lifecycle status of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleDto {
    /// Captured, not yet committed by governance.
    Candidate,
    /// Live and recallable.
    Active,
    /// Replaced by a newer revision.
    Superseded,
    /// Past its validity window.
    Expired,
    /// Deleted; kept as a merge marker.
    Tombstoned,
}

/// Explicit relations between records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationsDto {
    /// Records this one replaces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<String>,
    /// Records this one contradicts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradicts: Vec<String>,
    /// Records this one was derived from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
}

/// One versioned, scoped, evidence-backed memory (schema 2).
///
/// The serde format is byte-compatible with the kernel's `MemoryRecord`;
/// hosts convert through JSON, and a conformance test pins the equivalence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecordDto {
    /// Schema version (2).
    pub schema: u32,
    /// Stable identifier.
    pub id: String,
    /// Revision counter.
    pub revision: u32,
    /// What kind of knowledge this is.
    pub kind: MemoryKindDto,
    /// The access scope.
    pub scope: MemoryScopeDto,
    /// The remembered text.
    pub content: String,
    /// Arbitrary caller metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Where the content came from.
    pub provenance: ProvenanceDto,
    /// Heuristic confidence.
    pub confidence: ConfidenceDto,
    /// Lifecycle status.
    pub status: LifecycleDto,
    /// Creation time (Unix ms).
    pub created_at_ms: u64,
    /// Last update time (Unix ms).
    pub updated_at_ms: u64,
    /// Optional expiry (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    /// Explicit relations.
    #[serde(default, skip_serializing_if = "RelationsDto::is_empty")]
    pub relations: RelationsDto,
}

impl RelationsDto {
    /// Whether no relation is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.supersedes.is_empty() && self.contradicts.is_empty() && self.derived_from.is_empty()
    }
}

/// One hybrid recall request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallRequestDto {
    /// The query text (multilingual).
    pub query: String,
    /// Maximum hits.
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
    /// Optional token budget over hit contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<usize>,
    /// Optional kind allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<MemoryKindDto>>,
    /// The clock for expiry/recency; 0 means the host's wall clock.
    #[serde(default)]
    pub now_ms: u64,
}

fn default_recall_limit() -> usize {
    5
}

/// One named score contribution, for explainable recall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreFeatureDto {
    /// Feature name (bm25, kind, verified, recency).
    pub name: String,
    /// Its numeric contribution.
    pub value: f64,
}

/// One recalled record with its explanation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallHitDto {
    /// The record.
    pub record: MemoryRecordDto,
    /// The final ordering score.
    pub score: f64,
    /// The score's named contributions.
    pub features: Vec<ScoreFeatureDto>,
    /// Estimated content tokens.
    pub token_estimate: usize,
}

/// The outcome of a recall.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecallOutcomeDto {
    /// Ranked hits, best first.
    pub hits: Vec<RecallHitDto>,
    /// Why nothing was returned, when nothing was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
    /// Candidates dropped by the token budget.
    #[serde(default)]
    pub truncated: usize,
}

/// The typed memory service the host provides over its kernel.
///
/// Implementations must be commit-guaranteed: a mutation returns `Ok` only
/// after the durable commit succeeded, and scope is enforced on reads AND
/// writes before ranking or injection. Errors carry the stable `memory.*`
/// codes so callers can tell refused from failed from missing.
#[async_trait::async_trait]
pub trait MemoryRecords: Send + Sync {
    /// Every stored record (including history), oldest first, viewer-scoped.
    async fn records(&self) -> Result<Vec<MemoryRecordDto>, ExtensionError>;

    /// Hybrid recall: scope/kind filtering before ranking, token-budgeted.
    async fn recall(&self, request: RecallRequestDto) -> Result<RecallOutcomeDto, ExtensionError>;

    /// Commit a new record; the host stamps id, revision and attribution.
    async fn add(&self, record: MemoryRecordDto) -> Result<MemoryRecordDto, ExtensionError>;

    /// Update content/metadata in place, bumping the revision.
    async fn update(
        &self,
        id: &str,
        content: String,
        metadata: Option<Value>,
    ) -> Result<MemoryRecordDto, ExtensionError>;

    /// Replace a record with a successor, keeping the previous revision.
    async fn supersede(
        &self,
        id: &str,
        successor: MemoryRecordDto,
    ) -> Result<MemoryRecordDto, ExtensionError>;

    /// Tombstone a record, returning whether it existed.
    async fn remove(&self, id: &str) -> Result<bool, ExtensionError>;

    /// Remove every record, committing an empty store.
    async fn clear(&self) -> Result<(), ExtensionError>;
}

/// A sized handle for acquiring the memory service through the registry.
///
/// The service registry stores sized types, so the host provides the service
/// wrapped in this handle and consumers acquire it back typed. Deref gives
/// direct access to the contract: `lease.records()`, `lease.recall(..)`, ...
#[derive(Clone)]
pub struct MemoryServiceHandle(pub std::sync::Arc<dyn MemoryRecords>);

impl std::ops::Deref for MemoryServiceHandle {
    type Target = dyn MemoryRecords;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

/// The JSON transport seam for out-of-process runtimes.
///
/// The Node bridge's shim-to-host service frame (`service: "memory"`) and any
/// other JSON transport dispatch through this one function, so every runtime
/// speaks the same contract. Methods: records, recall, add, update, supersede,
/// remove, clear.
pub async fn dispatch_memory_json(
    service: &dyn MemoryRecords,
    method: &str,
    params: Value,
) -> Result<Value, ExtensionError> {
    let invalid = |detail: String| memory_error(MEMORY_ERR_INVALID, detail);
    match method {
        "records" => {
            let records = service.records().await?;
            serde_json::to_value(records).map_err(|e| invalid(e.to_string()))
        }
        "recall" => {
            let request: RecallRequestDto =
                serde_json::from_value(params).map_err(|e| invalid(e.to_string()))?;
            let outcome = service.recall(request).await?;
            serde_json::to_value(outcome).map_err(|e| invalid(e.to_string()))
        }
        "add" => {
            let record: MemoryRecordDto =
                serde_json::from_value(params).map_err(|e| invalid(e.to_string()))?;
            let committed = service.add(record).await?;
            serde_json::to_value(committed).map_err(|e| invalid(e.to_string()))
        }
        "update" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("update needs an id".to_string()))?
                .to_string();
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("update needs content".to_string()))?
                .to_string();
            let metadata = params.get("metadata").cloned();
            let updated = service.update(&id, content, metadata).await?;
            serde_json::to_value(updated).map_err(|e| invalid(e.to_string()))
        }
        "supersede" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("supersede needs an id".to_string()))?
                .to_string();
            let successor: MemoryRecordDto = serde_json::from_value(
                params
                    .get("successor")
                    .cloned()
                    .ok_or_else(|| invalid("supersede needs a successor".to_string()))?,
            )
            .map_err(|e| invalid(e.to_string()))?;
            let committed = service.supersede(&id, successor).await?;
            serde_json::to_value(committed).map_err(|e| invalid(e.to_string()))
        }
        "remove" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("remove needs an id".to_string()))?;
            let removed = service.remove(id).await?;
            Ok(serde_json::json!({ "removed": removed }))
        }
        "clear" => {
            service.clear().await?;
            Ok(serde_json::json!({ "cleared": true }))
        }
        other => Err(memory_error(
            MEMORY_ERR_INVALID,
            format!("unknown memory method {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A minimal in-memory implementation for contract tests.
    #[derive(Default)]
    struct FakeKernel {
        records: Mutex<Vec<MemoryRecordDto>>,
        next_id: Mutex<u64>,
    }

    fn record(content: &str) -> MemoryRecordDto {
        MemoryRecordDto {
            schema: 2,
            id: String::new(),
            revision: 1,
            kind: MemoryKindDto::Fact,
            scope: MemoryScopeDto::Workspace {
                workspace: "w".into(),
            },
            content: content.into(),
            metadata: None,
            provenance: ProvenanceDto {
                source: EvidenceSourceDto::ToolEvidence,
                session: None,
                turn: None,
                evidence: Vec::new(),
            },
            confidence: ConfidenceDto {
                level: ConfidenceLevelDto::Unverified,
                verified: false,
            },
            status: LifecycleDto::Active,
            created_at_ms: 10,
            updated_at_ms: 10,
            expires_at_ms: None,
            relations: RelationsDto::default(),
        }
    }

    #[async_trait::async_trait]
    impl MemoryRecords for FakeKernel {
        async fn records(&self) -> Result<Vec<MemoryRecordDto>, ExtensionError> {
            Ok(self.records.lock().unwrap().clone())
        }

        async fn recall(
            &self,
            request: RecallRequestDto,
        ) -> Result<RecallOutcomeDto, ExtensionError> {
            let records = self.records.lock().unwrap();
            let hits: Vec<RecallHitDto> = records
                .iter()
                .filter(|r| r.content.contains(&request.query))
                .take(request.limit)
                .map(|r| RecallHitDto {
                    record: r.clone(),
                    score: 1.0,
                    features: vec![ScoreFeatureDto {
                        name: "bm25".into(),
                        value: 1.0,
                    }],
                    token_estimate: r.content.len() / 4,
                })
                .collect();
            Ok(RecallOutcomeDto {
                hits,
                skip: None,
                truncated: 0,
            })
        }

        async fn add(
            &self,
            mut record: MemoryRecordDto,
        ) -> Result<MemoryRecordDto, ExtensionError> {
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            record.id = format!("rec-{next}");
            self.records.lock().unwrap().push(record.clone());
            Ok(record)
        }

        async fn update(
            &self,
            id: &str,
            content: String,
            metadata: Option<Value>,
        ) -> Result<MemoryRecordDto, ExtensionError> {
            let mut records = self.records.lock().unwrap();
            let record = records
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| memory_error(MEMORY_ERR_NOT_FOUND, id))?;
            record.content = content;
            record.metadata = metadata;
            record.revision += 1;
            Ok(record.clone())
        }

        async fn supersede(
            &self,
            id: &str,
            mut successor: MemoryRecordDto,
        ) -> Result<MemoryRecordDto, ExtensionError> {
            let mut records = self.records.lock().unwrap();
            let previous = records
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| memory_error(MEMORY_ERR_NOT_FOUND, id))?;
            previous.status = LifecycleDto::Superseded;
            successor.relations.supersedes.push(id.to_string());
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            successor.id = format!("rec-{next}");
            records.push(successor.clone());
            Ok(successor)
        }

        async fn remove(&self, id: &str) -> Result<bool, ExtensionError> {
            let mut records = self.records.lock().unwrap();
            let Some(record) = records.iter_mut().find(|r| r.id == id) else {
                return Ok(false);
            };
            record.status = LifecycleDto::Tombstoned;
            Ok(true)
        }

        async fn clear(&self) -> Result<(), ExtensionError> {
            self.records.lock().unwrap().clear();
            Ok(())
        }
    }

    #[test]
    fn the_dto_wire_format_matches_the_kernel_shape() {
        let json = serde_json::json!({
            "schema": 2,
            "id": "r1",
            "revision": 3,
            "kind": "decision",
            "scope": {"level": "session", "workspace": "w", "session": "s"},
            "content": "use the kernel",
            "provenance": {"source": "user_statement", "session": "s", "turn": "t1", "evidence": ["turn:s/t1"]},
            "confidence": {"level": "medium", "verified": true},
            "status": "active",
            "created_at_ms": 1,
            "updated_at_ms": 2,
            "expires_at_ms": 99,
            "relations": {"supersedes": ["r0"]}
        });
        let dto: MemoryRecordDto = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(dto.kind, MemoryKindDto::Decision);
        assert_eq!(dto.confidence.level, ConfidenceLevelDto::Medium);
        let back = serde_json::to_value(&dto).unwrap();
        assert_eq!(back, json, "the DTO round-trips the kernel wire format");
    }

    #[test]
    fn optional_fields_are_omitted_like_the_kernel() {
        let dto = record("plain");
        let value = serde_json::to_value(&dto).unwrap();
        assert!(value.get("metadata").is_none());
        assert!(value.get("expires_at_ms").is_none());
        assert!(value.get("relations").is_none());
        assert!(value["provenance"].get("session").is_none());
    }

    #[tokio::test]
    async fn json_dispatch_covers_every_method() {
        let kernel = FakeKernel::default();
        let added = dispatch_memory_json(
            &kernel,
            "add",
            serde_json::to_value(record("the deploy gate runs clippy")).unwrap(),
        )
        .await
        .unwrap();
        let id = added["id"].as_str().unwrap().to_string();

        let listed = dispatch_memory_json(&kernel, "records", Value::Null)
            .await
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);

        let outcome = dispatch_memory_json(
            &kernel,
            "recall",
            serde_json::json!({"query": "clippy", "limit": 3}),
        )
        .await
        .unwrap();
        assert_eq!(outcome["hits"].as_array().unwrap().len(), 1);

        let updated = dispatch_memory_json(
            &kernel,
            "update",
            serde_json::json!({"id": id, "content": "updated", "metadata": {"tag": "x"}}),
        )
        .await
        .unwrap();
        assert_eq!(updated["revision"], 2);
        assert_eq!(updated["content"], "updated");

        let successor = dispatch_memory_json(
            &kernel,
            "supersede",
            serde_json::json!({"id": id, "successor": record("successor")}),
        )
        .await
        .unwrap();
        assert_eq!(successor["relations"]["supersedes"][0], id);

        let removed = dispatch_memory_json(&kernel, "remove", serde_json::json!({"id": id}))
            .await
            .unwrap();
        assert_eq!(removed["removed"], true);

        let cleared = dispatch_memory_json(&kernel, "clear", Value::Null)
            .await
            .unwrap();
        assert_eq!(cleared["cleared"], true);
    }

    #[tokio::test]
    async fn dispatch_errors_carry_stable_codes() {
        let kernel = FakeKernel::default();
        let error = dispatch_memory_json(&kernel, "remove", serde_json::json!({}))
            .await
            .expect_err("remove needs an id");
        assert!(error.to_string().contains(MEMORY_ERR_INVALID));
        let error = dispatch_memory_json(&kernel, "nope", Value::Null)
            .await
            .expect_err("unknown method");
        assert!(error.to_string().contains(MEMORY_ERR_INVALID));
        let error = dispatch_memory_json(
            &kernel,
            "update",
            serde_json::json!({"id": "missing", "content": "x"}),
        )
        .await
        .expect_err("missing record");
        assert!(error.to_string().contains(MEMORY_ERR_NOT_FOUND));
    }

    #[test]
    fn recall_request_defaults_are_documented() {
        let request: RecallRequestDto =
            serde_json::from_value(serde_json::json!({"query": "q"})).unwrap();
        assert_eq!(request.limit, 5);
        assert_eq!(request.now_ms, 0);
        assert!(request.token_budget.is_none());
    }
}
