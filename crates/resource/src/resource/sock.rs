//! Network-class resource: a TCP socket, addressable as `sock://<host:port>`.

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::action::{ResourceAction, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};

/// A TCP socket resource (connect / bind / send / recv).
pub struct SockResource {
    meta: ResourceMeta,
    inner: Mutex<SockInner>,
}

struct SockInner {
    stream: Option<TcpStream>,
}

impl SockResource {
    /// Create a socket resource targeting `addr` (used for the URI; the actual
    /// peer is given per-`Connect`).
    pub fn new(addr: impl Into<String>) -> Self {
        let addr = addr.into();
        let uri = format!("sock://{addr}");
        let meta = ResourceMeta {
            uri,
            kind: ResourceType::Network,
            state: ResourceStateLabel::Closed,
            owner: String::new(),
            metadata: None,
        };
        Self {
            meta,
            inner: Mutex::new(SockInner { stream: None }),
        }
    }
}

#[async_trait]
impl ResourceHandler for SockResource {
    fn meta(&self) -> &ResourceMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut ResourceMeta {
        &mut self.meta
    }

    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        let mut inner = self.inner.lock().await;
        match action {
            ResourceAction::Open => {
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Opened)
            }
            ResourceAction::Close => {
                inner.stream.take();
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Closed)
            }
            ResourceAction::Status => Ok(ResourceOutput::Status {
                state: self.meta.state,
            }),
            ResourceAction::Connect { addr } => {
                let stream = TcpStream::connect(&addr).await.map_err(ResourceError::Io)?;
                inner.stream = Some(stream);
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Connected)
            }
            ResourceAction::Bind { addr } => {
                let listener = TcpListener::bind(&addr).await.map_err(ResourceError::Io)?;
                let (stream, _peer) = listener.accept().await.map_err(ResourceError::Io)?;
                inner.stream = Some(stream);
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Bound)
            }
            ResourceAction::Send { data } => {
                let s = inner.stream.as_mut().ok_or(ResourceError::Closed)?;
                s.write_all(&data).await.map_err(ResourceError::Io)?;
                Ok(ResourceOutput::Sent {
                    sent: data.len() as u64,
                })
            }
            ResourceAction::Recv { max } => {
                let s = inner.stream.as_mut().ok_or(ResourceError::Closed)?;
                let max = max.min(16 * 1024 * 1024) as usize;
                let mut buf = vec![0u8; max];
                let mut total = 0;
                loop {
                    let n = s.read(&mut buf[total..]).await.map_err(ResourceError::Io)?;
                    if n == 0 {
                        break;
                    }
                    total += n;
                    if total >= max {
                        break;
                    }
                }
                buf.truncate(total);
                Ok(ResourceOutput::RecvOk { data: buf })
            }
            other => Err(ResourceError::Unsupported(format!(
                "sock resource does not support {:?}",
                other.name()
            ))),
        }
    }
}
