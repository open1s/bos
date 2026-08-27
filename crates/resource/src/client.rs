//! `ResourceClient`: the unified entry point an agent uses. It routes each call
//! local-first (to a co-located [`ResourceManager`]) and, for URIs that carry a
//! host (`scheme://host/...`), transparently to the matching remote endpoint
//! (a QUIC [`Transport`] or any other). From the caller's perspective a resource
//! on another machine is operated with the exact same `invoke` call as a local
//! one — network location is just part of the URI.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use futures::stream::BoxStream;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::manager::ResourceManager;
use crate::meta::{ResourceInfo, ResourceType};
use crate::transport::{Dispatcher, Transport};
use log::debug;

/// Extract the host component of a resource URI.
///
/// `file://node1/path` -> `node1`, `file:///path` (no authority) -> `""`.
/// The host is what selects a remote endpoint, so a caller can address a
/// resource anywhere on the network without changing the calling code.
fn uri_host(uri: &str) -> &str {
    match uri.find("://") {
        Some(i) => {
            let rest = &uri[i + 3..];
            match rest.find('/') {
                Some(j) => &rest[..j],
                None => rest,
            }
        }
        None => "",
    }
}

/// Unified client used by agents. Local resources (registered in the manager,
/// or with an empty URI authority) are served locally; anything addressed to a
/// known host is forwarded to that host's transport automatically.
pub struct ResourceClient {
    agent: String,
    manager: Option<Arc<ResourceManager>>,
    /// Default remote transport, used when no host-specific endpoint matches.
    default_transport: Option<Arc<dyn Transport>>,
    /// Remote endpoints keyed by the host component of the URI.
    remotes: StdMutex<HashMap<String, Arc<dyn Transport>>>,
}

impl ResourceClient {
    /// Build a client for `agent`. Provide a local manager and/or a default
    /// remote transport; at least one must be present. Use
    /// [`ResourceClient::register_remote`] to add per-host endpoints.
    pub fn new(
        agent: impl Into<String>,
        manager: Option<Arc<ResourceManager>>,
        transport: Option<Arc<dyn Transport>>,
    ) -> Self {
        assert!(
            manager.is_some() || transport.is_some(),
            "ResourceClient needs a manager or a transport"
        );
        Self {
            agent: agent.into(),
            manager,
            default_transport: transport,
            remotes: StdMutex::new(HashMap::new()),
        }
    }

    /// Register a remote endpoint reachable at `host` (the authority of a
    /// `scheme://host/...` URI). After this, `invoke("file://host/...", ...)`
    /// routes transparently to that endpoint. Alias for [`ResourceClient::connect`].
    pub fn register_remote(&self, host: impl Into<String>, transport: Arc<dyn Transport>) {
        self.connect(host, transport)
    }

    /// Attach a remote node reachable at `host` (the URI authority). Once
    /// connected, `invoke("scheme://host/...", ...)` reaches that node with the
    /// same call used for a local resource — the caller never branches on
    /// location.
    pub fn connect(&self, host: impl Into<String>, transport: Arc<dyn Transport>) {
        self.remotes.lock().unwrap().insert(host.into(), transport);
    }

    /// Register a local resource. The resource's URI comes from its
    /// [`ResourceHandler::meta`]; after registration it is operated through the
    /// exact same `invoke`/`subscribe`/`list`/`resolve` calls as any remote
    /// resource. This is the symmetric mirror of [`ResourceClient::connect`]:
    /// `register` adds a resource that happens to live here, `connect` adds the
    /// route to a resource that lives elsewhere.
    pub async fn register(
        &self,
        resource: Box<dyn ResourceHandler>,
        owner: impl Into<String>,
    ) -> Result<()> {
        let manager = self.manager.as_ref().ok_or_else(|| {
            ResourceError::Other("ResourceClient has no local manager to register into".into())
        })?;
        manager.register(resource, owner.into()).await
    }

    /// Select the transport for `uri`: a host-specific remote if registered,
    /// otherwise the default remote transport.
    fn route(&self, uri: &str) -> Option<Arc<dyn Transport>> {
        let host = uri_host(uri);
        if !host.is_empty() {
            if let Some(t) = self.remotes.lock().unwrap().get(host) {
                return Some(t.clone());
            }
        }
        self.default_transport.clone()
    }

    /// Invoke an action on a resource. Routing is automatic:
    /// local manager (if the URI is registered there) → host-specific remote →
    /// default remote → not found.
    pub async fn invoke(&self, uri: &str, action: ResourceAction) -> Result<ResourceOutput> {
        if let Some(mgr) = &self.manager {
            if mgr.contains(uri).await {
                debug!("route {} -> local manager", uri);
                return mgr.dispatch(&self.agent, uri, action).await;
            }
        }
        if let Some(t) = self.route(uri) {
            debug!("route {} -> remote transport", uri);
            return t.invoke(uri, action).await;
        }
        debug!("route {} -> not found (no local match, no transport)", uri);
        Err(ResourceError::NotFound(uri.to_string()))
    }

