//! Unified Document / Chunk / Symbol / Memory data model.
//!
//! Retrieval in this crate addresses four unit kinds through one identity
//! scheme. Source-derived units (document, chunk, symbol) are keyed by
//! workspace-relative path plus a monotonic per-document revision and the
//! fingerprint of the exact source bytes they were derived from. Memory units
//! are keyed by an opaque UUID and carry their own provenance edge back to a
//! source range when they were written from code.
//!
//! Nothing here stores source text. Chunks and symbols retain byte ranges
//! against the fingerprinted snapshot, so a consumer resolves text through the
//! workspace capability and can verify it against the fingerprint.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::{
    WorkspaceId, classification::LanguageId, evidence::SourceFingerprint, resolution::SymbolKind,
    scope::Acl, syntax::SourceRange,
};

/// Workspace-relative identity of one source document.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DocumentId {
    /// Host-supplied checkout identity.
    pub workspace_id: WorkspaceId,
    /// Workspace-relative path.
    pub path: PathBuf,
}

impl DocumentId {
    /// Identity of `path` inside `workspace_id`.
    #[must_use]
    pub fn new(workspace_id: WorkspaceId, path: impl Into<PathBuf>) -> Self {
        Self {
            workspace_id,
            path: path.into(),
        }
    }

    /// Workspace-relative path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.workspace_id, self.path.display())
    }
}

/// Monotonic per-document content revision.
///
/// A new revision is assigned every time the document's bytes change, so
/// evidence produced under an older revision can be told apart from current
/// evidence without re-reading the source.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Revision(pub u64);

impl Revision {
    /// Successor of this revision.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Source-backed location of a unit.
///
/// This is the provenance edge every retrieved result carries: which document,
/// which exact byte snapshot, and where inside it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Provenance {
    /// Document the unit was derived from.
    pub document: DocumentId,
    /// Fingerprint of the exact source bytes used.
    pub fingerprint: SourceFingerprint,
    /// Half-open byte range in that snapshot.
    pub range: SourceRange,
    /// Provider and version that parsed the snapshot, when applicable.
    pub provider: Option<String>,
}

impl Provenance {
    /// Provenance for a range of one fingerprinted snapshot.
    #[must_use]
    pub fn new(document: DocumentId, fingerprint: SourceFingerprint, range: SourceRange) -> Self {
        Self {
            document,
            fingerprint,
            range,
            provider: None,
        }
    }

    /// Attach the parsing provider identity.
    #[must_use]
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }
}

/// Published metadata for one source document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentRecord {
    /// Document identity.
    pub id: DocumentId,
    /// Classified language.
    pub language: LanguageId,
    /// Fingerprint of the current bytes.
    pub fingerprint: SourceFingerprint,
    /// Current content revision.
    pub revision: Revision,
    /// Number of indexed chunks.
    pub chunks: u32,
    /// Number of indexed symbols.
    pub symbols: u32,
    /// Bytes in the fingerprinted snapshot.
    pub bytes: usize,
}

impl DocumentRecord {
    /// Provenance covering the whole document.
    #[must_use]
    pub fn provenance(&self) -> Provenance {
        Provenance::new(
            self.id.clone(),
            self.fingerprint,
            SourceRange {
                start_byte: 0,
                end_byte: self.fingerprint.source_len(),
            },
        )
    }
}

/// Identity of one chunk inside a document.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkId {
    /// Owning document.
    pub document: DocumentId,
    /// Zero-based chunk ordinal within the document revision.
    pub ordinal: u32,
}

impl ChunkId {
    /// Chunk `ordinal` of `document`.
    #[must_use]
    pub fn new(document: DocumentId, ordinal: u32) -> Self {
        Self { document, ordinal }
    }
}

/// One indexed source chunk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkRecord {
    /// Chunk identity.
    pub id: ChunkId,
    /// Byte range in the fingerprinted snapshot.
    pub range: SourceRange,
    /// Revision the chunk was produced from.
    pub revision: Revision,
    /// Symbol this chunk was anchored to, when it came from a declaration.
    pub symbol: Option<SymbolUnitId>,
}

impl ChunkRecord {
    /// Provenance for this chunk.
    #[must_use]
    pub fn provenance(&self, fingerprint: SourceFingerprint) -> Provenance {
        Provenance::new(self.id.document.clone(), fingerprint, self.range)
    }
}

/// Identity of one indexed symbol, unique across kinds.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolUnitId {
    /// Owning document.
    pub document: DocumentId,
    /// First declaration byte.
    pub start_byte: usize,
    /// Declared name.
    pub name: String,
}

