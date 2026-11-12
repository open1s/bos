//! The review domain: changesets, snapshots, reviews and decisions.
//!
//! The types here are framework-free. The tool layer records exact snapshots
//! through the runtime, the store persists them, and the GPUI shell only reads
//! a prepared view model.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The manifest schema this build writes.
pub(crate) const SCHEMA: u32 = 1;
/// The largest text file a snapshot may carry.
pub(crate) const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
/// The largest binary file a snapshot may carry.
pub(crate) const MAX_BINARY_BYTES: u64 = 64 * 1024 * 1024;

/// Wall-clock milliseconds since the epoch.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// A fresh opaque identity.
pub(crate) fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// One content-addressed object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SnapshotRef {
    /// The content hash.
    pub(crate) hash: String,
    /// Byte length.
    pub(crate) bytes: u64,
    /// Sniffed media type.
    pub(crate) media_type: String,
    /// The format family used by the renderer.
    pub(crate) family: ResourceFamily,
    /// Unix permission bits at capture time.
    pub(crate) mode: u32,
    /// Where this image came from.
    pub(crate) source: SnapshotSource,
}

/// The before or after side of one revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub(crate) enum SnapshotState {
    /// The content is present in the object store.
    Present(SnapshotRef),
    /// The path did not exist.
    Absent,
    /// The capture could not be made; the reason is rendered to the reader.
    Unavailable(UnavailableReason),
}

impl SnapshotState {
    /// Whether this side is present.
    pub(crate) fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    /// Whether two captures hold the same content.
    ///
    /// The source (before/after) is provenance, not content: the same bytes
    /// read on both sides of an unchanged write must not create a revision.
    pub(crate) fn same_content(left: &Self, right: &Self) -> bool {
        match (left, right) {
            (Self::Absent, Self::Absent) => true,
            (Self::Present(left), Self::Present(right)) => left.hash == right.hash,
            (Self::Unavailable(left), Self::Unavailable(right)) => left == right,
            _ => false,
        }
    }

    /// The reader-facing reason when this side is unavailable.
    pub(crate) fn reason(&self) -> Option<&UnavailableReason> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Why a snapshot is unavailable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UnavailableReason {
    /// The file exceeded the capture ceiling.
    TooLarge { bytes: u64, limit: u64 },
    /// The format is binary and binary capture is off.
    BinaryNotCaptured,
    /// The policy or filesystem refused the read.
    PermissionDenied,
    /// The read failed for another reason.
    ReadFailed(String),
    /// The path is outside the workspace.
    OutsideWorkspace,
    /// Capture was disabled for the turn.
    CaptureDisabled,
    /// The file changed between the two reads.
    Raced,
    /// A legacy record has no before-image.
    LegacyNoBeforeImage,
}

/// Where a snapshot image was captured.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SnapshotSource {
    /// Read before a mutating tool ran.
    ToolBefore,
    /// Read after a mutating tool ran.
    ToolAfter,
    /// Found by a workspace scan.
    WorkspaceScan,
}

/// The format family the renderer uses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceFamily {
    /// Plain text.
    Text,
    /// Source code.
    Code,
    /// Structured configuration.
    Config,
    /// Markdown.
    Markdown,
    /// Raster image.
    Image,
    /// PDF document.
    Pdf,
    /// Word-processing document.
    Document,
    /// Spreadsheet.
    Spreadsheet,
    /// Archive.
    Archive,
    /// Other binary.
    Binary,
    /// Unknown.
    Unknown,
}

impl ResourceFamily {
    /// Whether a text diff can be attempted.
    pub(crate) fn is_text(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Code | Self::Config | Self::Markdown
        )
    }
}

/// Where a changeset came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureMode {
    /// Every revision came from a tool boundary.
    Exact,
    /// Every revision came from an observed scan.
    Observed,
    /// Both exact and observed revisions are present.
    Mixed,
    /// A legacy record with no before-image.
    Legacy,
}

/// The lifecycle of one changeset.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChangeSetStatus {
    /// Still accepting revisions.
    Open,
    /// Sealed and immutable.
    Sealed,
    /// Sealed with at least one unavailable snapshot.
    Partial,
    /// Capture itself failed.
    Failed,
}

