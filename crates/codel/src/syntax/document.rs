//! Owned source snapshots paired with native Tree-sitter syntax trees.

use std::{path::PathBuf, sync::Arc};

use tree_sitter::Tree;

use crate::WorkspaceId;
use crate::classification::LanguageId;

/// Native Tree-sitter parse coverage for one source snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParsedDocumentStatus {
    /// The root syntax tree contains no Tree-sitter error or missing nodes.
    Complete,
    /// Tree-sitter produced a recovered tree containing syntax errors or missing nodes.
    Recovered,
}

/// An owned source snapshot and its native Tree-sitter tree.
///
/// This is intentionally not a codel AST. The tree is the upstream
/// Tree-sitter representation and is exposed directly through the tree and
/// root_node accessors. Nodes are borrowed from the owned tree and must not
/// outlive this document.
///
/// The source bytes are retained alongside the tree so node byte ranges can
/// always be interpreted against the exact snapshot that was parsed.
pub struct ParsedDocument {
    workspace_id: WorkspaceId,
    relative_path: PathBuf,
    language: LanguageId,
    provider_id: String,
    provider_version: String,
    source: Arc<[u8]>,
    tree: Tree,
    status: ParsedDocumentStatus,
}

impl ParsedDocument {
    pub(crate) fn new(
        workspace_id: WorkspaceId,
        relative_path: PathBuf,
        language: LanguageId,
        provider_id: String,
        provider_version: String,
        source: Arc<[u8]>,
        tree: Tree,
    ) -> Self {
        let status = if tree.root_node().has_error() {
            ParsedDocumentStatus::Recovered
        } else {
            ParsedDocumentStatus::Complete
        };

        Self {
            workspace_id,
            relative_path,
            language,
            provider_id,
            provider_version,
            source,
            tree,
            status,
        }
    }

    /// Host-supplied identity of the checkout being indexed.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Workspace-relative source path.
    #[must_use]
    pub fn relative_path(&self) -> &std::path::Path {
        &self.relative_path
    }

    /// Language parsed by the provider.
    #[must_use]
    pub const fn language(&self) -> LanguageId {
        self.language
    }

    /// Stable parser/provider identifier.
    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Parser and grammar version used to produce this tree.
    #[must_use]
    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    /// Exact immutable source bytes used to produce the tree.
    #[must_use]
    pub fn source(&self) -> &[u8] {
        &self.source
    }

    /// Shared ownership of the immutable source snapshot.
    #[must_use]
    pub fn source_arc(&self) -> Arc<[u8]> {
        Arc::clone(&self.source)
    }

    /// The native upstream Tree-sitter tree.
    #[must_use]
    pub const fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The native Tree-sitter root node, borrowed from this document.
    #[must_use]
    pub fn root_node(&self) -> tree_sitter::Node<'_> {
        self.tree.root_node()
    }

    /// Whether Tree-sitter completed parsing without recovery nodes.
    #[must_use]
    pub const fn status(&self) -> ParsedDocumentStatus {
        self.status
    }

    /// Resolve a node's byte range against this document's source snapshot.
    ///
    /// Returns None when the range is invalid or the selected bytes are not
    /// valid UTF-8 boundaries. Callers should pass nodes borrowed from this
    /// document's tree.
    #[must_use]
    pub fn text_for_node<'a>(&'a self, node: &tree_sitter::Node<'_>) -> Option<&'a str> {
        self.source
            .get(node.byte_range())
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    }
}
