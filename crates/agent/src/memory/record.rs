//! Versioned, evidence-backed memory records (issue #344).
//!
//! [`MemoryRecord`] is the storage unit of the durable record kernel: every
//! record carries its kind, scope, provenance, confidence semantics, validity,
//! lifecycle status, revision and relations, so governance, recall and context
//! assembly can reason about WHAT a memory is instead of treating it as bare
//! text. Legacy [`MemoryItem`] JSONL lines migrate on read through
//! [`parse_record`]; nothing is destroyed and the legacy surface keeps working.
//!
//! Confidence is a documented HEURISTIC, never an objective probability: the
//! levels order evidence quality for ranking and review, and no code may treat
//! them as calibrated. Untrusted record content is data: no field of a record
//! is ever read as host policy or capability authority.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::MemoryItem;

/// The schema version written by this code.
///
/// Version 1 is the legacy bare item (no schema field); version 2 is the full
/// record. Readers accept both; writers always emit 2. A future version must
/// keep [`parse_record`] able to read every older line.
pub const RECORD_SCHEMA: u32 = 2;

/// What a memory IS, so capture governance and recall can treat a decision
/// differently from a raw exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// An unclassified note (the legacy default).
    Note,
    /// A raw user/assistant exchange retained by auto-remember.
    Exchange,
    /// A stated fact about the world or the project.
    Fact,
    /// A user or project preference.
    Preference,
    /// A decision that was made, with its evidence.
    Decision,
    /// A lesson learned from an outcome.
    Lesson,
    /// A reusable procedure.
    Procedure,
    /// The state of a task at a point in time.
    TaskState,
}

impl MemoryKind {
    /// A stable, human-readable label for diagnostics and fixtures.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            MemoryKind::Note => "note",
            MemoryKind::Exchange => "exchange",
            MemoryKind::Fact => "fact",
            MemoryKind::Preference => "preference",
            MemoryKind::Decision => "decision",
            MemoryKind::Lesson => "lesson",
            MemoryKind::Procedure => "procedure",
            MemoryKind::TaskState => "task_state",
        }
    }
}

/// The access scope a record belongs to (issue #340).
///
/// Scopes form a chain: User is the widest identity, then Workspace, then
/// Session, then Agent. A viewer sees records at its own level or wider in the
/// SAME identity chain, and writes only at its own level or narrower. Storage
/// identity (which file) is resolved separately; this type is the access rule
/// enforced on both the recall and the write path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum MemoryScope {
    /// Shared only within one user identity.
    User {
        /// The user identity.
        user: String,
    },
    /// Isolated by a stable workspace identity.
    Workspace {
        /// The workspace identity (registry id, not a path).
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

impl MemoryScope {
    /// A user scope.
    #[must_use]
    pub fn user(user: impl Into<String>) -> Self {
        MemoryScope::User { user: user.into() }
    }

    /// A workspace scope.
    #[must_use]
    pub fn workspace(workspace: impl Into<String>) -> Self {
        MemoryScope::Workspace {
            workspace: workspace.into(),
        }
    }

    /// A session scope inside a workspace.
    #[must_use]
    pub fn session(workspace: impl Into<String>, session: impl Into<String>) -> Self {
        MemoryScope::Session {
            workspace: workspace.into(),
            session: session.into(),
        }
    }

    /// An agent scope inside a session.
    #[must_use]
    pub fn agent(
        workspace: impl Into<String>,
        session: impl Into<String>,
        agent: impl Into<String>,
    ) -> Self {
        MemoryScope::Agent {
            workspace: workspace.into(),
            session: session.into(),
            agent: agent.into(),
        }
    }

    /// Whether a viewer at `self` may READ a record scoped to `record`.
    ///
    /// The record must sit at the viewer's level or wider, in the same identity
    /// chain: a session sees its workspace's records, never another session's.
    #[must_use]
    pub fn permits(&self, record: &MemoryScope) -> bool {
        match (self, record) {
            (MemoryScope::User { user: a }, MemoryScope::User { user: b }) => a == b,
            (MemoryScope::Workspace { workspace: a }, MemoryScope::Workspace { workspace: b }) => {
                a == b
            }
            (
                MemoryScope::Workspace { workspace: a },
                MemoryScope::Session { workspace: b, .. },
            ) => a == b,
            (MemoryScope::Workspace { workspace: a }, MemoryScope::Agent { workspace: b, .. }) => {
                a == b
            }
            (
                MemoryScope::Session {
                    workspace: a,
                    session: s,
                },
                MemoryScope::Session {
                    workspace: b,
                    session: t,
                },
            ) => a == b && s == t,
            (
                MemoryScope::Session {
                    workspace: a,
                    session: s,
                },
                MemoryScope::Agent {
                    workspace: b,
                    session: t,
                    ..
                },
            ) => a == b && s == t,
            (
                MemoryScope::Session { workspace: a, .. },
                MemoryScope::Workspace { workspace: b },
            ) => a == b,
            (
                MemoryScope::Agent {
                    workspace: a,
                    session: s,
                    agent: x,
                },
                MemoryScope::Agent {
                    workspace: b,
                    session: t,
                    agent: y,
                },
            ) => a == b && s == t && x == y,
            (
                MemoryScope::Agent {
                    workspace: a,
                    session: s,
                    ..
                },
                MemoryScope::Session {
                    workspace: b,
                    session: t,
                },
            ) => a == b && s == t,
            (MemoryScope::Agent { workspace: a, .. }, MemoryScope::Workspace { workspace: b }) => {
                a == b
            }
            _ => false,
        }
    }

    /// Whether a writer at `self` may WRITE a record scoped to `record`.
    ///
    /// The record must sit at the writer's level or NARROWER in the same chain:
    /// a session writer cannot publish workspace-wide records, and no writer
    /// can cross into another identity.
    #[must_use]
    pub fn may_write(&self, record: &MemoryScope) -> bool {
        match (self, record) {
            (MemoryScope::User { .. }, MemoryScope::User { .. }) => self.permits(record),
            (MemoryScope::Workspace { .. }, MemoryScope::User { .. }) => false,
            (
                MemoryScope::Session { .. },
                MemoryScope::User { .. } | MemoryScope::Workspace { .. },
            ) => false,
            (
                MemoryScope::Agent { .. },
                MemoryScope::User { .. }
                | MemoryScope::Workspace { .. }
                | MemoryScope::Session { .. },
            ) => false,
            _ => self.permits(record),
        }
    }

    /// The workspace identity this scope belongs to, when it has one.
    #[must_use]
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            MemoryScope::User { .. } => None,
            MemoryScope::Workspace { workspace }
            | MemoryScope::Session { workspace, .. }
            | MemoryScope::Agent { workspace, .. } => Some(workspace),
        }
    }
}

