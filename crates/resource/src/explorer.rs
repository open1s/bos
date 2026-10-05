//! Unified resource explorer: discover, describe, traverse, peek, and watch
//! resources through a single [`ResourceClient`], without the caller (or this
//! module) ever distinguishing local from remote.
//!
//! Everything here is built on the client's four verbs —
//! `invoke`/`subscribe`/`list`/`resolve` — so a resource hosted on another
//! machine explores exactly like a local one.

use std::sync::Arc;

use futures::stream::BoxStream;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::client::ResourceClient;
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceInfo, ResourceType};
use crate::resource::{
    combine::CombineResource, file::FileResource, folder::FolderResource, mem::MemResource,
    proc::ProcResource, sock::SockResource,
};
use log::debug;

/// Construct a local handler for a well-known `scheme://` URI: `file://`,
/// `folder://`, `mem://`, `proc://`, `sock://`, and `combine://`. The
/// operand is the URI tail (`file:///tmp/x` -> `/tmp/x`, `combine://foo`
/// -> `foo`). Returns `None` for an unrecognized scheme.
///
/// This is the *construction* counterpart of the seamless operation model: the
/// caller names a resource by URI and gets a registration-ready handler, the
/// same way they later operate it by URI.
pub fn handler_for(uri: &str) -> Option<Box<dyn ResourceHandler>> {
    let (scheme, operand) = uri.split_once("://")?;
    Some(match scheme {
        "file" => Box::new(FileResource::new(operand)),
        "folder" => Box::new(FolderResource::new(operand)),
        "mem" => Box::new(MemResource::new(operand)),
        "proc" => Box::new(ProcResource::new(operand)),
        "sock" => Box::new(SockResource::new(operand)),
        "combine" => Box::new(CombineResource::new(operand)),
        _ => return None,
    })
}

/// One row of a [`Explorer::tree`] walk, flattened with its depth for indented
/// rendering.
#[derive(Debug)]
pub enum Row {
    /// A registered resource (came out of `resolve`).
    Resource { depth: usize, info: ResourceInfo },
    /// A listing entry inside a resource (folder entry, mem key, …) that is
    /// not itself a registered resource — shown as a leaf.
    Child { depth: usize, name: String },
}

/// Read-only explorer over a [`ResourceClient`]. The SAME code explores local
/// and remote resources: routing, identity, and policy all live behind the
/// client, not here.
pub struct Explorer {
    client: Arc<ResourceClient>,
}

impl Explorer {
    /// Wrap an existing client.
    pub fn new(client: Arc<ResourceClient>) -> Self {
        Self { client }
    }

    /// Discover all resources reachable from the client (local + every
    /// connected node), optionally filtered by kind.
    pub async fn describe(&self, kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>> {
        self.client.list(kind).await
    }

    /// Interactive state of a single resource (uri + state + owner).
    pub async fn status(&self, uri: &str) -> Result<ResourceInfo> {
        self.client
            .resolve(uri)
            .await?
            .ok_or_else(|| ResourceError::NotFound(uri.to_string()))
    }

    /// Recursive listing: registered resources (`Row::Resource`) interleaved
    /// with their listing entries (`Row::Child`). An entry that itself
    /// resolves to a registered resource (e.g. a sub-folder that was
    /// registered separately) is recursed into, up to `max_depth`.
    pub async fn tree(&self, uri: &str, max_depth: usize) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        self.walk(uri, 0, max_depth, &mut rows).await;
        Ok(rows)
    }

