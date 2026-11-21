//! Native Tree-sitter query registration and bounded execution.
//!
//! This module is TS-02: a thin syntax-access layer over the native
//! Tree-sitter trees owned by [`ParsedDocument`]. It does not define a second
//! syntax tree, AST, or language-specific declaration model.
//!
//! # Compatibility
//!
//! A query is compiled once, at setup time, against the grammar a
//! [`LanguageProvider`] exposes through
//! [`LanguageProvider::tree_sitter_language`]. Registry metadata pins the
//! query to `(language, provider id, provider version, query id, query
//! version)`. Execution rejects a document whose language, provider id, or
//! provider version differs. A provider version must change whenever the
//! grammar or query-relevant parser configuration changes.
//!
//! # Execution
//!
//! Every execution creates a fresh [`tree_sitter::QueryCursor`] and borrows
//! the document's native tree. Captures are retained as generic metadata:
//! capture index and source byte range in the document's original coordinate
//! space. Output is bounded during streaming by match and capture caps; the
//! result carries an explicit non-complete [`QueryExecutionStatus`] whenever
//! the operation is cancelled, hits the shared deadline, reaches an output
//! cap, or exhausts Tree-sitter's in-progress match limit.
//!
//! Cancellation is cooperative. The pinned Tree-sitter cursor exposes a
//! progress callback, not hard wall-clock preemption, so a query can be
//! computationally expensive even when its output is small. The shared
//! [`ScanBudget`] remains the authority for cancellation and deadlines.
//!
//! A successful match is syntax evidence only. It is never a claim of
//! compiler-resolved semantics.

use std::ops::ControlFlow;
use std::sync::Arc;

use tree_sitter::{
    Query, QueryCursor, QueryCursorOptions, QueryCursorState, QueryErrorKind, StreamingIterator,
};

use crate::classification::LanguageId;
use crate::scan_budget::{ScanBudget, ScanBudgetError};

use super::{LanguageProvider, ParsedDocument, ParsedDocumentStatus, SourceRange};

/// Maximum accepted byte length of a query identifier.
pub const MAX_QUERY_ID_BYTES: usize = 128;

/// Maximum accepted byte length of a query definition version.
pub const MAX_QUERY_VERSION_BYTES: usize = 64;

/// Smallest in-progress match limit accepted by the pinned Tree-sitter cursor.
pub const MIN_MATCH_LIMIT: u32 = 1;

/// Largest in-progress match limit accepted by the pinned Tree-sitter cursor.
pub const MAX_MATCH_LIMIT: u32 = 65_536;

/// Absolute maximum accepted value of [`QueryExecutionLimits::max_matches`].
///
/// Callers may configure any value from 1 through this constant. The default
/// (10,000) is deliberately far lower so interactive retrieval stays small;
/// batch indexing may raise the limit up to this documented ceiling.
pub const MAX_RETAINED_MATCHES: usize = 1_000_000;

/// Absolute maximum accepted value of [`QueryExecutionLimits::max_captures`].
///
/// Callers may configure any value from 1 through this constant. The default
/// (100,000) is deliberately far lower so interactive retrieval stays small;
/// batch indexing may raise the limit up to this documented ceiling.
pub const MAX_RETAINED_CAPTURES: usize = 5_000_000;

/// Absolute maximum accepted value of the query registry's per-query source
/// limit.
///
/// Bounds the query source accepted for compilation before Tree-sitter sees
/// it. The default (64 KiB) is far lower; this ceiling exists so a registry
/// configured from external bundles cannot be handed an effectively
/// unbounded definition.
pub const MAX_QUERY_SOURCE_BYTES: usize = 1 << 20;

/// Absolute maximum accepted value of the query registry's query-count limit.
pub const MAX_REGISTRY_QUERIES: usize = 4_096;

/// Absolute maximum accepted value of the query registry's total-source limit.
pub const MAX_REGISTRY_TOTAL_SOURCE_BYTES: usize = 8 << 20;

/// Validate a query identifier or version.
///
/// A valid identity is non-empty, no longer than the supplied byte maximum,
/// starts with an ASCII letter or digit, and contains only ASCII
/// alphanumerics, `.`, `_`, or `-`. Whitespace, separators, and
/// filesystem-style paths are rejected so identities remain metadata rather
/// than paths or shell fragments.
fn valid_query_identity(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Category of a Tree-sitter query compilation error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryCompileErrorKind {
    /// The query text is not syntactically valid.
    Syntax,
    /// The query names a node kind that the grammar does not define.
    NodeType,
    /// The query names a field that the grammar does not define.
    Field,
    /// The query names or uses an invalid capture.
    Capture,
    /// The query contains an invalid predicate.
    Predicate,
    /// The query's pattern structure is invalid.
    Structure,
    /// The query is not compatible with the supplied language.
    Language,
}

impl From<QueryErrorKind> for QueryCompileErrorKind {
    fn from(kind: QueryErrorKind) -> Self {
        match kind {
            QueryErrorKind::Syntax => Self::Syntax,
            QueryErrorKind::NodeType => Self::NodeType,
            QueryErrorKind::Field => Self::Field,
            QueryErrorKind::Capture => Self::Capture,
            QueryErrorKind::Predicate => Self::Predicate,
            QueryErrorKind::Structure => Self::Structure,
            QueryErrorKind::Language => Self::Language,
        }
    }
}

/// Reasons a query cannot be registered.
#[derive(Debug, thiserror::Error)]
pub enum QueryRegistrationError {
    /// The query identifier is empty, too long, or contains unsupported characters.
    #[error(
        "query id must start with an ASCII letter or digit, contain at most 128 bytes, \
         and use only ASCII alphanumerics, '.', '_', or '-'"
    )]
    InvalidQueryId,
    /// The query version is empty, too long, or contains unsupported characters.
    #[error(
        "query version must start with an ASCII letter or digit, contain at most 64 bytes, \
         and use only ASCII alphanumerics, '.', '_', or '-'"
    )]
    InvalidQueryVersion,
    /// A query with this identifier is already registered.
    #[error("query id `{query_id}` is already registered")]
    DuplicateQueryId {
        /// Identifier of the existing registration.
        query_id: String,
    },
    /// The provider does not expose a Tree-sitter grammar.
    #[error("provider `{provider_id}` does not expose a Tree-sitter grammar")]
    MissingGrammar {
        /// Stable identifier of the rejecting provider.
        provider_id: String,
    },
    /// Tree-sitter rejected the query source.
    ///
    /// The message and location come from the pinned Tree-sitter binding and
    /// never include the complete query source.
    #[error(
        "query `{query_id}` failed to compile ({kind:?}) at row {row}, column {column}, \
         offset {offset}: {message}"
    )]
    Compile {
        /// Identifier supplied for the rejected query.
        query_id: String,
        /// Tree-sitter error category.
        kind: QueryCompileErrorKind,
        /// Zero-based source row reported by Tree-sitter.
        row: usize,
        /// Zero-based source column reported by Tree-sitter.
        column: usize,
        /// Zero-based source byte offset reported by Tree-sitter.
        offset: usize,
        /// Short Tree-sitter diagnostic; never the full query source.
        message: String,
    },
}

