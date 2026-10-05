//! Transport abstraction. Local resources are dispatched in-process; remote
//! resources travel over QUIC. The server side uses a [`Dispatcher`] (the
//! manager) which enforces policy using the peer's certificate identity.

pub mod inprocess;
pub mod quic;

pub use quic::{QuicServer, QuicTransport};

use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::Stream;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::Result;
use crate::meta::{ResourceInfo, ResourceType};

/// One chunk of a data-plane byte stream. `Err` carries a display string so a
/// wire transport can forward mid-stream failures without a typed error codec.
pub type Chunk = std::result::Result<Vec<u8>, String>;

/// Incremental byte stream returned by data-plane reads.
pub type ChunkStream = Pin<Box<dyn Stream<Item = Chunk> + Send>>;

/// Data-plane write sink: chunked input counterpart of [`ChunkStream`].
/// `finish` flushes, closes, and returns the total bytes accepted.
#[async_trait]
pub trait ChunkWriter: Send {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<()>;
    async fn finish(&mut self) -> Result<u64>;
}

/// Client-side transport: how a [`ResourceClient`](crate::ResourceClient)
/// reaches resources that live on a *remote* host.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Invoke `action` on remote resource `uri`.
    async fn invoke(&self, uri: &str, action: ResourceAction) -> Result<ResourceOutput>;

    /// Subscribe to push events for remote resource `uri`.
    async fn subscribe(
        &self,
        uri: &str,
        events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>>;

    /// List remote resources visible to `agent`, optionally filtered by kind.
    async fn list(&self, agent: &str, kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>>;

    /// Resolve a single remote URI visible to `agent` to its info.
    async fn resolve(&self, agent: &str, uri: &str) -> Result<Option<ResourceInfo>>;

    /// Data-plane read: bytes are delivered incrementally, never buffered as a
    /// single allocation. `len` of `None` reads from `offset` to end-of-data.
    /// Default falls back to a buffered [`invoke`], chunked at 8 MiB.
    async fn read_stream(&self, uri: &str, offset: u64, len: Option<u64>) -> Result<ChunkStream>;

    /// Data-plane write sink starting at byte `offset`.
    async fn write_stream(&self, uri: &str, offset: u64) -> Result<Box<dyn ChunkWriter>>;
}

/// Server-side dispatch target. Implemented by [`ResourceManager`](crate::ResourceManager).
#[async_trait]
pub trait Dispatcher: Send + Sync {
    /// Dispatch an already-authenticated request. `agent` is the peer's
    /// verified certificate identity.
    async fn dispatch(
        &self,
        agent: &str,
        uri: &str,
        action: ResourceAction,
    ) -> Result<ResourceOutput>;

    /// Begin streaming events for an authenticated subscriber.
    async fn subscribe(
        &self,
        agent: &str,
        uri: &str,
        events: Vec<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = ResourceEvent> + Send>>>;

    /// Discovery: list resources visible to `agent` (policy may filter).
    async fn list(&self, agent: &str, kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>>;

    /// Discovery: resolve a single URI visible to `agent`, if any.
    async fn resolve(&self, agent: &str, uri: &str) -> Result<Option<ResourceInfo>>;

    /// Data-plane read on behalf of `agent` (policy is enforced here).
    async fn read_stream(
        &self,
        agent: &str,
        uri: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<ChunkStream>;

    /// Data-plane write sink on behalf of `agent` (policy is enforced here).
    async fn write_stream(
        &self,
        agent: &str,
        uri: &str,
        offset: u64,
    ) -> Result<Box<dyn ChunkWriter>>;
}