/// One resource inside a changeset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ResourcePath {
    /// Path relative to the workspace root, with forward slashes.
    pub(crate) workspace_relative: String,
    /// The path as a reader sees it.
    pub(crate) display: String,
    /// The archive container, when this resource is inside one.
    pub(crate) container: Option<String>,
}

impl ResourcePath {
    /// Build a workspace-relative path.
    pub(crate) fn new(root: &Path, path: &Path) -> Self {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        Self {
            workspace_relative: relative,
            display: path.to_string_lossy().to_string(),
            container: None,
        }
    }
}

/// The net effect of one change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChangeKind {
    /// The resource did not exist before.
    Created,
    /// The resource existed on both sides.
    Modified,
    /// The resource is gone after.
    Deleted,
    /// The resource was renamed.
    Renamed { from: ResourcePath },
    /// The mode changed.
    ModeChanged { from: u32, to: u32 },
    /// The resource is unchanged.
    Unchanged,
}

/// What one tool did.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChangeOperation {
    /// Created a new file.
    Create,
    /// Replaced a file.
    Write,
    /// Replaced an exact string.
    Edit,
    /// Applied a grouped patch.
    Patch,
    /// Deleted a file.
    Delete,
    /// Renamed a file.
    Rename,
    /// Observed an external effect.
    Observed,
}

/// The tool that caused a revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ToolRef {
    /// Tool name.
    pub(crate) name: String,
    /// Stable call id when the caller has one.
    pub(crate) call_id: Option<String>,
}

impl ToolRef {
    /// Build a tool reference from the name alone.
    pub(crate) fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            call_id: None,
        }
    }
}

/// One append-only edit of a resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Revision {
    /// Stable revision id.
    pub(crate) id: String,
    /// One-based order inside the change.
    pub(crate) seq: u32,
    /// What the tool did.
    pub(crate) operation: ChangeOperation,
    /// The before image.
    pub(crate) before: SnapshotState,
    /// The after image.
    pub(crate) after: SnapshotState,
    /// The tool that caused it.
    pub(crate) tool: Option<ToolRef>,
    /// Capture time.
    pub(crate) captured_at_ms: u64,
}

/// One resource in a changeset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Change {
    /// Stable change id.
    pub(crate) id: String,
    /// The resource.
    pub(crate) path: ResourcePath,
    /// The net effect.
    pub(crate) kind: ChangeKind,
    /// Every edit, oldest first.
    pub(crate) revisions: Vec<Revision>,
    /// First touch.
    pub(crate) first_seen_ms: u64,
    /// Last touch.
    pub(crate) last_seen_ms: u64,
}

/// One sealed or open unit of work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ChangeSet {
    /// Manifest schema.
    pub(crate) schema: u32,
    /// Stable id.
    pub(crate) id: String,
    /// Origin session.
    pub(crate) session: String,
    /// Origin turn, when known.
    pub(crate) turn: Option<String>,
    /// Origin task, when known.
    pub(crate) task: Option<String>,
    /// Origin attempt, when known.
    pub(crate) attempt: Option<String>,
    /// Parent changeset for a child run.
    pub(crate) parent: Option<String>,
    /// Capture quality.
    pub(crate) capture: CaptureMode,
    /// Lifecycle.
    pub(crate) status: ChangeSetStatus,
    /// Creation time.
    pub(crate) created_at_ms: u64,
    /// Seal time.
    pub(crate) sealed_at_ms: Option<u64>,
    /// Every changed resource.
    pub(crate) changes: Vec<Change>,
    /// Digest of the sealed manifest.
    pub(crate) digest: Option<String>,
}

impl ChangeSet {
    /// Open a changeset for one execution scope.
    pub(crate) fn new(session: impl Into<String>, turn: Option<String>) -> Self {
        Self {
            schema: SCHEMA,
            id: new_id(),
            session: session.into(),
            turn,
            task: None,
            attempt: None,
            parent: None,
            capture: CaptureMode::Exact,
            status: ChangeSetStatus::Open,
            created_at_ms: now_ms(),
            sealed_at_ms: None,
            changes: Vec::new(),
            digest: None,
        }
    }

