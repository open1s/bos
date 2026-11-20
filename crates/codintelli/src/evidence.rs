//! TS-03 generic query evidence: source-bound, coverage-aware, deterministic.
//!
//! Evidence is the language-neutral output of one native Tree-sitter query run
//! over one parsed document. It records *what matched, where, in which
//! snapshot, under which provider and query definition*, and never a second
//! syntax tree, node-shaped DTO, or semantic resolution claim.
//!
//! # Ordering
//!
//! [`EvidenceBatch::sort_canonical`] defines the public ordering contract:
//! workspace-relative path, query id, query version, pattern index, match
//! index within the pattern, capture index, then source byte range. Hash-map
//! iteration order never reaches a public result.
//!
//! # Invalidation
//!
//! Evidence carries the fingerprint of the exact source bytes it was produced
//! from, the provider identity used to parse, and the query identity that
//! produced it. A change to any of those makes prior evidence stale; nothing is
//! relabelled as current. See [`crate::evidence_index::EvidenceIndex`].

use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::WorkspaceId;
use crate::classification::LanguageId;
use crate::syntax::SourceRange;
use crate::syntax::query::{QueryExecutionStatus, QueryOutputLimit};

/// Digest algorithm used to fingerprint source snapshots.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FingerprintAlgorithm {
    /// SHA-256 over the exact source bytes.
    Sha256,
}

impl FingerprintAlgorithm {
    /// Stable machine-readable algorithm name, as persisted in provenance.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
        }
    }

    /// Algorithm identity including the version of this fingerprint scheme.
    ///
    /// A change to bytes-to-digest semantics must bump this string so old
    /// evidence is recognised as incompatible instead of compared as equal.
    #[must_use]
    pub const fn versioned_id(&self) -> &'static str {
        match self {
            Self::Sha256 => "sha256/1",
        }
    }
}

/// Fingerprint of the exact source bytes a piece of evidence was produced from.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceFingerprint {
    algorithm: FingerprintAlgorithm,
    digest: [u8; 32],
    source_len: usize,
}

impl SourceFingerprint {
    /// Fingerprint the supplied bytes.
    #[must_use]
    pub fn of(source: &[u8]) -> Self {
        Self::with_algorithm(source, FingerprintAlgorithm::Sha256)
    }

    /// Fingerprint the supplied bytes with an explicit algorithm.
    #[must_use]
    pub fn with_algorithm(source: &[u8], algorithm: FingerprintAlgorithm) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(source);
        let output = hasher.finalize();
        Self::from_digest(algorithm, output, source.len())
    }

    fn from_digest(
        algorithm: FingerprintAlgorithm,
        output: impl AsRef<[u8]>,
        source_len: usize,
    ) -> Self {
        let mut digest = [0u8; 32];
        let bytes = output.as_ref();
        let len = digest.len().min(bytes.len());
        digest[..len].copy_from_slice(&bytes[..len]);
        Self {
            algorithm,
            digest,
            source_len,
        }
    }

    /// Algorithm that produced this digest.
    #[must_use]
    pub const fn algorithm(&self) -> FingerprintAlgorithm {
        self.algorithm
    }

    /// Raw digest bytes.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    /// Number of source bytes that were hashed.
    #[must_use]
    pub const fn source_len(&self) -> usize {
        self.source_len
    }

    /// Lower-case hexadecimal digest, suitable for diagnostics and exports.
    #[must_use]
    pub fn hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.digest {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        out
    }

    /// Whether the supplied bytes are exactly this snapshot.
    #[must_use]
    pub fn matches(&self, source: &[u8]) -> bool {
        *self == Self::with_algorithm(source, self.algorithm)
    }
}

/// Stable identity of the provider and language that parsed the source.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProviderIdentity {
    /// Classified language.
    pub language: LanguageId,
    /// Stable provider identifier.
    pub provider_id: String,
    /// Provider/grammar version; a change invalidates prior evidence.
    pub provider_version: String,
}

/// Stable identity of the query definition that produced evidence.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct QueryIdentity {
    /// Registered query identifier.
    pub query_id: String,
    /// Registered query definition version.
    pub query_version: String,
}

/// Stable identity of the capture a record came from.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CaptureIdentity {
    /// Tree-sitter capture index.
    pub capture_index: u32,
    /// Captured name from the query definition.
    pub capture_name: String,
}

