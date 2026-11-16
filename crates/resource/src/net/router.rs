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

/// Bytes fetched per relayed `Read` page (matches `FileResource`'s buffered
/// cap).
const RELAY_PAGE: u64 = 8 * 1024 * 1024;

/// Flush a relayed write once the pending buffer reaches this many bytes, so
/// a large stream never accumulates in memory until `finish`.
const RELAY_FLUSH_BYTES: usize = 8 * 1024 * 1024;

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
        // `Read` invokes. Pages are fetched lazily as the consumer polls, so a
        // large read is never buffered whole on the relay.
        let transport = self.upstream.clone();
        let relay_uri = self.relay_uri.clone();
        let uri = uri.to_string();
        let chunks = async_stream::stream! {
            let mut next = offset;
            let mut left = len;
            loop {
                let want = match left {
                    Some(0) => break,
                    Some(l) => l.min(RELAY_PAGE),
                    None => RELAY_PAGE,
                };
                let payload = match encode_action(&ResourceAction::Read {
                    offset: next,
                    len: want,
                }) {
                    Ok(payload) => payload,
                    Err(e) => {
                        yield Err(e.to_string());
                        break;
                    }
                };
                match transport
                    .invoke(
                        &relay_uri,
                        ResourceAction::Forward {
                            uri: uri.clone(),
                            payload,
                        },
                    )
                    .await
                {
                    Ok(ResourceOutput::ReadOk { data }) => {
                        let n = data.len() as u64;
                        if n == 0 {
                            break;
                        }
                        yield Ok(data);
                        next = next.saturating_add(n);
                        if let Some(l) = &mut left {
                            *l = l.saturating_sub(n);
                        }
                    }
                    Ok(other) => {
                        yield Err(format!("relay chunk read: unexpected {other:?}"));
                        break;
                    }
                    Err(e) => {
                        yield Err(e.to_string());
                        break;
                    }
                }
            }
        };
        Ok(Box::pin(chunks))
    }

    async fn write_stream(
        &self,
        uri: &str,
        offset: u64,
    ) -> Result<Box<dyn crate::transport::ChunkWriter>> {
        // Chunks are flushed through the relay in bounded pages, so a large
        // write stream does not sit in memory until `finish`.
        Ok(Box::new(RelayChunkWriter {
            transport: self.upstream.clone(),
            relay_uri: self.relay_uri.clone(),
            uri: uri.to_string(),
            offset,
            buf: Vec::new(),
            written: 0,
        }))
    }
}

/// Collect chunks and write them through the relay in bounded pages.
struct RelayChunkWriter {
    transport: Arc<dyn Transport>,
    relay_uri: String,
    uri: String,
    offset: u64,
    buf: Vec<u8>,
    written: u64,
}

impl RelayChunkWriter {
    /// Send the pending buffer as one `Write` through the relay.
    async fn flush(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let at = self.offset.saturating_add(self.written);
        let data = std::mem::take(&mut self.buf);
        let len = data.len() as u64;
        let payload = encode_action(&ResourceAction::Write { offset: at, data })?;
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
            ResourceOutput::WriteOk { written } => {
                if written != len {
                    return Err(ResourceError::Other(format!(
                        "relay write page: expected {len} bytes, wrote {written}"
                    )));
                }
                self.written = self.written.saturating_add(written);
                Ok(())
            }
            other => Err(ResourceError::Other(format!(
                "relay write expected WriteOk, got {other:?}"
            ))),
        }
    }
}