/// A native Tree-sitter query bound to one provider and grammar version.
///
/// Registered queries are immutable after registration. The native
/// [`Query`] is shared through an [`Arc`] so handles can be cloned and used
/// concurrently. A handle never owns a cursor.
#[derive(Clone)]
pub struct RegisteredQuery {
    query_id: String,
    query_version: String,
    language: LanguageId,
    provider_id: String,
    provider_version: String,
    query: Arc<Query>,
}

impl std::fmt::Debug for RegisteredQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegisteredQuery")
            .field("query_id", &self.query_id)
            .field("query_version", &self.query_version)
            .field("language", &self.language)
            .field("provider_id", &self.provider_id)
            .field("provider_version", &self.provider_version)
            .finish_non_exhaustive()
    }
}

impl RegisteredQuery {
    /// Stable caller-supplied query identifier.
    #[must_use]
    pub fn query_id(&self) -> &str {
        &self.query_id
    }

    /// Caller-managed query definition version.
    #[must_use]
    pub fn query_version(&self) -> &str {
        &self.query_version
    }

    /// Language the query was registered for.
    #[must_use]
    pub const fn language(&self) -> LanguageId {
        self.language
    }

    /// Provider that supplied the grammar.
    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Provider version that supplied the grammar.
    #[must_use]
    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    /// The native compiled Tree-sitter query.
    #[must_use]
    pub fn native_query(&self) -> &Query {
        &self.query
    }

    /// Capture names in the order Tree-sitter assigned their indices.
    #[must_use]
    pub fn capture_names(&self) -> &[&str] {
        self.query.capture_names()
    }

    /// Resolve a capture index to its declared name.
    #[must_use]
    pub fn capture_name(&self, capture_index: u32) -> Option<&str> {
        self.query
            .capture_names()
            .get(capture_index as usize)
            .copied()
    }

    /// Number of patterns in the query.
    #[must_use]
    pub fn pattern_count(&self) -> usize {
        self.query.pattern_count()
    }
}

/// Setup-time registry of compiled native Tree-sitter queries.
///
/// Registration requires exclusive mutable access. Lookup and execution only
/// need a shared reference, so a registry can be shared across concurrent
/// executions after setup. The registry never stores a [`QueryCursor`].
#[derive(Debug, Default)]
pub struct QueryRegistry {
    queries: Vec<RegisteredQuery>,
    by_id: std::collections::HashMap<String, usize>,
}

impl QueryRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile and register a query against the provider's native grammar.
    ///
    /// Query source is compiled here and not retained. Identities are
    /// validated before duplication, grammar availability, or compilation is
    /// checked, so every rejection is deterministic.
    pub fn register(
        &mut self,
        provider: &dyn LanguageProvider,
        query_id: impl Into<String>,
        query_version: impl Into<String>,
        source: &str,
    ) -> Result<(), QueryRegistrationError> {
        let query_id = query_id.into();
        let query_version = query_version.into();

        if !valid_query_identity(&query_id, MAX_QUERY_ID_BYTES) {
            return Err(QueryRegistrationError::InvalidQueryId);
        }
        if !valid_query_identity(&query_version, MAX_QUERY_VERSION_BYTES) {
            return Err(QueryRegistrationError::InvalidQueryVersion);
        }
        if self.by_id.contains_key(&query_id) {
            return Err(QueryRegistrationError::DuplicateQueryId { query_id });
        }

        let Some(language) = provider.tree_sitter_language() else {
            return Err(QueryRegistrationError::MissingGrammar {
                provider_id: provider.provider_id().to_owned(),
            });
        };

        let query =
            Query::new(&language, source).map_err(|error| QueryRegistrationError::Compile {
                query_id: query_id.clone(),
                kind: error.kind.into(),
                row: error.row,
                column: error.column,
                offset: error.offset,
                message: error.message,
            })?;

        let registered = RegisteredQuery {
            query_id: query_id.clone(),
            query_version,
            language: provider.language(),
            provider_id: provider.provider_id().to_owned(),
            provider_version: provider.provider_version().to_owned(),
            query: Arc::new(query),
        };

        self.by_id.insert(query_id, self.queries.len());
        self.queries.push(registered);
        Ok(())
    }

    /// Look up a registered query by identifier.
    #[must_use]
    pub fn get(&self, query_id: &str) -> Option<&RegisteredQuery> {
        self.by_id
            .get(query_id)
            .and_then(|index| self.queries.get(*index))
    }

    /// Whether an identifier is registered.
    #[must_use]
    pub fn contains(&self, query_id: &str) -> bool {
        self.by_id.contains_key(query_id)
    }

    /// Number of registered queries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queries.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queries.is_empty()
    }

    /// Iterate over registered queries in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &RegisteredQuery> {
        self.queries.iter()
    }
}

/// Cooperative cancellation and deadline control shared with a workspace scan.
///
/// Query execution never creates its own timeout. The supplied budget is the
/// same authority used by scanning and parsing, and it is observed both before
/// execution and from Tree-sitter's cursor progress callback.
pub struct QueryControl<'a> {
    budget: &'a ScanBudget,
}

impl<'a> QueryControl<'a> {
    /// Bind query execution to the shared workspace budget.
    #[must_use]
    pub const fn new(budget: &'a ScanBudget) -> Self {
        Self { budget }
    }

    /// Check cancellation and the shared workspace deadline.
    pub fn check(&self) -> Result<(), ScanBudgetError> {
        self.budget.check()
    }
}