/// Where a record's content came from (issue #343/#344).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    /// An explicit user statement.
    UserStatement,
    /// A model inference, never equivalent to a user statement.
    ModelInference,
    /// Evidence produced by a tool run.
    ToolEvidence,
    /// Migrated from a legacy bare item with no provenance.
    Legacy,
    /// Host/system-authored (capture pipeline metadata, migrations).
    System,
}

/// Provenance back to the source turn and evidence (issue #344).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The kind of source that produced the content.
    pub source: EvidenceSource,
    /// The session the record was captured in, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The turn identifier inside that session, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// Evidence references: turn ids, tool-call ids, file paths. References
    /// only - never the payload itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            source: EvidenceSource::Legacy,
            session: None,
            turn: None,
            evidence: Vec::new(),
        }
    }
}

/// Heuristic evidence-quality levels, NOT calibrated probabilities.
///
/// The ordering is for ranking and review only. No code may present a level as
/// an objective probability of truth (issue #344).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    /// No quality claim at all (legacy default).
    Unverified,
    /// Weak or indirect evidence.
    Low,
    /// Direct but unreviewed evidence.
    Medium,
    /// Strong evidence, still subject to verification.
    High,
}

/// Confidence semantics for one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confidence {
    /// The heuristic level.
    pub level: ConfidenceLevel,
    /// Whether a human or a verified tool run confirmed the content.
    pub verified: bool,
}

impl Default for Confidence {
    fn default() -> Self {
        Self {
            level: ConfidenceLevel::Unverified,
            verified: false,
        }
    }
}

/// The lifecycle status of a record (issue #344).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    /// Captured but not yet committed by governance.
    Candidate,
    /// Live and recallable.
    Active,
    /// Replaced by a newer revision; kept for history.
    Superseded,
    /// Past its validity window; kept until compaction.
    Expired,
    /// Deleted; kept as a merge marker so cross-process commits cannot
    /// resurrect it.
    Tombstoned,
}

/// Explicit relations between records (issue #344).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relations {
    /// Records this one replaces (their ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<String>,
    /// Records this one contradicts (their ids); both stay readable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradicts: Vec<String>,
    /// Records this one was derived from (their ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
}

impl Relations {
    /// Whether no relation is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.supersedes.is_empty() && self.contradicts.is_empty() && self.derived_from.is_empty()
    }
}