    /// Recursion engine: resolve → emit → list children → recurse into the
    /// ones that resolve. Cycles and errors in children are swallowed as
    /// leaves — exploration must be total, never crash on a partial namespace.
    fn walk<'a>(
        &'a self,
        uri: &'a str,
        depth: usize,
        max_depth: usize,
        out: &'a mut Vec<Row>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            match self.client.resolve(uri).await {
                Ok(Some(info)) => {
                    debug!("walk: resolved {} -> {:?}", uri, info);
                    out.push(Row::Resource { depth, info });
                }
                _ => {
                    debug!("walk: unresolved {}", uri);
                    return;
                }
            }
            if depth >= max_depth {
                debug!("walk: max depth {} reached at {}", max_depth, uri);
                return;
            }
            // Listing is best-effort: a resource without listing support
            // (e.g. proc, sock) simply has no children.
            if let Ok(ResourceOutput::Listed { entries }) = self
                .client
                .invoke(uri, ResourceAction::List { pattern: None })
                .await
            {
                debug!("walk: {} listed {} entries", uri, entries.len());
                for entry in entries {
                    // vnode children are member names; the vnode itself owns
                    // the sub-path namespace.
                    if uri.starts_with("vnode://") {
                        let child_uri = format!("{uri}/{entry}");
                        if let Ok(Some(_)) = self.client.resolve(&child_uri).await {
                            debug!("walk: recursing into vnode {}", child_uri);
                            self.walk(&child_uri, depth + 1, max_depth, out).await;
                            continue;
                        }
                        out.push(Row::Child {
                            depth: depth + 1,
                            name: entry,
                        });
                        continue;
                    }
                    // A child may be a file or a folder; probe both schemes so
                    // a file under a folder binds as a file handle (peekable)
                    // and a subdir binds as a folder handle (recurseable).
                    let operand = uri.split_once("://").map(|(_, rest)| rest).unwrap_or(uri);
                    let base = operand.trim_end_matches('/');
                    let file_uri = format!("file://{base}/{entry}");
                    let folder_uri = format!("folder://{base}/{entry}");
                    if let Ok(Some(_)) = self.client.resolve(&folder_uri).await {
                        debug!("walk: recursing into folder {}", folder_uri);
                        self.walk(&folder_uri, depth + 1, max_depth, out).await;
                        continue;
                    }
                    if let Ok(Some(_)) = self.client.resolve(&file_uri).await {
                        debug!("walk: recursing into file {}", file_uri);
                        self.walk(&file_uri, depth + 1, max_depth, out).await;
                        continue;
                    }
                    // Not resolvable as a resource — render as a plain child.
                    debug!("walk: child {} not resolvable as resource", entry);
                    out.push(Row::Child {
                        depth: depth + 1,
                        name: entry,
                    });
                }
            }
        })
    }

    /// Read-only peek at a resource's content: first `len` bytes of a file or
    /// the next `len` bytes off a socket. The resource is opened lazily if it
    /// is not open yet — peeking must not force the caller through state
    /// ceremony. Returns the raw bytes; rendering (hex vs text) is up to the
    /// caller.
    pub async fn peek(&self, uri: &str, len: u64) -> Result<Vec<u8>> {
        debug!("peek {} bytes={}", uri, len);
        let scheme = uri.split("://").next().unwrap_or("");
        // Cheap lifecycle: only bother opening when it's actually required.
        if scheme == "file" {
            if let Ok(ResourceOutput::Status { state }) =
                self.client.invoke(uri, ResourceAction::Status).await
            {
                if state != crate::meta::ResourceStateLabel::Open {
                    debug!("peek: opening file {}", uri);
                    self.client.invoke(uri, ResourceAction::Open).await?;
                }
            }
        }
        let out = match scheme {
            "file" => {
                self.client
                    .invoke(uri, ResourceAction::Read { offset: 0, len })
                    .await?
            }
            "sock" => {
                self.client
                    .invoke(uri, ResourceAction::Recv { max: len })
                    .await?
            }
            other => {
                return Err(ResourceError::Unsupported(format!(
                    "peek is not supported for `{other}://` (files and sockets only)"
                )))
            }
        };
        match out {
            ResourceOutput::ReadOk { data } | ResourceOutput::RecvOk { data } => {
                debug!("peek: got {} bytes", data.len());
                Ok(data)
            }
            other => Err(ResourceError::Other(format!(
                "unexpected peek output: {other:?}"
            ))),
        }
    }

    /// Live view: subscribe to a resource's push events. Returns the event
    /// stream pinned to the caller's lifetime — identical mechanics regardless
    /// of where the resource lives.
    pub async fn watch(
        &self,
        uri: &str,
        events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>> {
        self.client.subscribe(uri, events).await
    }
}