/// Tree-sitter range-restriction mode.
///
/// The two modes are not interchangeable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryRangeMode {
    /// Restrict where the cursor executes, following Tree-sitter's
    /// `set_byte_range` semantics. A returned match is not required to be
    /// fully contained in the range.
    ByteRange,
    /// Require every node of a returned match to be fully contained in the
    /// range, following Tree-sitter's `set_containing_byte_range` semantics.
    ContainingByteRange,
}

/// A validated, optional byte-range restriction for query execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryRange {
    mode: QueryRangeMode,
    start_byte: usize,
    end_byte: usize,
}

impl QueryRange {
    /// Restrict execution to a byte range using `set_byte_range` semantics.
    ///
    /// Returns `None` when the range is not a valid half-open range for
    /// `source_len` or cannot be represented by the pinned Tree-sitter
    /// binding.
    #[must_use]
    pub fn byte_range(start_byte: usize, end_byte: usize, source_len: usize) -> Option<Self> {
        Self::new(QueryRangeMode::ByteRange, start_byte, end_byte, source_len)
    }

    /// Require matches to be fully contained in a byte range.
    ///
    /// Returns `None` under the same conditions as [`Self::byte_range`].
    #[must_use]
    pub fn containing_byte_range(
        start_byte: usize,
        end_byte: usize,
        source_len: usize,
    ) -> Option<Self> {
        Self::new(
            QueryRangeMode::ContainingByteRange,
            start_byte,
            end_byte,
            source_len,
        )
    }

    fn new(
        mode: QueryRangeMode,
        start_byte: usize,
        end_byte: usize,
        source_len: usize,
    ) -> Option<Self> {
        SourceRange::new(start_byte, end_byte, source_len)?;
        // The pinned binding narrows byte offsets to u32.
        u32::try_from(end_byte).ok()?;
        Some(Self {
            mode,
            start_byte,
            end_byte,
        })
    }

    /// Range-restriction mode.
    #[must_use]
    pub const fn mode(&self) -> QueryRangeMode {
        self.mode
    }

    /// First byte of the range, inclusive.
    #[must_use]
    pub const fn start_byte(&self) -> usize {
        self.start_byte
    }

    /// First byte after the range.
    #[must_use]
    pub const fn end_byte(&self) -> usize {
        self.end_byte
    }

    fn fits(&self, source_len: usize) -> bool {
        SourceRange::new(self.start_byte, self.end_byte, source_len).is_some()
            && u32::try_from(self.end_byte).is_ok()
    }
}

/// Bounds for one query execution.
///
/// `max_matches` and `max_captures` bound retained output. They are enforced
/// while streaming and are not substitutes for Tree-sitter's in-progress
/// match limit, which bounds the cursor's match state and is configured with
/// `match_limit`.
///
/// # Hard ceilings
///
/// Retained-output limits are caller-configurable between 1 and documented
/// absolute maximums: [`MAX_RETAINED_MATCHES`] for `max_matches` and
/// [`MAX_RETAINED_CAPTURES`] for `max_captures`. Values of 0 or above the
/// ceiling are rejected with [`QueryExecutionError::InvalidOutputLimit`]
/// before a cursor is created; nothing is clamped silently. These ceilings
/// bound *retained result records*, not native memory: Tree-sitter cursor
/// state, the parse tree, and the source snapshot are outside this contract,
/// so output ceilings do not constitute a hard native-memory or RSS sandbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryExecutionLimits {
    /// Maximum match records retained; 1 through [`MAX_RETAINED_MATCHES`].
    pub max_matches: usize,
    /// Maximum capture records retained across all matches; 1 through
    /// [`MAX_RETAINED_CAPTURES`].
    pub max_captures: usize,
    /// Tree-sitter in-progress match limit; must be 1 through 65,536.
    pub match_limit: u32,
}

impl QueryExecutionLimits {
    /// Validate every limit against its documented range.
    ///
    /// Returns the first violated field. Zero values are invalid because a
    /// caller that wants no output should not execute the query at all.
    pub fn validate(&self) -> Result<(), QueryExecutionError> {
        if self.max_matches == 0 || self.max_matches > MAX_RETAINED_MATCHES {
            return Err(QueryExecutionError::InvalidOutputLimit {
                field: QueryOutputLimitField::MaxMatches,
                value: self.max_matches,
                max: MAX_RETAINED_MATCHES,
            });
        }
        if self.max_captures == 0 || self.max_captures > MAX_RETAINED_CAPTURES {
            return Err(QueryExecutionError::InvalidOutputLimit {
                field: QueryOutputLimitField::MaxCaptures,
                value: self.max_captures,
                max: MAX_RETAINED_CAPTURES,
            });
        }
        if !(MIN_MATCH_LIMIT..=MAX_MATCH_LIMIT).contains(&self.match_limit) {
            return Err(QueryExecutionError::InvalidMatchLimit(self.match_limit));
        }
        Ok(())
    }
}

impl Default for QueryExecutionLimits {
    fn default() -> Self {
        Self {
            max_matches: 10_000,
            max_captures: 100_000,
            match_limit: MAX_MATCH_LIMIT,
        }
    }
}

/// Which output-limit field rejected a configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryOutputLimitField {
    /// `max_matches` was 0 or above [`MAX_RETAINED_MATCHES`].
    MaxMatches,
    /// `max_captures` was 0 or above [`MAX_RETAINED_CAPTURES`].
    MaxCaptures,
}

/// Options for one query execution.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryExecutionOptions {
    /// Output and in-progress match limits.
    pub limits: QueryExecutionLimits,
    /// Optional byte-range restriction.
    pub range: Option<QueryRange>,
}

/// Which retained-output cap stopped execution.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum QueryOutputLimit {
    /// The retained match cap was reached.
    Matches,
    /// The retained capture cap was reached.
    Captures,
}

/// Coverage state of one query execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryExecutionStatus {
    /// The cursor was consumed to exhaustion and no limit applied.
    Complete,
    /// The shared budget was cancelled during execution; results are partial.
    Cancelled,
    /// The shared workspace deadline expired during execution; results are partial.
    DeadlineExceeded,
    /// A retained-output cap was reached; results are partial.
    OutputLimited(QueryOutputLimit),
    /// Tree-sitter exhausted its in-progress match limit; results are partial.
    MatchLimitExceeded,
}