    /// Append one revision. Returns false when the call changed nothing.
    pub(crate) fn add_revision(
        &mut self,
        path: ResourcePath,
        operation: ChangeOperation,
        before: SnapshotState,
        after: SnapshotState,
        tool: Option<ToolRef>,
    ) -> bool {
        if SnapshotState::same_content(&before, &after) {
            return false;
        }
        let now = now_ms();
        let existing = self
            .changes
            .iter()
            .position(|change| change.path.workspace_relative == path.workspace_relative);
        let index = existing.unwrap_or_else(|| {
            self.changes.push(Change {
                id: new_id(),
                path: path.clone(),
                kind: ChangeKind::Modified,
                revisions: Vec::new(),
                first_seen_ms: now,
                last_seen_ms: now,
            });
            self.changes.len() - 1
        });
        let change = &mut self.changes[index];
        let seq = change.revisions.len() as u32 + 1;
        change.revisions.push(Revision {
            id: new_id(),
            seq,
            operation,
            before,
            after,
            tool,
            captured_at_ms: now,
        });
        change.first_seen_ms = change.first_seen_ms.min(now);
        change.last_seen_ms = now;
        change.kind = net_kind(&change.revisions);
        if change
            .revisions
            .iter()
            .any(|revision| revision.before.reason().is_some() || revision.after.reason().is_some())
        {
            self.capture = match self.capture {
                CaptureMode::Exact => CaptureMode::Mixed,
                other => other,
            };
        }
        true
    }

    /// Compute the canonical digest.
    pub(crate) fn recompute_digest(&mut self) -> String {
        let mut rows: Vec<String> = Vec::new();
        for change in &self.changes {
            for revision in &change.revisions {
                rows.push(format!(
                    "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    change.path.workspace_relative,
                    kind_name(&change.kind),
                    revision.seq,
                    operation_name(revision.operation),
                    state_digest(&revision.before),
                    state_digest(&revision.after),
                ));
            }
        }
        rows.sort();
        let mut hasher = Sha256::new();
        for row in rows {
            hasher.update((row.len() as u64).to_be_bytes());
            hasher.update(row.as_bytes());
        }
        let digest = format!("sha256:{}", hex_lower(hasher.finalize().as_slice()));
        self.digest = Some(digest.clone());
        digest
    }

    /// Seal the changeset, computing its digest.
    pub(crate) fn seal(&mut self) -> String {
        self.sealed_at_ms = Some(now_ms());
        self.status = if self
            .changes
            .iter()
            .flat_map(|change| &change.revisions)
            .any(|revision| revision.before.reason().is_some() || revision.after.reason().is_some())
        {
            ChangeSetStatus::Partial
        } else {
            ChangeSetStatus::Sealed
        };
        self.recompute_digest()
    }

    /// Total revisions.
    pub(crate) fn revision_count(&self) -> usize {
        self.changes
            .iter()
            .map(|change| change.revisions.len())
            .sum()
    }
}

fn net_kind(revisions: &[Revision]) -> ChangeKind {
    let Some(first) = revisions.first() else {
        return ChangeKind::Unchanged;
    };
    let last = revisions.last().expect("a revision exists");
    match (first.before.is_present(), last.after.is_present()) {
        (false, true) => ChangeKind::Created,
        (true, false) => ChangeKind::Deleted,
        (true, true) if SnapshotState::same_content(&first.before, &last.after) => {
            ChangeKind::Unchanged
        }
        (true, true) => ChangeKind::Modified,
        (false, false) => ChangeKind::Unchanged,
    }
}

fn kind_name(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Created => "created",
        ChangeKind::Modified => "modified",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed { .. } => "renamed",
        ChangeKind::ModeChanged { .. } => "mode",
        ChangeKind::Unchanged => "unchanged",
    }
}

fn operation_name(operation: ChangeOperation) -> &'static str {
    match operation {
        ChangeOperation::Create => "create",
        ChangeOperation::Write => "write",
        ChangeOperation::Edit => "edit",
        ChangeOperation::Patch => "patch",
        ChangeOperation::Delete => "delete",
        ChangeOperation::Rename => "rename",
        ChangeOperation::Observed => "observed",
    }
}