impl SymbolUnitId {
    /// Symbol identity inside `document`.
    #[must_use]
    pub fn new(document: DocumentId, start_byte: usize, name: impl Into<String>) -> Self {
        Self {
            document,
            start_byte,
            name: name.into(),
        }
    }
}

/// One indexed declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolUnit {
    /// Symbol identity.
    pub id: SymbolUnitId,
    /// Declaration category.
    pub kind: SymbolKind,
    /// Byte range of the declaration.
    pub range: SourceRange,
    /// Inline module scope, outermost first.
    pub scope_path: Vec<String>,
    /// Revision the symbol was produced from.
    pub revision: Revision,
}

impl SymbolUnit {
    /// Fully qualified spelling inside its file, e.g. `outer::Thing`.
    #[must_use]
    pub fn qualified(&self) -> String {
        if self.scope_path.is_empty() {
            self.id.name.clone()
        } else {
            format!("{}::{}", self.scope_path.join("::"), self.id.name)
        }
    }

    /// Provenance for this symbol.
    #[must_use]
    pub fn provenance(&self, fingerprint: SourceFingerprint) -> Provenance {
        Provenance::new(self.id.document.clone(), fingerprint, self.range)
    }
}

/// Category of a durable memory unit.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MemoryKind {
    /// A verified statement about the workspace.
    Fact,
    /// A recorded choice with its rationale.
    Decision,
    /// A free-form note.
    Note,
    /// A user preference that shapes future work.
    Preference,
    /// A summary of prior work or a conversation.
    Summary,
}

impl MemoryKind {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Decision => "decision",
            Self::Note => "note",
            Self::Preference => "preference",
            Self::Summary => "summary",
        }
    }
}

/// Opaque identity of one memory unit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MemoryId(Uuid);

impl MemoryId {
    /// Generate a fresh identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Underlying UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

/// One retrievable memory unit.
///
/// The body is the only place in this model that holds user-written text: it
/// is not derived from a workspace file, so there is no snapshot to fetch it
/// from. Source-derived memories still carry [`MemoryRecord::provenance`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryRecord {
    /// Memory identity.
    pub id: MemoryId,
    /// Access-control record.
    pub acl: Acl,
    /// Category.
    pub kind: MemoryKind,
    /// Short title used for exact/phrase matching.
    pub title: String,
    /// Body text.
    pub body: String,
    /// Tags used for filtering.
    pub tags: Vec<String>,
    /// Source range the memory was written from, when applicable.
    pub provenance: Option<Provenance>,
    /// Revision of the memory itself; bumped on edit.
    pub revision: Revision,
}

impl MemoryRecord {
    /// Build a workspace-visible memory owned by `acl`.
    #[must_use]
    pub fn new(
        acl: Acl,
        kind: MemoryKind,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: MemoryId::new(),
            acl,
            kind,
            title: title.into(),
            body: body.into(),
            tags: Vec::new(),
            provenance: None,
            revision: Revision(1),
        }
    }

    /// Attach tags.
    #[must_use]
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Attach source provenance.
    #[must_use]
    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = Some(provenance);
        self
    }
}

/// Kind discriminator for a retrievable unit.
///
/// The kind is derived from [`UnitRef`] and is the value used in filters, the
/// Elasticsearch surface, and context segment roles.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum UnitKind {
    /// Whole source document.
    Document,
    /// Source chunk.
    Chunk,
    /// Source symbol declaration.
    Symbol,
    /// Durable memory unit.
    Memory,
}

impl UnitKind {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Chunk => "chunk",
            Self::Symbol => "symbol",
            Self::Memory => "memory",
        }
    }

    /// Parse the machine-readable name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "document" => Some(Self::Document),
            "chunk" => Some(Self::Chunk),
            "symbol" => Some(Self::Symbol),
            "memory" => Some(Self::Memory),
            _ => None,
        }
    }
}

/// Reference to any indexed unit.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum UnitRef {
    /// Whole document.
    Document(DocumentId),
    /// Source chunk.
    Chunk(ChunkId),
    /// Source symbol.
    Symbol(SymbolUnitId),
    /// Memory unit.
    Memory(MemoryId),
}