/// One captured node, identified by capture index and source byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryCaptureRecord {
    /// Capture index, resolvable through [`RegisteredQuery::capture_name`].
    pub capture_index: u32,
    /// Exact half-open byte range in the document source.
    pub range: SourceRange,
}

/// One native query match with its bounded capture records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryMatchRecord {
    /// Zero-based index of the query pattern that matched.
    pub pattern_index: usize,
    /// Captures retained for this match, in Tree-sitter order.
    pub captures: Vec<QueryCaptureRecord>,
}

/// Bounded result of one query execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryExecution {
    /// Coverage state; anything other than [`QueryExecutionStatus::Complete`]
    /// means the retained matches are partial.
    pub status: QueryExecutionStatus,
    /// Retained generic match records.
    pub matches: Vec<QueryMatchRecord>,
    /// Number of retained capture records across all matches.
    pub total_captures: usize,
    /// Parse coverage of the queried document.
    ///
    /// A recovered document remains queryable. This field preserves its
    /// recovered status instead of presenting the query result as if the
    /// document had parsed cleanly.
    pub document_status: ParsedDocumentStatus,
}

impl QueryExecution {
    /// Whether execution ran to exhaustion without cancellation or limits.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self.status, QueryExecutionStatus::Complete)
    }
}

/// Why a registered query cannot run against a document.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum QueryCompatibilityError {
    /// The document was parsed as a different language.
    #[error("query targets language {expected:?}, but the document was parsed as {actual:?}")]
    Language {
        /// Language recorded at registration.
        expected: LanguageId,
        /// Language of the parsed document.
        actual: LanguageId,
    },
    /// The document was parsed by a different provider.
    #[error("query targets provider `{expected}`, but the document was parsed by `{actual}`")]
    ProviderId {
        /// Provider recorded at registration.
        expected: String,
        /// Provider that produced the document.
        actual: String,
    },
    /// The document was parsed by a different provider version.
    #[error(
        "query targets provider version `{expected}`, but the document was parsed by version `{actual}`"
    )]
    ProviderVersion {
        /// Provider version recorded at registration.
        expected: String,
        /// Provider version that produced the document.
        actual: String,
    },
}

/// Reasons query execution cannot start.
///
/// Cancellation or deadline expiry *during* execution is not an error: it is
/// reported through [`QueryExecutionStatus`] with partial matches retained.
#[derive(Debug, thiserror::Error)]
pub enum QueryExecutionError {
    /// The registered query and the parsed document are incompatible.
    #[error("query `{query_id}` is incompatible with the parsed document: {reason}")]
    IncompatibleDocument {
        /// Identifier of the rejected query.
        query_id: String,
        /// Specific compatibility mismatch.
        reason: QueryCompatibilityError,
    },
    /// The Tree-sitter match limit is outside the binding's supported range.
    #[error("invalid Tree-sitter match limit {0}: expected 1 through 65,536")]
    InvalidMatchLimit(u32),
    /// A retained-output limit is outside its documented range.
    ///
    /// Rejected before the cursor is created; nothing is clamped.
    #[error("invalid output limit {field:?} = {value}: expected 1 through {max}")]
    InvalidOutputLimit {
        /// The rejected field.
        field: QueryOutputLimitField,
        /// Supplied value.
        value: usize,
        /// Documented absolute maximum for the field.
        max: usize,
    },
    /// The range is not valid for this document's source snapshot.
    #[error("query byte range {start}..{end} is invalid for a document of {source_len} bytes")]
    InvalidRange {
        /// Requested start byte.
        start: usize,
        /// Requested end byte.
        end: usize,
        /// Document source length.
        source_len: usize,
    },
    /// The shared budget rejected execution before the cursor ran.
    #[error("query execution rejected by the shared budget: {0}")]
    Budget(#[from] ScanBudgetError),
}

fn ensure_compatible(
    registered: &RegisteredQuery,
    document: &ParsedDocument,
) -> Result<(), QueryExecutionError> {
    if registered.language != document.language() {
        return Err(QueryExecutionError::IncompatibleDocument {
            query_id: registered.query_id.clone(),
            reason: QueryCompatibilityError::Language {
                expected: registered.language,
                actual: document.language(),
            },
        });
    }
    if registered.provider_id != document.provider_id() {
        return Err(QueryExecutionError::IncompatibleDocument {
            query_id: registered.query_id.clone(),
            reason: QueryCompatibilityError::ProviderId {
                expected: registered.provider_id.clone(),
                actual: document.provider_id().to_owned(),
            },
        });
    }
    if registered.provider_version != document.provider_version() {
        return Err(QueryExecutionError::IncompatibleDocument {
            query_id: registered.query_id.clone(),
            reason: QueryCompatibilityError::ProviderVersion {
                expected: registered.provider_version.clone(),
                actual: document.provider_version().to_owned(),
            },
        });
    }
    Ok(())
}

/// Execute a registered native query against a parsed document.
///
/// A fresh [`tree_sitter::QueryCursor`] is created for every call, so
/// independent executions may run concurrently over shared immutable
/// documents and registered queries. The document is validated for language,
/// provider, and provider-version compatibility before the cursor exists.
///
/// Capture records contain only generic metadata and byte ranges. They never
/// own or clone Tree-sitter nodes, and they are valid only against the
/// document's source snapshot.
///
/// # Errors
///
/// Returns [`QueryExecutionError`] when compatibility, limits, or the range
/// are invalid, or when the shared budget already rejects execution.
pub fn execute_query(
    registered: &RegisteredQuery,
    document: &ParsedDocument,
    control: &QueryControl<'_>,
    options: &QueryExecutionOptions,
) -> Result<QueryExecution, QueryExecutionError> {
    ensure_compatible(registered, document)?;

    let limits = options.limits;
    limits.validate()?;

    let source = document.source();
    let source_len = source.len();
    if let Some(range) = options.range
        && !range.fits(source_len)
    {
        return Err(QueryExecutionError::InvalidRange {
            start: range.start_byte,
            end: range.end_byte,
            source_len,
        });
    }

    control.check()?;

    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(limits.match_limit);
    if let Some(range) = options.range {
        let bytes = range.start_byte..range.end_byte;
        match range.mode {
            QueryRangeMode::ByteRange => {
                cursor.set_byte_range(bytes);
            }
            QueryRangeMode::ContainingByteRange => {
                cursor.set_containing_byte_range(bytes);
            }
        }
    }

    let mut budget_error: Option<ScanBudgetError> = None;
    let mut matches: Vec<QueryMatchRecord> = Vec::new();
    let mut total_captures = 0usize;
    let mut output_limit: Option<QueryOutputLimit> = None;

    {
        let mut progress = |_: &QueryCursorState| match control.check() {
            Ok(()) => ControlFlow::Continue(()),
            Err(error) => {
                budget_error = Some(error);
                ControlFlow::Break(())
            }
        };
        let cursor_options = QueryCursorOptions::new().progress_callback(&mut progress);
        let mut stream = cursor.matches_with_options(
            registered.native_query(),
            document.root_node(),
            source,
            cursor_options,
        );

        'stream: while let Some(query_match) = stream.next() {
            if matches.len() >= limits.max_matches {
                output_limit = Some(QueryOutputLimit::Matches);
                break;
            }

            let mut captures = Vec::with_capacity(query_match.captures.len());
            for capture in query_match.captures {
                if total_captures >= limits.max_captures {
                    output_limit = Some(QueryOutputLimit::Captures);
                    break 'stream;
                }
                let node = capture.node.byte_range();
                let Some(range) = SourceRange::new(node.start, node.end, source_len) else {
                    // The document source is exactly the snapshot Tree-sitter
                    // parsed, so an out-of-bounds node would indicate a broken
                    // invariant. Do not invent a range for it.
                    continue;
                };
                captures.push(QueryCaptureRecord {
                    capture_index: capture.index,
                    range,
                });
                total_captures = total_captures.saturating_add(1);
            }

            matches.push(QueryMatchRecord {
                pattern_index: query_match.pattern_index,
                captures,
            });
        }
    }