/// Parse coverage of the document evidence was produced from.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ParseCoverage {
    /// The document parsed without Tree-sitter error or missing nodes.
    Complete,
    /// The document contained recovered syntax.
    Recovered,
}

/// Query execution coverage for one document/query pair.
///
/// Anything other than [`Self::Complete`] means the evidence produced by that
/// execution is incomplete and must not be presented as a complete result set.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum QueryCoverage {
    /// The cursor ran to exhaustion without cancellation or limits.
    Complete,
    /// The query was not executed: unknown, unsupported, or incompatible input.
    NotExecuted,
    /// A retained-output cap was reached.
    OutputLimited(QueryOutputLimit),
    /// Tree-sitter exhausted its in-progress match limit.
    MatchLimitExceeded,
    /// The shared budget cancelled execution.
    Cancelled,
    /// The shared workspace deadline expired during execution.
    DeadlineExceeded,
}

impl QueryCoverage {
    /// Whether the execution ran to exhaustion.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }

    fn severity(self) -> u8 {
        match self {
            Self::Complete => 0,
            Self::NotExecuted => 1,
            Self::OutputLimited(_) => 2,
            Self::MatchLimitExceeded => 3,
            Self::Cancelled => 4,
            Self::DeadlineExceeded => 5,
        }
    }

    fn from_execution(status: QueryExecutionStatus) -> Self {
        match status {
            QueryExecutionStatus::Complete => Self::Complete,
            QueryExecutionStatus::Cancelled => Self::Cancelled,
            QueryExecutionStatus::DeadlineExceeded => Self::DeadlineExceeded,
            QueryExecutionStatus::OutputLimited(limit) => Self::OutputLimited(limit),
            QueryExecutionStatus::MatchLimitExceeded => Self::MatchLimitExceeded,
        }
    }
}

/// Coverage pair carried by every evidence record and batch.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct EvidenceCoverage {
    /// Parse coverage of the source document.
    pub parse: Option<ParseCoverage>,
    /// Query execution coverage.
    pub query: Option<QueryCoverage>,
}

impl EvidenceCoverage {
    /// Coverage of a cleanly parsed document whose query ran to exhaustion.
    #[must_use]
    pub const fn complete() -> Self {
        Self {
            parse: Some(ParseCoverage::Complete),
            query: Some(QueryCoverage::Complete),
        }
    }

    /// Whether both parse and query coverage are complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.parse == Some(ParseCoverage::Complete)
            && self.query.is_some_and(QueryCoverage::is_complete)
    }

    /// Merge another coverage into this one, preserving the worst state.
    ///
    /// A recovered parse always wins over a complete parse, and a more severe
    /// query state wins over a less severe one. Merging is associative and
    /// commutative, so aggregation across files cannot depend on completion
    /// order.
    pub fn merge(&mut self, other: Self) {
        self.parse = match (self.parse, other.parse) {
            (Some(ParseCoverage::Recovered), _) | (_, Some(ParseCoverage::Recovered)) => {
                Some(ParseCoverage::Recovered)
            }
            (Some(ParseCoverage::Complete), _) | (_, Some(ParseCoverage::Complete)) => {
                Some(ParseCoverage::Complete)
            }
            (None, None) => None,
        };
        self.query = match (self.query, other.query) {
            (Some(a), Some(b)) => Some(if a.severity() >= b.severity() { a } else { b }),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
    }
}

/// One generic, source-bound evidence record.
///
/// This is deliberately not node-shaped: it carries query/capture identity and
/// a byte range, never a Tree-sitter node or a language-specific structure.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EvidenceRecord {
    /// Host-supplied identity of the checkout.
    pub workspace_id: WorkspaceId,
    /// Workspace-relative source path.
    pub relative_path: PathBuf,
    /// Fingerprint of the exact source snapshot used.
    pub fingerprint: SourceFingerprint,
    /// Provider and language identity.
    pub provider: ProviderIdentity,
    /// Query definition identity.
    pub query: QueryIdentity,
    /// Capture identity within the query.
    pub capture: CaptureIdentity,
    /// Half-open byte range in the fingerprinted snapshot.
    pub range: SourceRange,
    /// Pattern that matched.
    pub pattern_index: usize,
    /// Ordinal of the match within this document/query execution.
    pub match_index: usize,
    /// Parse and query coverage.
    pub coverage: EvidenceCoverage,
}

