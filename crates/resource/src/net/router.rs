//! Multi-hop routing: reach a node through an intermediate peer.
//!
//! Two pieces, inverses of each other:
//!
//! - **`RelayResource`** (intermediate / "B" side): a `ResourceHandler`
//!   registered as `relay://<node_id>`. Its `Forward` handler unwraps the
//!   inner action and re-invokes it on the downstream transport. Everything
//!   (fs, proc, vnode member reads) works through the same path — the relay
//!   doesn't care what kind of resource is downstream.
//!
//! - **`RelayTransport`** (origin / "A" side): an endpoint whose `invoke`
//!   wraps any action as `Forward { uri, payload }` and pushes it through
//!   the upstream (to-B) transport. The caller never thinks about routing:
//!   they address `proc://pid` on A, the transport wraps it, B's relay
//!   resource unwraps it and hits C.
//!
//! `list`/`resolve`/`subscribe` are *not* relayed (relay is a datagram path
//! for actions, not a discovery bus). Use a quality vnode on A for those.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::action::{decode_action, encode_action, ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceInfo, ResourceMeta, ResourceType};
use crate::transport::Transport;

/// Intermediate-side resource: accepts `Forward` actions and dispatches them
/// to the configured downstream transport.
pub struct RelayResource {
    meta: ResourceMeta,
    downstream: Arc<dyn Transport>,
}

impl RelayResource {
    /// Register as `relay://<name>`; any action forwarded here goes to
    /// `downstream`.
    pub fn new(name: impl Into<String>, downstream: Arc<dyn Transport>) -> Self {
        let name = name.into();
        Self {
            meta: ResourceMeta {
                uri: format!("relay://{name}"),
                kind: ResourceType::Network,
                state: crate::meta::ResourceStateLabel::Open,
                owner: String::new(),
                metadata: None,
            },
            downstream,
        }
    }
}

#[async_trait]
impl ResourceHandler for RelayResource {
    fn meta(&self) -> &ResourceMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut ResourceMeta {
        &mut self.meta
    }

    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        match action {
            ResourceAction::Forward { uri, payload } => {
                let inner = decode_action(&payload)?;
                self.downstream.invoke(&uri, inner).await
            }
            ResourceAction::Status => Ok(ResourceOutput::Status {
                state: self.meta.state,
            }),
            other => Err(ResourceError::Unsupported(format!(
                "relay does not support {:?} (only Forward)",
                other.name()
            ))),
        }
    }
}

/// Origin-side transport: wraps every action in `Forward { uri, payload }`
/// and sends it to the upstream transport's `relay://<target>` resource.
pub struct RelayTransport {
    upstream: Arc<dyn Transport>,
    /// URI of the relay resource on the upstream, e.g. `relay://nodeC`.
    relay_uri: String,
}

impl RelayTransport {
    /// `upstream` is the transport to the intermediate node; `target_node` is
    /// the name the relay resource is registered under there.
    pub fn new(upstream: Arc<dyn Transport>, target_node: impl Into<String>) -> Self {
        Self {
            upstream,
            relay_uri: format!("relay://{}", target_node.into()),
        }
    }
}

#[async_trait]
impl Transport for RelayTransport {
    async fn invoke(&self, uri: &str, action: ResourceAction) -> Result<ResourceOutput> {
        let payload = encode_action(&action)?;
        self.upstream
            .invoke(
                &self.relay_uri,
                ResourceAction::Forward {
                    uri: uri.to_string(),
                    payload,
                },
            )
            .await
    }

    async fn subscribe(
        &self,
        _uri: &str,
        _events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>> {
        Err(ResourceError::Unsupported(
            "subscribe is not routed through relays".into(),
        ))
    }

    async fn list(&self, _agent: &str, _kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>> {
        Err(ResourceError::Unsupported(
            "list is not routed through relays".into(),
        ))
    }

    async fn resolve(&self, _agent: &str, _uri: &str) -> Result<Option<ResourceInfo>> {
        Err(ResourceError::Unsupported(
            "resolve is not routed through relays".into(),
        ))
    }

    async fn read_stream(
        &self,
        uri: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<crate::transport::ChunkStream> {
        // Relay is not a data-plane fast path: page through with plain
        // `Read` invokes. 8 MiB per page matches FileResource's buffered cap.
        let mut next = offset;
        let mut left = len;
        let mut chunks: Vec<crate::transport::Chunk> = Vec::new();
        loop {
            let want = match left {
                Some(0) => break,
                Some(l) => l.min(8 * 1024 * 1024),
                None => 8 * 1024 * 1024,
            };
            match self
                .invoke(
                    uri,
                    ResourceAction::Read {
                        offset: next,
                        len: want,
                    },
                )
                .await
            {
                Ok(ResourceOutput::ReadOk { data }) => {
                    let n = data.len() as u64;
                    if n == 0 {
                        break;
                    }
                    chunks.push(Ok(data));
                    next += n;
                    if let Some(l) = &mut left {
                        *l = l.saturating_sub(n);
                    }
                }
                Ok(other) => {
                    chunks.push(Err(format!("relay chunk read: unexpected {other:?}")));
                    break;
                }
                Err(e) => {
                    chunks.push(Err(e.to_string()));
                    break;
                }
            }
        }
        Ok(Box::pin(futures::stream::iter(chunks)))
    }

    async fn write_stream(
        &self,
        uri: &str,
        offset: u64,
    ) -> Result<Box<dyn crate::transport::ChunkWriter>> {
        // Buffered on the relay: chunks are accumulated and flushed as one
        // `Write` on `finish`.
        Ok(Box::new(RelayChunkWriter {
            transport: self.upstream.clone(),
            relay_uri: self.relay_uri.clone(),
            uri: uri.to_string(),
            offset,
            buf: Vec::new(),
        }))
    }
}

/// Collect chunks and write them as one buffered `Write` through the relay.
struct RelayChunkWriter {
    transport: Arc<dyn Transport>,
    relay_uri: String,
    uri: String,
    offset: u64,
    buf: Vec<u8>,
}

#[async_trait]
impl crate::transport::ChunkWriter for RelayChunkWriter {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        self.buf.extend_from_slice(chunk);
        Ok(())
    }

    async fn finish(&mut self) -> Result<u64> {
        let payload = crate::action::encode_action(&ResourceAction::Write {
            offset: self.offset,
            data: std::mem::take(&mut self.buf),
        })?;
        let out = self
            .transport
            .invoke(
                &self.relay_uri,
                ResourceAction::Forward {
                    uri: self.uri.clone(),
                    payload,
                },
            )
            .await?;
        match out {
            ResourceOutput::WriteOk { written } => Ok(written),
            other => Err(ResourceError::Other(format!(
                "relay write expected WriteOk, got {other:?}"
            ))),
        }
    }
}