/// Lower-case hex for a digest.
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn state_digest(state: &SnapshotState) -> String {
    match state {
        SnapshotState::Present(snapshot) => snapshot.hash.clone(),
        SnapshotState::Absent => "absent".to_string(),
        SnapshotState::Unavailable(_) => "unavailable".to_string(),
    }
}

/// How risky the changes look to the deterministic reviewer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskLevel {
    /// Nothing stands out.
    Low,
    /// Worth a human look.
    Medium,
    /// Needs explicit attention.
    High,
    /// Blocks silent approval.
    Critical,
}

/// One reason the risk is elevated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskReason {
    /// A snapshot was unavailable.
    SnapshotUnavailable,
    /// A binary or opaque format changed.
    OpaqueFormat,
    /// The changeset is large.
    LargeChange,
    /// The capture was observed, not exact.
    ObservedCapture,
    /// No verification was recorded.
    NoVerification,
    /// The review is stale.
    Stale,
}

/// The deterministic risk assessment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RiskAssessment {
    /// Overall level.
    pub(crate) level: RiskLevel,
    /// Why.
    pub(crate) reasons: Vec<RiskReason>,
    /// Whether a human must decide.
    pub(crate) requires_human: bool,
}

/// One review finding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Finding {
    /// Stable id.
    pub(crate) id: String,
    /// The affected change, when narrow enough.
    pub(crate) change_id: Option<String>,
    /// Severity.
    pub(crate) severity: Severity,
    /// Kind.
    pub(crate) kind: FindingKind,
    /// State.
    pub(crate) status: FindingStatus,
    /// Short title.
    pub(crate) title: String,
    /// Explanation.
    pub(crate) detail: String,
    /// Who produced it.
    pub(crate) source: FindingSource,
    /// Evidence references.
    pub(crate) evidence: Vec<EvidenceRef>,
    /// Creation time.
    pub(crate) created_at_ms: u64,
}

/// Finding severity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    /// Informational.
    Info,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Critical.
    Critical,
}

/// Finding category.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FindingKind {
    /// Correctness.
    Correctness,
    /// Security.
    Security,
    /// Tests.
    Test,
    /// Performance.
    Performance,
    /// Documentation.
    Documentation,
    /// Compatibility.
    Compatibility,
    /// Data loss.
    DataLoss,
    /// Policy.
    Policy,
    /// Other.
    Other,
}

/// Finding lifecycle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FindingStatus {
    /// Open.
    Open,
    /// Accepted.
    Accepted,
    /// Dismissed.
    Dismissed,
    /// Fixed.
    Fixed,
    /// A human must decide.
    NeedsHuman,
}

/// Who produced a finding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FindingSource {
    /// A deterministic rule.
    Deterministic,
    /// An agent.
    Agent,
    /// A human.
    Human,
}

/// One evidence reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EvidenceRef {
    /// Stable id.
    pub(crate) id: String,
    /// Kind label.
    pub(crate) kind: String,
    /// Short label.
    pub(crate) label: String,
    /// A content hash when one applies.
    pub(crate) digest: Option<String>,
}

/// One verification record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Verification {
    /// Stable id.
    pub(crate) id: String,
    /// Kind label.
    pub(crate) kind: String,
    /// Status label.
    pub(crate) status: String,
    /// Command, when one ran.
    pub(crate) command: Option<String>,
    /// Exit code.
    pub(crate) exit_code: Option<i32>,
    /// Summary.
    pub(crate) summary: String,
    /// When it started.
    pub(crate) started_at_ms: Option<u64>,
    /// When it settled.
    pub(crate) finished_at_ms: Option<u64>,
}

/// A review decision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    /// Approve the exact digest.
    Approve,
    /// Reject.
    Reject,
    /// Ask for changes.
    RequestChanges,
}

/// One recorded decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReviewDecision {
    /// Stable id.
    pub(crate) id: String,
    /// The decision.
    pub(crate) decision: Decision,
    /// Who decided.
    pub(crate) actor: String,
    /// Why.
    pub(crate) rationale: String,
    /// The digest the decision was made against.
    pub(crate) digest: String,
    /// When.
    pub(crate) at_ms: u64,
}

