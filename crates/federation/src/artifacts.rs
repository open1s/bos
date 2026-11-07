//! Artifact cleanup: ask the Node that holds bytes to drop named references.
//!
//! The task graph owns references, never the bytes. A host installs an
//! ArtifactStore to make deletion real; the protocol records per-artifact
//! outcomes so a caller can reconcile rather than assume success.

use async_trait::async_trait;
use capability::{CapabilityError, NodeId};
use serde::{Deserialize, Serialize};

use crate::collaboration::ArtifactRef;

/// The envelope kind for an artifact-cleanup request.
pub const ARTIFACT_CLEANUP_KIND: &str = "artifact_cleanup";
/// The schema version of the cleanup contracts.
pub const ARTIFACT_CLEANUP_SCHEMA: u32 = 1;
/// The largest number of references one request may carry.
pub const MAX_CLEANUP_ARTIFACTS: usize = 256;

/// One request to remove artifact references on the receiving Node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCleanupRequest {
    /// Schema version.
    pub schema: u32,
    /// Caller-chosen identity for correlation and idempotency.
    pub request_id: String,
    /// The references to remove. The bytes are never sent.
    pub artifacts: Vec<ArtifactRef>,
    /// Why the caller is cleaning up, for audit.
    pub reason: Option<String>,
}

impl ArtifactCleanupRequest {
    /// A request with the given id and references.
    pub fn new(request_id: impl Into<String>, artifacts: Vec<ArtifactRef>) -> Self {
        Self {
            schema: ARTIFACT_CLEANUP_SCHEMA,
            request_id: request_id.into(),
            artifacts,
            reason: None,
        }
    }

    /// Refuse an unsupported schema or an empty/oversized batch.
    pub fn validate(&self) -> Result<(), CapabilityError> {
        if self.schema != ARTIFACT_CLEANUP_SCHEMA {
            return Err(CapabilityError::Invalid {
                message: format!(
                    "artifact cleanup schema {} is not {ARTIFACT_CLEANUP_SCHEMA}",
                    self.schema
                ),
            });
        }
        if self.request_id.trim().is_empty() {
            return Err(CapabilityError::Invalid {
                message: "artifact cleanup needs a request id".to_string(),
            });
        }
        if self.artifacts.is_empty() || self.artifacts.len() > MAX_CLEANUP_ARTIFACTS {
            return Err(CapabilityError::Invalid {
                message: format!("artifact cleanup carries 1..={MAX_CLEANUP_ARTIFACTS} references"),
            });
        }
        Ok(())
    }
}

/// Why one artifact could not be removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactFailure {
    /// The reference's URI.
    pub uri: String,
    /// The host's refusal or I/O error.
    pub error: String,
}

/// The authoritative result of one cleanup request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCleanupResponse {
    /// Schema version.
    pub schema: u32,
    /// Mirrors the request id.
    pub request_id: String,
    /// References the host removed (or found already absent).
    pub removed: Vec<String>,
    /// References the host refused or failed to remove.
    pub failed: Vec<ArtifactFailure>,
}

/// The host seam that actually deletes stored artifact bytes.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Remove one artifact on behalf of an authenticated requester.
    ///
    /// Returning Ok(true) means removed or already absent; Ok(false) means the
    /// store declined; Err carries the reason.
    async fn remove(&self, requester: &NodeId, artifact: &ArtifactRef) -> Result<bool, String>;
}

/// A host store that deletes files confined to one root directory.
#[derive(Debug, Clone)]
pub struct FileArtifactStore {
    root: std::path::PathBuf,
}

impl FileArtifactStore {
    /// Confine deletion to this directory.
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve a URI to a path under the root, refusing escapes.
    fn resolve(&self, uri: &str) -> Result<std::path::PathBuf, String> {
        let raw = uri.strip_prefix("file://").unwrap_or(uri);
        let candidate = std::path::Path::new(raw);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let canonical_root = std::fs::canonicalize(&self.root)
            .map_err(|error| format!("artifact root is unavailable: {error}"))?;
        let parent = joined
            .parent()
            .ok_or_else(|| "artifact path has no parent".to_string())?;
        let canonical_parent = std::fs::canonicalize(parent)
            .map_err(|error| format!("artifact path is unavailable: {error}"))?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err("artifact path escapes the store root".to_string());
        }
        Ok(joined)
    }
}

#[async_trait]
impl ArtifactStore for FileArtifactStore {
    async fn remove(&self, _requester: &NodeId, artifact: &ArtifactRef) -> Result<bool, String> {
        let path = self.resolve(&artifact.uri)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeIdentity;

    #[tokio::test]
    async fn a_file_store_removes_inside_the_root_and_refuses_escapes() {
        let root = std::env::temp_dir().join(format!("bos-artifacts-{}", std::process::id()));
        let outside = std::env::temp_dir().join(format!("bos-outside-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        let inside = root.join("a.bin");
        std::fs::write(&inside, b"x").expect("inside");
        std::fs::write(&outside, b"x").expect("outside");
        let store = FileArtifactStore::new(&root);
        let requester = NodeIdentity::from_seed([1u8; 32]).node_id();

        let removed = store
            .remove(
                &requester,
                &ArtifactRef::new("a", inside.display().to_string()),
            )
            .await
            .expect("removed");
        assert!(removed);
        assert!(!inside.exists());
        let escaped = store
            .remove(
                &requester,
                &ArtifactRef::new("b", outside.display().to_string()),
            )
            .await;
        assert!(escaped.is_err());
        assert!(outside.exists());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }
}