    /// Subscribe to push events for a resource (routed like [`invoke`]).
    pub async fn subscribe(
        &self,
        uri: &str,
        events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>> {
        if let Some(mgr) = &self.manager {
            if mgr.contains(uri).await {
                debug!("subscribe {} -> local manager", uri);
                return mgr.subscribe(&self.agent, uri, events).await;
            }
        }
        if let Some(t) = self.route(uri) {
            debug!("subscribe {} -> remote transport", uri);
            return t.subscribe(uri, events).await;
        }
        debug!("subscribe {} -> not found", uri);
        Err(ResourceError::NotFound(uri.to_string()))
    }

    /// Discover resources across the local manager and every registered remote
    /// endpoint, de-duplicated by URI.
    pub async fn list(&self, kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        if let Some(mgr) = &self.manager {
            for i in mgr.list(&self.agent, kind).await? {
                seen.insert(i.uri.clone());
                out.push(i);
            }
        }
        let mut remotes: Vec<Arc<dyn Transport>> = {
            let g = self.remotes.lock().unwrap();
            g.values().cloned().collect()
        };
        if let Some(d) = &self.default_transport {
            remotes.push(d.clone());
        }
        for t in remotes {
            for i in t.list(&self.agent, kind).await? {
                if seen.insert(i.uri.clone()) {
                    out.push(i);
                }
            }
        }
        Ok(out)
    }

    /// Resolve a single URI to its info (routed like [`invoke`]).
    pub async fn resolve(&self, uri: &str) -> Result<Option<ResourceInfo>> {
        if let Some(mgr) = &self.manager {
            if let Some(i) = mgr.resolve(&self.agent, uri).await? {
                return Ok(Some(i));
            }
        }
        if let Some(t) = self.route(uri) {
            return t.resolve(&self.agent, uri).await;
        }
        Ok(None)
    }

    /// Bind a [`Resource`] handle to `uri`. The handle operates the resource —
    /// local or remote — with the *same* `invoke`/`subscribe` methods, so a
    /// caller can obtain a handle once and forget where the resource lives.
    pub fn resource(self: &Arc<Self>, uri: impl Into<String>) -> Resource {
        Resource::new(self.clone(), uri)
    }

    /// Data-plane read, routed like [`invoke`]. The returned stream yields
    /// chunks as they arrive — bytes are not buffered as a whole on either
    /// side. `len: None` streams from `offset` to end-of-data.
    pub async fn read_stream(
        &self,
        uri: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<crate::transport::ChunkStream> {
        if let Some(mgr) = &self.manager {
            if mgr.contains(uri).await {
                return mgr.read_stream(&self.agent, uri, offset, len).await;
            }
        }
        if let Some(t) = self.route(uri) {
            return t.read_stream(uri, offset, len).await;
        }
        Err(ResourceError::NotFound(uri.to_string()))
    }

    /// Data-plane write sink, routed like [`invoke`]. Chunks are written at
    /// increasing offsets starting at `offset`; `finish` returns the total.
    pub async fn write_stream(
        &self,
        uri: &str,
        offset: u64,
    ) -> Result<Box<dyn crate::transport::ChunkWriter>> {
        if let Some(mgr) = &self.manager {
            if mgr.contains(uri).await {
                return mgr.write_stream(&self.agent, uri, offset).await;
            }
        }
        if let Some(t) = self.route(uri) {
            return t.write_stream(uri, offset).await;
        }
        Err(ResourceError::NotFound(uri.to_string()))
    }
}

/// A stable handle to one resource, bound to a [`ResourceClient`] and a URI.
/// Lets an agent operate a resource repeatedly without re-passing the address,
/// whether that resource is local or on a remote host.
pub struct Resource {
    client: Arc<ResourceClient>,
    uri: String,
}

impl Resource {
    /// Bind a handle to `uri` on `client`.
    pub fn new(client: Arc<ResourceClient>, uri: impl Into<String>) -> Self {
        Self {
            client,
            uri: uri.into(),
        }
    }

    /// The bound URI.
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Invoke an action on the bound resource.
    pub async fn invoke(&self, action: ResourceAction) -> Result<ResourceOutput> {
        self.client.invoke(&self.uri, action).await
    }

    /// Subscribe to events on the bound resource.
    pub async fn subscribe(
        &self,
        events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>> {
        self.client.subscribe(&self.uri, events).await
    }

    /// Data-plane read on the bound resource.
    pub async fn read_stream(
        &self,
        offset: u64,
        len: Option<u64>,
    ) -> Result<crate::transport::ChunkStream> {
        self.client.read_stream(&self.uri, offset, len).await
    }

    /// Data-plane write sink on the bound resource.
    pub async fn write_stream(
        &self,
        offset: u64,
    ) -> Result<Box<dyn crate::transport::ChunkWriter>> {
        self.client.write_stream(&self.uri, offset).await
    }
}