/// The review aggregate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Review {
    /// Manifest schema.
    pub(crate) schema: u32,
    /// Stable id.
    pub(crate) id: String,
    /// The changeset this review covers.
    pub(crate) change_set_id: String,
    /// The digest the review was opened against.
    pub(crate) digest: String,
    /// Lifecycle.
    pub(crate) status: ReviewStatus,
    /// Deterministic risk.
    pub(crate) risk: RiskAssessment,
    /// Findings.
    pub(crate) findings: Vec<Finding>,
    /// Verifications.
    pub(crate) verifications: Vec<Verification>,
    /// Decisions, oldest first.
    pub(crate) decisions: Vec<ReviewDecision>,
    /// Opened at.
    pub(crate) created_at_ms: u64,
    /// Last changed.
    pub(crate) updated_at_ms: u64,
}

/// Review lifecycle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewStatus {
    /// Not yet reviewed.
    Draft,
    /// Being reviewed.
    InReview,
    /// Changes requested.
    ChangesRequested,
    /// Approved at the bound digest.
    Approved,
    /// Rejected.
    Rejected,
    /// The changeset digest moved after the review opened.
    Stale,
}

impl Review {
    /// Build the deterministic review for a sealed changeset.
    pub(crate) fn open(set: &ChangeSet) -> Self {
        let mut reasons = Vec::new();
        let unavailable = set
            .changes
            .iter()
            .flat_map(|change| &change.revisions)
            .any(|revision| {
                revision.before.reason().is_some() || revision.after.reason().is_some()
            });
        if unavailable {
            reasons.push(RiskReason::SnapshotUnavailable);
        }
        if set.changes.iter().any(|change| {
            change
                .revisions
                .last()
                .and_then(|revision| match &revision.after {
                    SnapshotState::Present(snapshot) => Some(snapshot.family),
                    _ => None,
                })
                .is_some_and(|family| !family.is_text())
        }) {
            reasons.push(RiskReason::OpaqueFormat);
        }
        if set.changes.len() > 20 {
            reasons.push(RiskReason::LargeChange);
        }
        if matches!(set.capture, CaptureMode::Observed | CaptureMode::Mixed) {
            reasons.push(RiskReason::ObservedCapture);
        }
        reasons.push(RiskReason::NoVerification);
        let level = if reasons.contains(&RiskReason::SnapshotUnavailable) {
            RiskLevel::High
        } else if reasons.iter().any(|reason| {
            matches!(
                reason,
                RiskReason::ObservedCapture | RiskReason::OpaqueFormat | RiskReason::LargeChange
            )
        }) {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };
        let now = now_ms();
        let findings = reasons.iter().map(|reason| finding_for(*reason)).collect();
        Self {
            schema: SCHEMA,
            id: new_id(),
            change_set_id: set.id.clone(),
            digest: set.digest.clone().unwrap_or_default(),
            status: ReviewStatus::InReview,
            risk: RiskAssessment {
                level,
                reasons,
                requires_human: level != RiskLevel::Low,
            },
            findings,
            verifications: Vec::new(),
            decisions: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    /// Record a decision if the review still points at the current digest.
    pub(crate) fn decide(
        &mut self,
        decision: Decision,
        actor: impl Into<String>,
        rationale: String,
        current_digest: &str,
    ) -> Result<(), String> {
        if self.digest != current_digest {
            self.status = ReviewStatus::Stale;
            return Err("the change set changed after this review opened".to_string());
        }
        self.status = match decision {
            Decision::Approve => ReviewStatus::Approved,
            Decision::Reject => ReviewStatus::Rejected,
            Decision::RequestChanges => ReviewStatus::ChangesRequested,
        };
        self.decisions.push(ReviewDecision {
            id: new_id(),
            decision,
            actor: actor.into(),
            rationale,
            digest: current_digest.to_string(),
            at_ms: now_ms(),
        });
        self.updated_at_ms = now_ms();
        Ok(())
    }

    /// Mark the review stale when the digest moved.
    pub(crate) fn refresh(&mut self, current_digest: &str) {
        if self.digest != current_digest {
            self.status = ReviewStatus::Stale;
        }
    }
}

fn finding_for(reason: RiskReason) -> Finding {
    let (severity, kind, title, detail) = match reason {
        RiskReason::SnapshotUnavailable => (
            Severity::High,
            FindingKind::Correctness,
            "Snapshot unavailable",
            "At least one side of a change could not be captured. Review the available evidence; do not treat this as a complete diff.",
        ),
        RiskReason::OpaqueFormat => (
            Severity::Medium,
            FindingKind::Compatibility,
            "Opaque format",
            "The changeset includes a binary or document format. The diff is metadata-level, not a semantic comparison.",
        ),
        RiskReason::LargeChange => (
            Severity::Medium,
            FindingKind::Other,
            "Large change",
            "This changeset touches more than 20 files. Review the tree by area before approving.",
        ),
        RiskReason::ObservedCapture => (
            Severity::Medium,
            FindingKind::Correctness,
            "Observed changes",
            "Some changes were observed from a shell or process, not captured at a tool boundary. Unrelated edits may be included.",
        ),
        RiskReason::NoVerification => (
            Severity::Info,
            FindingKind::Test,
            "No verification recorded",
            "No build, test, lint or manual verification is attached to this review.",
        ),
        RiskReason::Stale => (
            Severity::High,
            FindingKind::Correctness,
            "Review is stale",
            "The changeset changed after this review opened. Re-read the current digest before deciding.",
        ),
    };
    Finding {
        id: new_id(),
        change_id: None,
        severity,
        kind,
        status: FindingStatus::Open,
        title: title.to_string(),
        detail: detail.to_string(),
        source: FindingSource::Deterministic,
        evidence: Vec::new(),
        created_at_ms: now_ms(),
    }
}

/// A text diff line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DiffLine {
    /// The diff mark: plus, minus or context.
    pub(crate) mark: String,
    /// Old line number, one-based.
    pub(crate) old_line: Option<u32>,
    /// New line number, one-based.
    pub(crate) new_line: Option<u32>,
    /// The line text.
    pub(crate) text: String,
}

/// One structural node in a diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StructuralNode {
    /// Stable id inside the diff.
    pub(crate) id: String,
    /// Parent id.
    pub(crate) parent: Option<String>,
    /// Kind label: function, struct, config-key, section.
    pub(crate) kind: String,
    /// Reader-facing name.
    pub(crate) qualified_name: String,
    /// One of added, removed, modified, moved.
    pub(crate) change: String,
    /// Old line, one-based.
    pub(crate) old_line: Option<u32>,
    /// New line, one-based.
    pub(crate) new_line: Option<u32>,
    /// Extra detail.
    pub(crate) detail: String,
}