impl EvidenceRecord {
    /// Total deterministic ordering key; ties are impossible for distinct
    /// captures of the same document and query.
    fn sort_key(&self) -> impl Ord + '_ {
        (
            &self.relative_path,
            &self.query.query_id,
            &self.query.query_version,
            self.pattern_index,
            self.match_index,
            self.capture.capture_index,
            self.range.start_byte,
            self.range.end_byte,
        )
    }
}

/// Why a file or query produced no (complete) evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceDiagnosticKind {
    /// No provider is registered for the classified language.
    UnsupportedLanguage(LanguageId),
    /// The registered query does not exist in the registry.
    UnknownQuery(String),
    /// Query execution was rejected for this document.
    IncompatibleDocument,
    /// The query ran but produced an incomplete result.
    IncompleteExecution,
}

/// A diagnostic attached to an evidence batch. It never contains source text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceDiagnostic {
    /// Workspace-relative path, when the diagnostic is file-specific.
    pub relative_path: Option<PathBuf>,
    /// Diagnostic category.
    pub kind: EvidenceDiagnosticKind,
    /// Non-source message text.
    pub message: String,
}

/// Deterministically ordered evidence produced by one collection pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceBatch {
    /// Host-supplied identity of the checkout.
    pub workspace_id: WorkspaceId,
    /// Evidence records in canonical order.
    pub records: Vec<EvidenceRecord>,
    /// Diagnostics for files/queries that produced no usable evidence.
    pub diagnostics: Vec<EvidenceDiagnostic>,
    /// Worst coverage across the whole batch.
    pub coverage: EvidenceCoverage,
    /// Documents visited.
    pub files_queried: u64,
    /// Query executions performed.
    pub query_executions: u64,
    /// Files skipped because no provider supported their language.
    pub unsupported_files: u64,
}

impl EvidenceBatch {
    /// Empty batch for one workspace.
    #[must_use]
    pub const fn new(workspace_id: WorkspaceId) -> Self {
        Self {
            workspace_id,
            records: Vec::new(),
            diagnostics: Vec::new(),
            coverage: EvidenceCoverage {
                parse: None,
                query: None,
            },
            files_queried: 0,
            query_executions: 0,
            unsupported_files: 0,
        }
    }

    /// Sort records and diagnostics into the documented canonical order.
    pub fn sort_canonical(&mut self) {
        self.records.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        self.diagnostics.sort_by(|a, b| {
            a.relative_path
                .cmp(&b.relative_path)
                .then_with(|| format!("{:?}", a.kind).cmp(&format!("{:?}", b.kind)))
                .then_with(|| a.message.cmp(&b.message))
        });
    }

    /// Deterministic digest of the batch's evidence content.
    ///
    /// Repeated runs over identical source, provider, and query identities
    /// produce the same digest; it is a serialization-independent stand-in for
    /// byte-for-byte comparison of exported evidence.
    #[must_use]
    pub fn content_digest(&self) -> SourceFingerprint {
        let mut hasher = Sha256::new();
        hasher.update(self.workspace_id.as_uuid().as_bytes());
        let mut field = |bytes: &[u8]| {
            hasher.update(bytes);
            hasher.update(b"\0");
        };
        for record in &self.records {
            field(record.relative_path.to_string_lossy().as_bytes());
            field(record.fingerprint.hex().as_bytes());
            field(record.provider.provider_id.as_bytes());
            field(record.provider.provider_version.as_bytes());
            field(record.query.query_id.as_bytes());
            field(record.query.query_version.as_bytes());
            field(record.capture.capture_index.to_string().as_bytes());
            field(record.capture.capture_name.as_bytes());
            field(record.range.start_byte.to_string().as_bytes());
            field(record.range.end_byte.to_string().as_bytes());
        }
        SourceFingerprint::from_digest(FingerprintAlgorithm::Sha256, hasher.finalize(), self.records.len())
    }

    /// Whether the batch is complete and contains at least one record.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.coverage.is_complete() && self.diagnostics.is_empty()
    }
}
