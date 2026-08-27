//! The handler contract: every concrete resource implements `handle`, which
//! maps a [`ResourceAction`] onto its capabilities and returns a
//! [`ResourceOutput`]. The manager enforces policy *before* calling this.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::meta::ResourceMeta;
use crate::transport::{ChunkStream, ChunkWriter};

/// A concrete, addressable resource.
#[async_trait]
pub trait ResourceHandler: Send + Sync {
    /// Borrow current metadata (URI, kind, state, owner). Returning a reference
    /// avoids cloning the (potentially large) `metadata` blob on every
    /// `list`/`resolve`; callers that need an owned snapshot clone explicitly.
    fn meta(&self) -> &ResourceMeta;

    /// Mutable access to metadata (owner tracking, state changes).
    fn meta_mut(&mut self) -> &mut ResourceMeta;

    /// Execute a single action. Policy is already enforced by the manager.
    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput>;

    /// Optional push-event stream. Return `None` if the resource is not
    /// event-producing (e.g. a plain file). The manager connects this to a
    /// subscriber when `ResourceAction::Subscribe` is issued. The stream must
    /// be owned (`'static`) so the manager can return it independently of the
    /// handler's borrow.
    fn events(&mut self) -> Option<Pin<Box<dyn Stream<Item = ResourceEvent> + Send + 'static>>> {
        let _ = self;
        None
    }

    /// Record the owning agent identity. Called by the manager at registration
    /// time so discovery/metadata reflects who registered the resource.
    fn set_owner(&mut self, owner: String) {
        self.meta_mut().owner = owner;
    }

    /// Data-plane read: byte-chunked, never buffered whole. The default
    /// implementation falls back to a single [`Self::handle`] `Read` — correct
    /// but not incremental. State-bearing handlers should override and return
    /// a stream that owns whatever I/O handle it needs (the manager releases
    /// its lock before the stream is consumed).
    async fn read_stream(&mut self, offset: u64, len: Option<u64>) -> Result<ChunkStream> {
        match self
            .handle(ResourceAction::Read {
                offset,
                len: len.unwrap_or(8 * 1024 * 1024),
            })
            .await
        {
            Ok(ResourceOutput::ReadOk { data }) => {
                Ok(Box::pin(futures::stream::once(async move { Ok(data) })))
            }
            Ok(_) => Err(ResourceError::Unsupported(
                "read fallback returned a non-data output".into(),
            )),
            Err(e) => Err(e),
        }
    }

    /// Data-plane write sink positioned at `offset`. Defaults to unsupported;
    /// storage handlers override.
    async fn write_stream(&mut self, _offset: u64) -> Result<Box<dyn ChunkWriter>> {
        Err(ResourceError::Unsupported(format!(
            "{} does not support streaming writes",
            self.meta().uri
        )))
    }
}