/// One versioned, scoped, evidence-backed memory (issue #344).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// The schema version of this line (always [`RECORD_SCHEMA`] on write).
    pub schema: u32,
    /// Stable identifier, unique within a store.
    pub id: String,
    /// Revision counter; every committed update increments it.
    pub revision: u32,
    /// What kind of knowledge this is.
    pub kind: MemoryKind,
    /// The access scope this record belongs to.
    pub scope: MemoryScope,
    /// The remembered text.
    pub content: String,
    /// Arbitrary caller metadata (tags, importance, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Where the content came from.
    pub provenance: Provenance,
    /// Heuristic confidence semantics.
    pub confidence: Confidence,
    /// The lifecycle status.
    pub status: Lifecycle,
    /// Creation time as milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    /// Last committed update time.
    pub updated_at_ms: u64,
    /// Optional expiry; at or after this instant the record is not recallable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    /// Explicit relations to other records.
    #[serde(default, skip_serializing_if = "Relations::is_empty")]
    pub relations: Relations,
}

impl MemoryRecord {
    /// Build an active record with a fresh id, revision 1 and the current time.
    #[must_use]
    pub fn new(
        kind: MemoryKind,
        scope: MemoryScope,
        content: impl Into<String>,
        provenance: Provenance,
    ) -> Self {
        let now = super::now_millis();
        Self {
            schema: RECORD_SCHEMA,
            id: Uuid::new_v4().to_string(),
            revision: 1,
            kind,
            scope,
            content: content.into(),
            metadata: None,
            provenance,
            confidence: Confidence::default(),
            status: Lifecycle::Active,
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: None,
            relations: Relations::default(),
        }
    }

    /// Migrate a legacy bare item, preserving id, content, metadata and time.
    ///
    /// The kind is inferred (a `User: ... / Assistant: ...` note is an
    /// Exchange), the scope is the store's default, and provenance is marked
    /// Legacy so later governance can tell migrated data from captured data.
    #[must_use]
    pub fn from_legacy(item: &MemoryItem, default_scope: &MemoryScope) -> Self {
        let kind = if item.content.starts_with("User: ") && item.content.contains("\nAssistant: ") {
            MemoryKind::Exchange
        } else {
            MemoryKind::Note
        };
        Self {
            schema: RECORD_SCHEMA,
            id: item.id.clone(),
            revision: 1,
            kind,
            scope: default_scope.clone(),
            content: item.content.clone(),
            metadata: item.metadata.clone(),
            provenance: Provenance::default(),
            confidence: Confidence::default(),
            status: Lifecycle::Active,
            created_at_ms: item.created_at_ms,
            updated_at_ms: item.created_at_ms,
            expires_at_ms: None,
            relations: Relations::default(),
        }
    }

    /// The legacy item view, for the compatibility surface.
    #[must_use]
    pub fn to_item(&self) -> MemoryItem {
        MemoryItem {
            id: self.id.clone(),
            content: self.content.clone(),
            metadata: self.metadata.clone(),
            created_at_ms: self.created_at_ms,
        }
    }

    /// Whether the record is recallable at `now_ms`: active and unexpired.
    #[must_use]
    pub fn is_live(&self, now_ms: u64) -> bool {
        self.status == Lifecycle::Active && self.expires_at_ms.is_none_or(|expiry| now_ms < expiry)
    }

    /// Mark the record expired at `now_ms` when its validity has passed.
    /// Returns whether the status changed.
    pub fn expire_if_due(&mut self, now_ms: u64) -> bool {
        if self.status == Lifecycle::Active
            && self.expires_at_ms.is_some_and(|expiry| now_ms >= expiry)
        {
            self.status = Lifecycle::Expired;
            self.updated_at_ms = now_ms;
            return true;
        }
        false
    }