#[async_trait]
impl crate::transport::ChunkWriter for RelayChunkWriter {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() >= RELAY_FLUSH_BYTES {
            self.flush().await?;
        }
        Ok(())
    }

    async fn finish(&mut self) -> Result<u64> {
        self.flush().await?;
        Ok(self.written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::BoxStream;
    use futures::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Minimal transport that answers relayed Read/Write forwards.
    #[derive(Default)]
    struct FakeTransport {
        reads: AtomicUsize,
        writes: Mutex<Vec<(u64, Vec<u8>)>>,
        data: Vec<u8>,
    }

    #[async_trait]
    impl Transport for FakeTransport {
        async fn invoke(&self, _uri: &str, action: ResourceAction) -> Result<ResourceOutput> {
            let inner = match action {
                ResourceAction::Forward { payload, .. } => decode_action(&payload)?,
                other => {
                    return Err(ResourceError::Unsupported(format!(
                        "fake transport got {:?}",
                        other.name()
                    )))
                }
            };
            match inner {
                ResourceAction::Read { .. } => {
                    if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                        Ok(ResourceOutput::ReadOk {
                            data: self.data.clone(),
                        })
                    } else {
                        Ok(ResourceOutput::ReadOk { data: Vec::new() })
                    }
                }
                ResourceAction::Write { offset, data } => {
                    self.writes
                        .lock()
                        .expect("write log")
                        .push((offset, data.clone()));
                    Ok(ResourceOutput::WriteOk {
                        written: data.len() as u64,
                    })
                }
                other => Err(ResourceError::Unsupported(format!(
                    "fake transport does not serve {:?}",
                    other.name()
                ))),
            }
        }

        async fn subscribe(
            &self,
            _uri: &str,
            _events: Vec<String>,
        ) -> Result<BoxStream<'static, ResourceEvent>> {
            Err(ResourceError::Unsupported("fake subscribe".into()))
        }

        async fn list(
            &self,
            _agent: &str,
            _kind: Option<ResourceType>,
        ) -> Result<Vec<ResourceInfo>> {
            Ok(Vec::new())
        }

        async fn resolve(&self, _agent: &str, _uri: &str) -> Result<Option<ResourceInfo>> {
            Ok(None)
        }

        async fn read_stream(
            &self,
            _uri: &str,
            _offset: u64,
            _len: Option<u64>,
        ) -> Result<crate::transport::ChunkStream> {
            Err(ResourceError::Unsupported("fake read_stream".into()))
        }

        async fn write_stream(
            &self,
            _uri: &str,
            _offset: u64,
        ) -> Result<Box<dyn crate::transport::ChunkWriter>> {
            Err(ResourceError::Unsupported("fake write_stream".into()))
        }
    }

    #[tokio::test]
    async fn relay_read_stream_is_lazy() {
        let fake = Arc::new(FakeTransport {
            data: b"hello".to_vec(),
            ..Default::default()
        });
        let relay = RelayTransport::new(fake.clone(), "downstream");

        let mut stream = relay
            .read_stream("file:///x", 0, None)
            .await
            .expect("stream");
        assert_eq!(
            fake.reads.load(Ordering::SeqCst),
            0,
            "read_stream must not fetch pages before the consumer polls"
        );

        let first = stream.next().await.expect("chunk").expect("ok");
        assert_eq!(first, b"hello");
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);

        assert!(stream.next().await.is_none(), "EOF ends the stream");
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn relay_write_stream_flushes_in_bounded_pages() {
        let fake = Arc::new(FakeTransport::default());
        let relay = RelayTransport::new(fake.clone(), "downstream");
        let mut writer = relay.write_stream("file:///x", 0).await.expect("writer");

        let chunk = vec![7u8; 5 * 1024 * 1024];
        for _ in 0..3 {
            writer.write_chunk(&chunk).await.expect("write chunk");
        }
        let total = writer.finish().await.expect("finish");
        assert_eq!(total, 15 * 1024 * 1024);

        let writes = fake.writes.lock().expect("write log");
        assert!(
            writes.len() >= 2,
            "expected bounded flushes, got {} write(s)",
            writes.len()
        );
        assert_eq!(writes[0].0, 0);
        assert_eq!(
            writes.iter().map(|(_, data)| data.len()).sum::<usize>(),
            15 * 1024 * 1024
        );
    }
}