impl UnitRef {
    /// Stable string identity, used for de-duplication and citations.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Document(id) => format!("document:{id}"),
            Self::Chunk(id) => format!("chunk:{}#{}", id.document, id.ordinal),
            Self::Symbol(id) => format!("symbol:{}@{}", id.document, id.start_byte),
            Self::Memory(id) => format!("memory:{id}"),
        }
    }

    /// Unit-kind label.
    #[must_use]
    pub const fn kind_label(&self) -> &'static str {
        match self {
            Self::Document(_) => "document",
            Self::Chunk(_) => "chunk",
            Self::Symbol(_) => "symbol",
            Self::Memory(_) => "memory",
        }
    }

    /// Kind of this unit.
    #[must_use]
    pub const fn kind(&self) -> UnitKind {
        match self {
            Self::Document(_) => UnitKind::Document,
            Self::Chunk(_) => UnitKind::Chunk,
            Self::Symbol(_) => UnitKind::Symbol,
            Self::Memory(_) => UnitKind::Memory,
        }
    }

    /// Document the unit belongs to, if it is source-derived.
    #[must_use]
    pub fn document(&self) -> Option<&DocumentId> {
        match self {
            Self::Document(id) => Some(id),
            Self::Chunk(id) => Some(&id.document),
            Self::Symbol(id) => Some(&id.document),
            Self::Memory(_) => None,
        }
    }
}

/// Lowercase word tokens used by the lexical index.
///
/// Identifiers are split on non-alphanumerics and on camel-case boundaries,
/// so `HybridRetriever` also yields `hybrid` and `retriever`. Tokens shorter
/// than two bytes are dropped to keep the posting lists meaningful.
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && previous_lower && !current.is_empty() {
                push_token(&mut tokens, &mut current);
            }
            for lower in ch.to_lowercase() {
                current.push(lower);
            }
            previous_lower = ch.is_lowercase() || ch.is_numeric();
        } else {
            push_token(&mut tokens, &mut current);
            previous_lower = false;
        }
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    // Single-character *letters* are separator noise ("a::b"), but a lone digit
    // is meaningful inside identifiers (`utf8`, `chunk_id_2`), so keep it.
    let keep = current.len() > 1 || current.bytes().all(|byte| byte.is_ascii_digit());
    if keep && !current.is_empty() {
        tokens.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{scope::OwnerScope, scope::TenantId, scope::UserId};

    #[test]
    fn tokenizer_splits_snake_and_camel_case() {
        let tokens = tokenize("HybridRetriever::build_index(chunk_id_2)");
        assert_eq!(
            tokens,
            vec!["hybrid", "retriever", "build", "index", "chunk", "id", "2"]
        );
    }

    #[test]
    fn tokenizer_drops_single_character_tokens() {
        assert_eq!(tokenize("a b cc"), vec!["cc".to_owned()]);
    }

    #[test]
    fn unit_keys_are_stable_and_unique() {
        let workspace = WorkspaceId::new();
        let document = DocumentId::new(workspace, "src/lib.rs");
        let chunk = UnitRef::Chunk(ChunkId::new(document.clone(), 3));
        let symbol = UnitRef::Symbol(SymbolUnitId::new(document.clone(), 7, "Thing"));
        assert_eq!(chunk.key(), format!("chunk:{document}#3"));
        assert_eq!(symbol.key(), format!("symbol:{document}@7"));
        assert_eq!(chunk.kind_label(), "chunk");
        assert_eq!(symbol.document(), Some(&document));
    }

    #[test]
    fn qualified_symbol_includes_scope() {
        let workspace = WorkspaceId::new();
        let unit = SymbolUnit {
            id: SymbolUnitId::new(DocumentId::new(workspace, "src/lib.rs"), 10, "Inner"),
            kind: SymbolKind::Struct,
            range: SourceRange {
                start_byte: 10,
                end_byte: 20,
            },
            scope_path: vec!["outer".to_owned()],
            revision: Revision(1),
        };
        assert_eq!(unit.qualified(), "outer::Inner");
    }

    #[test]
    fn memory_records_carry_acl_and_provenance() {
        let tenant = TenantId::new();
        let workspace = WorkspaceId::new();
        let acl = Acl::private(OwnerScope::durable(tenant, UserId::new()), Some(workspace));
        let memory = MemoryRecord::new(acl, MemoryKind::Decision, "Use RRF", "Reciprocal rank.")
            .with_tags(["retrieval"])
            .with_provenance(Provenance::new(
                DocumentId::new(workspace, "src/retrieval.rs"),
                SourceFingerprint::of(b"fn f() {}"),
                SourceRange {
                    start_byte: 0,
                    end_byte: 9,
                },
            ));
        assert_eq!(memory.kind.as_str(), "decision");
        assert!(memory.provenance.is_some());
        assert_eq!(memory.tags, vec!["retrieval".to_owned()]);
    }
}
