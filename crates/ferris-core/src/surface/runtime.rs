//! What a surface provider is, independent of where it runs.
//!
//! The runtime adapter implements this trait; the engine never calls it. A
//! native provider, a Cordis plugin, a process or WASM guest and a remote
//! node all lower into the same two operations: describe the contract, and
//! produce a frame for a request.

use serde_json::Value;

use super::contract::SurfaceContract;
use super::document::SurfaceDocument;
use super::{Generation, InstanceId, Revision, SurfaceError};

/// One request for a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceRequest {
    /// The instance the host is rendering.
    pub instance: InstanceId,
    /// The generation the request is for.
    pub generation: Generation,
    /// The revision the host is moving from.
    pub revision: Revision,
    /// The mount props the parent supplied.
    pub props: Value,
}

/// A provider of one surface.
///
/// Implementations must be safe to call from the host thread. A provider that
/// needs to block or await belongs behind its runtime adapter instead of
/// inside this method.
pub trait SurfaceProvider: Send + Sync + 'static {
    /// The contract this provider answers.
    fn contract(&self) -> &SurfaceContract;

    /// Produce the document for a request.
    ///
    /// The returned document is untrusted until the engine validates it
    /// against the contract, so a provider that computes a bad document gets
    /// a refusal rather than a broken frame.
    fn frame(&self, request: &SurfaceRequest) -> Result<SurfaceDocument, SurfaceError>;
}