/// A prepared diff for one change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FileDiff {
    /// Change id.
    pub(crate) change_id: String,
    /// Path.
    pub(crate) path: String,
    /// Net kind label.
    pub(crate) kind: String,
    /// Whether the content is binary.
    pub(crate) binary: bool,
    /// Old byte length.
    pub(crate) old_bytes: u64,
    /// New byte length.
    pub(crate) new_bytes: u64,
    /// Added lines.
    pub(crate) added: usize,
    /// Removed lines.
    pub(crate) removed: usize,
    /// Structural nodes.
    pub(crate) structural: Vec<StructuralNode>,
    /// Text lines.
    pub(crate) lines: Vec<DiffLine>,
    /// Why a full diff is unavailable.
    pub(crate) note: Option<String>,
}

impl FileDiff {
    /// An empty diff for a change that cannot be read.
    pub(crate) fn unavailable(change: &Change, note: impl Into<String>) -> Self {
        Self {
            change_id: change.id.clone(),
            path: change.path.workspace_relative.clone(),
            kind: kind_name(&change.kind).to_string(),
            binary: false,
            old_bytes: 0,
            new_bytes: 0,
            added: 0,
            removed: 0,
            structural: Vec::new(),
            lines: Vec::new(),
            note: Some(note.into()),
        }
    }
}

/// A changeset and its review, as the UI loads them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReviewSnapshot {
    /// The latest changeset.
    pub(crate) changeset: ChangeSet,
    /// Its review.
    pub(crate) review: Review,
}

/// The workspace review directory.
pub(crate) fn review_root(workspace: &Path) -> PathBuf {
    workspace.join(".bsh").join("review")
}

/// The default actor for a local human decision.
pub(crate) fn local_actor() -> String {
    "local-user".to_string()
}