    /// Serialize as one JSONL line.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Parse one JSONL line as a record, migrating legacy items on read.
///
/// A line carrying a `schema` field MUST parse as a full record: a malformed
/// v2 line is an error, never a silent downgrade to a bare item. A line without
/// `schema` is a legacy item and migrates with `default_scope`.
pub fn parse_record(line: &str, default_scope: &MemoryScope) -> Result<MemoryRecord, String> {
    let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if value.get("schema").is_some() {
        serde_json::from_value::<MemoryRecord>(value).map_err(|e| e.to_string())
    } else {
        let item: MemoryItem = serde_json::from_value(value).map_err(|e| e.to_string())?;
        Ok(MemoryRecord::from_legacy(&item, default_scope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str) -> MemoryItem {
        MemoryItem {
            id: "legacy-1".into(),
            content: content.into(),
            metadata: Some(serde_json::json!({"tag": "x"})),
            created_at_ms: 42,
        }
    }

    #[test]
    fn legacy_items_migrate_without_loss() {
        let scope = MemoryScope::user("u");
        let record = MemoryRecord::from_legacy(&item("a note"), &scope);
        assert_eq!(record.id, "legacy-1");
        assert_eq!(record.content, "a note");
        assert_eq!(record.metadata, Some(serde_json::json!({"tag": "x"})));
        assert_eq!(record.created_at_ms, 42);
        assert_eq!(record.kind, MemoryKind::Note);
        assert_eq!(record.scope, scope);
        assert_eq!(record.provenance.source, EvidenceSource::Legacy);
        assert_eq!(record.status, Lifecycle::Active);
        assert_eq!(record.revision, 1);
        let back = record.to_item();
        assert_eq!(back, item("a note"));
    }

    #[test]
    fn exchange_notes_are_recognized_on_migration() {
        let record =
            MemoryRecord::from_legacy(&item("User: hi\nAssistant: hello"), &MemoryScope::user("u"));
        assert_eq!(record.kind, MemoryKind::Exchange);
    }

    #[test]
    fn parse_reads_legacy_and_v2_lines() {
        let scope = MemoryScope::workspace("w");
        let legacy = serde_json::to_string(&item("old")).unwrap();
        let migrated = parse_record(&legacy, &scope).unwrap();
        assert_eq!(migrated.schema, RECORD_SCHEMA);
        assert_eq!(migrated.content, "old");

        let record = MemoryRecord::new(
            MemoryKind::Decision,
            scope.clone(),
            "use the kernel",
            Provenance {
                source: EvidenceSource::UserStatement,
                session: Some("s1".into()),
                turn: Some("t3".into()),
                evidence: vec!["turn:s1/t3".into()],
            },
        );
        let line = record.to_line().unwrap();
        let round = parse_record(&line, &scope).unwrap();
        assert_eq!(round, record);
    }

    #[test]
    fn a_malformed_v2_line_is_an_error_not_a_downgrade() {
        let scope = MemoryScope::user("u");
        // schema present but required fields missing: must NOT migrate as legacy.
        let broken = r#"{"schema":2,"id":"x","content":"c","created_at_ms":1}"#;
        assert!(parse_record(broken, &scope).is_err());
    }

    #[test]
    fn scope_visibility_follows_the_identity_chain() {
        let ws = MemoryScope::workspace("w");
        let session = MemoryScope::session("w", "s");
        let other_session = MemoryScope::session("w", "s2");
        let agent = MemoryScope::agent("w", "s", "a");
        let other_ws = MemoryScope::workspace("w2");
        let user = MemoryScope::user("u");

        assert!(session.permits(&ws), "a session reads workspace records");
        assert!(
            session.permits(&agent),
            "a session reads its agents' records"
        );
        assert!(!session.permits(&other_session), "sessions are isolated");
        assert!(!ws.permits(&other_ws), "workspaces are isolated");
        assert!(!ws.permits(&user), "user records live in the user store");
        assert!(agent.permits(&session));
        assert!(agent.permits(&ws));
        assert!(!agent.permits(&MemoryScope::agent("w", "s", "b")));
    }

    #[test]
    fn writes_cannot_widen_scope() {
        let session = MemoryScope::session("w", "s");
        assert!(session.may_write(&MemoryScope::session("w", "s")));
        assert!(session.may_write(&MemoryScope::agent("w", "s", "a")));
        assert!(!session.may_write(&MemoryScope::workspace("w")));
        assert!(!session.may_write(&MemoryScope::session("w", "other")));
        let ws = MemoryScope::workspace("w");
        assert!(ws.may_write(&MemoryScope::session("w", "s")));
        assert!(!ws.may_write(&MemoryScope::user("u")));
    }

    #[test]
    fn expiry_is_deterministic() {
        let mut record = MemoryRecord::new(
            MemoryKind::Fact,
            MemoryScope::user("u"),
            "temporary",
            Provenance::default(),
        );
        record.expires_at_ms = Some(100);
        assert!(record.is_live(99));
        assert!(!record.is_live(100));
        assert!(!record.expire_if_due(99));
        assert!(record.expire_if_due(100));
        assert_eq!(record.status, Lifecycle::Expired);
        assert!(!record.expire_if_due(200), "expiry is idempotent");
    }

    #[test]
    fn supersession_keeps_history_and_relation() {
        let scope = MemoryScope::workspace("w");
        let mut old = MemoryRecord::new(
            MemoryKind::Decision,
            scope.clone(),
            "use tokio",
            Provenance::default(),
        );
        let mut new = MemoryRecord::new(
            MemoryKind::Decision,
            scope,
            "use the kernel",
            Provenance::default(),
        );
        new.relations.supersedes.push(old.id.clone());
        old.status = Lifecycle::Superseded;
        assert!(!old.is_live(1));
        assert!(new.is_live(1));
        assert_eq!(new.relations.supersedes, vec![old.id.clone()]);
        // The previous revision survives serialization for history.
        let line = old.to_line().unwrap();
        let round = parse_record(&line, &MemoryScope::workspace("w")).unwrap();
        assert_eq!(round.status, Lifecycle::Superseded);
    }
}