    let match_limit_exceeded = cursor.did_exceed_match_limit();

    let status = if let Some(error) = budget_error {
        match error {
            ScanBudgetError::DeadlineExceeded => QueryExecutionStatus::DeadlineExceeded,
            _ => QueryExecutionStatus::Cancelled,
        }
    } else if match_limit_exceeded {
        QueryExecutionStatus::MatchLimitExceeded
    } else if let Some(limit) = output_limit {
        QueryExecutionStatus::OutputLimited(limit)
    } else {
        QueryExecutionStatus::Complete
    };

    Ok(QueryExecution {
        status,
        matches,
        total_captures,
        document_status: document.status(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use tree_sitter::Language;

    use crate::classification::LanguageId;
    use crate::syntax::rust::RustProvider;
    use crate::{ParseInput, ParseResult, ProviderError, ScanLimits, WorkspaceId};

    use super::*;

    fn parse_document(source: &str) -> ParsedDocument {
        RustProvider
            .parse_document(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/lib.rs"),
                source: source.as_bytes(),
            })
            .expect("parse document")
    }

    fn budget() -> ScanBudget {
        ScanBudget::new(ScanLimits::default())
    }

    fn registry_with(query_id: &str, query_version: &str, source: &str) -> QueryRegistry {
        let mut registry = QueryRegistry::new();
        registry
            .register(&RustProvider, query_id, query_version, source)
            .expect("register query");
        registry
    }

    fn execute(
        registry: &QueryRegistry,
        query_id: &str,
        document: &ParsedDocument,
        options: &QueryExecutionOptions,
    ) -> QueryExecution {
        let budget = budget();
        execute_query(
            registry.get(query_id).expect("registered query"),
            document,
            &QueryControl::new(&budget),
            options,
        )
        .expect("query execution")
    }

    fn options() -> QueryExecutionOptions {
        QueryExecutionOptions::default()
    }

    fn capture_text<'a>(source: &'a str, capture: &QueryCaptureRecord) -> &'a str {
        &source[capture.range.start_byte..capture.range.end_byte]
    }

    struct NoGrammarProvider;

    impl LanguageProvider for NoGrammarProvider {
        fn language(&self) -> LanguageId {
            LanguageId::Rust
        }

        fn provider_id(&self) -> &'static str {
            "test.no-grammar"
        }

        fn provider_version(&self) -> &'static str {
            "1"
        }

        fn parse(&self, _input: ParseInput<'_>) -> Result<ParseResult, ProviderError> {
            Err(ProviderError::ParserInitialization(
                "test provider does not parse".to_owned(),
            ))
        }
    }

    struct AliasedProvider {
        id: &'static str,
        version: &'static str,
        language: LanguageId,
    }

    impl LanguageProvider for AliasedProvider {
        fn language(&self) -> LanguageId {
            self.language
        }

        fn provider_id(&self) -> &'static str {
            self.id
        }

        fn provider_version(&self) -> &'static str {
            self.version
        }

        fn tree_sitter_language(&self) -> Option<Language> {
            Some(tree_sitter_rust::LANGUAGE.into())
        }

        fn parse(&self, _input: ParseInput<'_>) -> Result<ParseResult, ProviderError> {
            Err(ProviderError::ParserInitialization(
                "test provider does not parse".to_owned(),
            ))
        }
    }

    fn large_rust_source(functions: usize) -> String {
        let mut source = String::new();
        for index in 0..functions {
            source.push_str(&format!(
                "fn helper_{index}() {{ let value_{index} = {index} + 1; }}\n"
            ));
        }
        source
    }

    #[test]
    fn registry_compiles_against_provider_grammar_and_exposes_metadata() {
        let registry = registry_with(
            "rust.functions.v1",
            "1",
            "(function_item name: (identifier) @name)",
        );
        let registered = registry.get("rust.functions.v1").expect("registered query");

        assert_eq!(registered.query_id(), "rust.functions.v1");
        assert_eq!(registered.query_version(), "1");
        assert_eq!(registered.language(), LanguageId::Rust);
        assert_eq!(registered.provider_id(), RustProvider::ID);
        assert_eq!(registered.provider_version(), RustProvider::VERSION);
        assert_eq!(registered.pattern_count(), 1);
        assert!(registered.capture_names().contains(&"name"));
        assert_eq!(registered.capture_name(0), Some("name"));
        assert_eq!(registered.capture_name(9), None);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn registry_rejects_invalid_query_source_with_compile_location() {
        let mut registry = QueryRegistry::new();
        let error = registry
            .register(&RustProvider, "rust.broken.v1", "1", "(function_item")
            .expect_err("invalid query source");

        match error {
            QueryRegistrationError::Compile {
                query_id,
                kind,
                row,
                column,
                offset,
                message,
            } => {
                assert_eq!(query_id, "rust.broken.v1");
                assert_eq!(kind, QueryCompileErrorKind::Syntax);
                assert_eq!(row, 0);
                assert_eq!(offset, "(function_item".len());
                assert_eq!(column, offset);
                assert!(!message.is_empty());
            }
            other => panic!("unexpected registration error: {other}"),
        }
        assert!(registry.is_empty());
        assert!(registry.get("rust.broken.v1").is_none());
    }

    #[test]
    fn registry_rejects_unknown_node_kinds() {
        let mut registry = QueryRegistry::new();
        let error = registry
            .register(
                &RustProvider,
                "rust.unknown.v1",
                "1",
                "(not_a_rust_node) @x",
            )
            .expect_err("unknown node kind");

        assert!(matches!(
            error,
            QueryRegistrationError::Compile {
                kind: QueryCompileErrorKind::NodeType,
                ..
            }
        ));
    }

    #[test]
    fn registry_rejects_providers_without_a_grammar() {
        let mut registry = QueryRegistry::new();
        let error = registry
            .register(&NoGrammarProvider, "any.query.v1", "1", "(identifier) @id")
            .expect_err("provider has no grammar");

        match error {
            QueryRegistrationError::MissingGrammar { provider_id } => {
                assert_eq!(provider_id, "test.no-grammar");
            }
            other => panic!("unexpected registration error: {other}"),
        }
        assert!(registry.is_empty());
    }

    #[test]
    fn registry_rejects_invalid_identities() {
        let mut registry = QueryRegistry::new();
        let long_id = "a".repeat(MAX_QUERY_ID_BYTES + 1);
        for query_id in [
            "",
            " leading",
            "with space",
            "-dash",
            ".dot",
            "^caret",
            long_id.as_str(),
        ] {
            assert!(
                matches!(
                    registry.register(&RustProvider, query_id, "1", "(identifier) @id"),
                    Err(QueryRegistrationError::InvalidQueryId)
                ),
                "query id {query_id:?} must be rejected"
            );
        }

        let long_version = "v".repeat(MAX_QUERY_VERSION_BYTES + 1);
        for query_version in ["", " 1", "1 beta", "-1", ".1", long_version.as_str()] {
            assert!(
                matches!(
                    registry.register(
                        &RustProvider,
                        "rust.valid-id.v1",
                        query_version,
                        "(identifier) @id"
                    ),
                    Err(QueryRegistrationError::InvalidQueryVersion)
                ),
                "query version {query_version:?} must be rejected"
            );
        }
        assert!(registry.is_empty());
    }

    #[test]
    fn registry_rejects_duplicate_ids_without_replacing_the_original() {
        let mut registry = QueryRegistry::new();
        registry
            .register(
                &RustProvider,
                "rust.duplicate.v1",
                "1",
                "(identifier) @first",
            )
            .expect("first registration");
        let error = registry
            .register(
                &RustProvider,
                "rust.duplicate.v1",
                "2",
                "(identifier) @second",
            )
            .expect_err("duplicate id");

        match error {
            QueryRegistrationError::DuplicateQueryId { query_id } => {
                assert_eq!(query_id, "rust.duplicate.v1");
            }
            other => panic!("unexpected registration error: {other}"),
        }

        let registered = registry.get("rust.duplicate.v1").expect("original query");
        assert_eq!(registered.query_version(), "1");
        assert!(registered.capture_names().contains(&"first"));
        assert!(!registered.capture_names().contains(&"second"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registry_holds_multiple_queries_in_registration_order() {
        let mut registry = QueryRegistry::new();
        registry
            .register(&RustProvider, "b.structs.v1", "2", "(struct_item) @item")
            .expect("struct query");
        registry
            .register(
                &RustProvider,
                "a.functions.v1",
                "1",
                "(function_item) @item",
            )
            .expect("function query");

        let ids: Vec<&str> = registry.iter().map(RegisteredQuery::query_id).collect();
        assert_eq!(ids, vec!["b.structs.v1", "a.functions.v1"]);
        assert!(registry.contains("a.functions.v1"));
        assert!(!registry.contains("c.missing.v1"));
    }

    #[test]
    fn execution_returns_capture_names_and_exact_byte_ranges() {
        let source = "fn alpha() {}\nfn beta() {}\n";
        let document = parse_document(source);
        let registry = registry_with(
            "rust.functions.v1",
            "1",
            "(function_item name: (identifier) @name) @function",
        );
        let registered = registry.get("rust.functions.v1").expect("registered query");
        let name_index = registered
            .native_query()
            .capture_index_for_name("name")
            .expect("name capture");
        let function_index = registered
            .native_query()
            .capture_index_for_name("function")
            .expect("function capture");

        assert_eq!(registered.capture_name(name_index), Some("name"));
        assert_eq!(registered.capture_name(function_index), Some("function"));

        let execution = execute(&registry, "rust.functions.v1", &document, &options());
        assert!(execution.is_complete());
        assert_eq!(execution.status, QueryExecutionStatus::Complete);
        assert_eq!(execution.document_status, ParsedDocumentStatus::Complete);
        assert_eq!(execution.matches.len(), 2);
        assert_eq!(execution.total_captures, 4);

        let expected_names = ["alpha", "beta"];
        let expected_functions = ["fn alpha() {}", "fn beta() {}"];
        for (match_record, (name, function)) in execution
            .matches
            .iter()
            .zip(expected_names.iter().zip(expected_functions.iter()))
        {
            assert_eq!(match_record.pattern_index, 0);
            assert_eq!(match_record.captures.len(), 2);
            let name_capture = match_record
                .captures
                .iter()
                .find(|capture| capture.capture_index == name_index)
                .expect("name capture");
            let function_capture = match_record
                .captures
                .iter()
                .find(|capture| capture.capture_index == function_index)
                .expect("function capture");
            assert_eq!(capture_text(source, name_capture), *name);
            assert_eq!(capture_text(source, function_capture), *function);
        }
    }

    #[test]
    fn execution_rejects_incompatible_provider_metadata_before_running() {
        let document = parse_document("fn alpha() {}");
        let budget = budget();

        let mut provider_id_registry = QueryRegistry::new();
        provider_id_registry
            .register(
                &AliasedProvider {
                    id: "test.other-provider",
                    version: RustProvider::VERSION,
                    language: LanguageId::Rust,
                },
                "rust.functions.v1",
                "1",
                "(function_item) @f",
            )
            .expect("register provider-id mismatch query");
        let error = execute_query(
            provider_id_registry
                .get("rust.functions.v1")
                .expect("query"),
            &document,
            &QueryControl::new(&budget),
            &options(),
        )
        .expect_err("provider id mismatch");
        assert!(matches!(
            error,
            QueryExecutionError::IncompatibleDocument {
                reason: QueryCompatibilityError::ProviderId { .. },
                ..
            }
        ));

        let mut version_registry = QueryRegistry::new();
        version_registry
            .register(
                &AliasedProvider {
                    id: RustProvider::ID,
                    version: "facts-from-another-build",
                    language: LanguageId::Rust,
                },
                "rust.functions.v1",
                "1",
                "(function_item) @f",
            )
            .expect("register version mismatch query");
        let error = execute_query(
            version_registry.get("rust.functions.v1").expect("query"),
            &document,
            &QueryControl::new(&budget),
            &options(),
        )
        .expect_err("provider version mismatch");
        assert!(matches!(
            error,
            QueryExecutionError::IncompatibleDocument {
                reason: QueryCompatibilityError::ProviderVersion { .. },
                ..
            }
        ));

        let mut language_registry = QueryRegistry::new();
        language_registry
            .register(
                &AliasedProvider {
                    id: RustProvider::ID,
                    version: RustProvider::VERSION,
                    language: LanguageId::TypeScript,
                },
                "rust.functions.v1",
                "1",
                "(function_item) @f",
            )
            .expect("register language mismatch query");
        let error = execute_query(
            language_registry.get("rust.functions.v1").expect("query"),
            &document,
            &QueryControl::new(&budget),
            &options(),
        )
        .expect_err("language mismatch");
        assert!(matches!(
            error,
            QueryExecutionError::IncompatibleDocument {
                reason: QueryCompatibilityError::Language { .. },
                ..
            }
        ));
    }

    #[test]
    fn independent_executions_share_registered_queries_concurrently() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<QueryRegistry>();
        assert_send_sync::<ParsedDocument>();
        assert_send_sync::<RegisteredQuery>();

        let registry = registry_with(
            "rust.consts.v1",
            "1",
            "(const_item name: (identifier) @name)",
        );
        let document = parse_document("const ALPHA: u8 = 1;\nconst BETA: u8 = 2;\n");
        let budget = budget();

        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        execute_query(
                            registry.get("rust.consts.v1").expect("registered query"),
                            &document,
                            &QueryControl::new(&budget),
                            &options(),
                        )
                        .expect("concurrent execution")
                    })
                })
                .collect();

            for worker in workers {
                let execution = worker.join().expect("worker thread");
                assert!(execution.is_complete());
                assert_eq!(execution.matches.len(), 2);
            }
        });
    }

    #[test]
    fn execution_stops_at_match_and_capture_caps() {
        let source = "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n";
        let document = parse_document(source);
        let registry = registry_with(
            "rust.functions.v1",
            "1",
            "(function_item name: (identifier) @name)",
        );

        let match_limited = execute(
            &registry,
            "rust.functions.v1",
            &document,
            &QueryExecutionOptions {
                limits: QueryExecutionLimits {
                    max_matches: 2,
                    ..QueryExecutionLimits::default()
                },
                ..QueryExecutionOptions::default()
            },
        );
        assert_eq!(
            match_limited.status,
            QueryExecutionStatus::OutputLimited(QueryOutputLimit::Matches)
        );
        assert_eq!(match_limited.matches.len(), 2);
        assert_eq!(match_limited.total_captures, 2);
        assert!(!match_limited.is_complete());

        let capture_limited = execute(
            &registry,
            "rust.functions.v1",
            &document,
            &QueryExecutionOptions {
                limits: QueryExecutionLimits {
                    max_matches: 100,
                    max_captures: 1,
                    ..QueryExecutionLimits::default()
                },
                ..QueryExecutionOptions::default()
            },
        );
        assert_eq!(
            capture_limited.status,
            QueryExecutionStatus::OutputLimited(QueryOutputLimit::Captures)
        );
        assert_eq!(capture_limited.matches.len(), 1);
        assert_eq!(capture_limited.total_captures, 1);

        // A cap exactly equal to the available output is not exhaustion.
        let exact = execute(
            &registry,
            "rust.functions.v1",
            &document,
            &QueryExecutionOptions {
                limits: QueryExecutionLimits {
                    max_matches: 3,
                    max_captures: 3,
                    ..QueryExecutionLimits::default()
                },
                ..QueryExecutionOptions::default()
            },
        );
        assert!(exact.is_complete());
        assert_eq!(exact.matches.len(), 3);
    }

    #[test]
    fn execution_distinguishes_match_limit_exhaustion() {
        let source = "fn alpha() { let x = 1; let y = 2; let z = 3; }";
        let document = parse_document(source);
        let registry = registry_with(
            "rust.identifiers.v1",
            "1",
            "(identifier) @a\n(identifier) @b\n(identifier) @c\n(identifier) @d",
        );

        let execution = execute(
            &registry,
            "rust.identifiers.v1",
            &document,
            &QueryExecutionOptions {
                limits: QueryExecutionLimits {
                    max_matches: 100_000,
                    max_captures: 100_000,
                    match_limit: 1,
                },
                ..QueryExecutionOptions::default()
            },
        );
        assert_eq!(execution.status, QueryExecutionStatus::MatchLimitExceeded);
        assert!(!execution.is_complete());
    }

    #[test]
    fn execution_rejects_an_already_cancelled_budget() {
        let document = parse_document("fn alpha() {}");
        let registry = registry_with("rust.functions.v1", "1", "(function_item) @f");
        let budget = budget();
        budget.cancel();

        let error = execute_query(
            registry.get("rust.functions.v1").expect("query"),
            &document,
            &QueryControl::new(&budget),
            &options(),
        )
        .expect_err("cancelled budget must prevent execution");

        assert!(matches!(
            error,
            QueryExecutionError::Budget(ScanBudgetError::Cancelled)
        ));
    }

    #[test]
    fn deadline_exhaustion_during_execution_is_not_complete() {
        let source = large_rust_source(60_000);
        let document = parse_document(&source);
        let registry = registry_with("rust.identifiers.v1", "1", "(identifier) @id");
        let options = QueryExecutionOptions {
            limits: QueryExecutionLimits {
                max_matches: 1_000_000,
                max_captures: 1_000_000,
                ..QueryExecutionLimits::default()
            },
            ..QueryExecutionOptions::default()
        };

        let budget = ScanBudget::new(ScanLimits {
            max_duration: Duration::from_millis(3),
            ..ScanLimits::default()
        });
        let outcome = execute_query(
            registry.get("rust.identifiers.v1").expect("query"),
            &document,
            &QueryControl::new(&budget),
            &options,
        );

        match outcome {
            Ok(execution) => {
                assert_eq!(execution.status, QueryExecutionStatus::DeadlineExceeded);
                assert!(!execution.is_complete());
            }
            // The deadline may also expire between budget construction and the
            // pre-execution check on a heavily loaded machine.
            Err(QueryExecutionError::Budget(ScanBudgetError::DeadlineExceeded)) => {}
            Err(error) => panic!("unexpected execution error: {error}"),
        }
        assert_eq!(document.status(), ParsedDocumentStatus::Complete);
    }

    #[test]
    fn cancellation_during_execution_is_observed_and_not_complete() {
        let source = large_rust_source(60_000);
        let document = parse_document(&source);
        let registry = registry_with("rust.identifiers.v1", "1", "(identifier) @id");
        let options = QueryExecutionOptions {
            limits: QueryExecutionLimits {
                max_matches: 1_000_000,
                max_captures: 1_000_000,
                ..QueryExecutionLimits::default()
            },
            ..QueryExecutionOptions::default()
        };

        let budget = budget();
        let canceller = budget.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(3));
            canceller.cancel();
        });
        let execution = execute_query(
            registry.get("rust.identifiers.v1").expect("query"),
            &document,
            &QueryControl::new(&budget),
            &options,
        )
        .expect("execution observes cancellation");
        handle.join().expect("canceller thread");

        assert_eq!(execution.status, QueryExecutionStatus::Cancelled);
        assert!(!execution.is_complete());
    }

    #[test]
    fn byte_range_and_containing_range_have_distinct_semantics() {
        let source = "fn alpha() {}\nfn beta() {}\n";
        let document = parse_document(source);
        let registry = registry_with(
            "rust.functions.v1",
            "1",
            "(function_item name: (identifier) @name) @function",
        );

        let name_start = source.find("alpha").expect("alpha name");
        let name_end = name_start + "alpha".len();

        let overlap = execute(
            &registry,
            "rust.functions.v1",
            &document,
            &QueryExecutionOptions {
                range: QueryRange::byte_range(name_start, name_end, source.len()),
                ..QueryExecutionOptions::default()
            },
        );
        assert!(overlap.is_complete());
        assert_eq!(overlap.matches.len(), 1);

        let contained = execute(
            &registry,
            "rust.functions.v1",
            &document,
            &QueryExecutionOptions {
                range: QueryRange::containing_byte_range(name_start, name_end, source.len()),
                ..QueryExecutionOptions::default()
            },
        );
        assert!(contained.is_complete());
        assert!(contained.matches.is_empty());
    }

    #[test]
    fn execution_rejects_invalid_ranges() {
        assert!(QueryRange::byte_range(5, 2, 10).is_none());
        assert!(QueryRange::byte_range(0, 11, 10).is_none());
        assert!(QueryRange::containing_byte_range(0, 11, 10).is_none());
        assert!(QueryRange::byte_range(0, 10, 10).is_some());

        let document = parse_document("fn alpha() {}");
        let registry = registry_with("rust.functions.v1", "1", "(function_item) @f");
        // Build a range valid for a larger source, then run it against a
        // shorter document.
        let range = QueryRange::byte_range(0, 100, 1_000).expect("range for larger source");
        let budget = budget();
        let error = execute_query(
            registry.get("rust.functions.v1").expect("query"),
            &document,
            &QueryControl::new(&budget),
            &QueryExecutionOptions {
                range: Some(range),
                ..QueryExecutionOptions::default()
            },
        )
        .expect_err("range outside document");
        assert!(matches!(error, QueryExecutionError::InvalidRange { .. }));
    }

    #[test]
    fn recovered_documents_remain_queryable_without_status_upgrade() {
        let document = parse_document("fn good() {}\nfn broken( {");
        assert_eq!(document.status(), ParsedDocumentStatus::Recovered);
        assert!(document.root_node().has_error());

        let registry = registry_with(
            "rust.functions.v1",
            "1",
            "(function_item name: (identifier) @name)",
        );
        let execution = execute(&registry, "rust.functions.v1", &document, &options());
        assert!(execution.is_complete());
        assert_eq!(execution.document_status, ParsedDocumentStatus::Recovered);
        assert!(
            execution
                .matches
                .iter()
                .any(|record| record.captures.iter().any(|capture| {
                    &document.source()[capture.range.start_byte..capture.range.end_byte] == b"good"
                }))
        );
    }

    #[test]
    fn execution_rejects_invalid_match_limits() {
        let document = parse_document("fn alpha() {}");
        let registry = registry_with("rust.functions.v1", "1", "(function_item) @f");
        let budget = budget();

        for match_limit in [0, MAX_MATCH_LIMIT + 1] {
            let error = execute_query(
                registry.get("rust.functions.v1").expect("query"),
                &document,
                &QueryControl::new(&budget),
                &QueryExecutionOptions {
                    limits: QueryExecutionLimits {
                        match_limit,
                        ..QueryExecutionLimits::default()
                    },
                    ..QueryExecutionOptions::default()
                },
            )
            .expect_err("invalid match limit");
            assert!(matches!(
                error,
                QueryExecutionError::InvalidMatchLimit(value) if value == match_limit
            ));
        }
    }
}
