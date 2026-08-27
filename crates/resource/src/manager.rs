//! Resource manager: the local registry, router, and policy enforcement
//! point. Implements [`Dispatcher`] so it can back both in-process and QUIC
//! transports.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceInfo, ResourceType};
use crate::policy::SharedPolicy;
use crate::transport::Dispatcher;
use log::debug;

type HandlerSlot = Arc<Mutex<Box<dyn ResourceHandler>>>;

/// The core manager. Holds registered resources and a shared policy.
pub struct ResourceManager {
    handlers: RwLock<HashMap<String, HandlerSlot>>,
    policy: SharedPolicy,
}

impl ResourceManager {
    /// Create a manager with an initial policy document.
    pub fn new(policy: SharedPolicy) -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            policy,
        }
    }

    /// Register a resource. Its `uri` (from `meta()`) is the registry key.
    /// The registering agent becomes the owner recorded in metadata.
    pub async fn register(&self, mut resource: Box<dyn ResourceHandler>, owner: String) -> Result<()> {
        resource.set_owner(owner);
        let uri = resource.meta().uri.clone();
        // Check-and-insert under a single write lock to avoid a TOCTOU race
        // where two concurrent registrations of the same URI both succeed.
        let mut handlers = self.handlers.write().await;
        if handlers.contains_key(&uri) {
            return Err(ResourceError::AlreadyExists(uri));
        }
        handlers.insert(uri, Arc::new(Mutex::new(resource)));
        Ok(())
    }

    /// Find a registered URI (local-first check).
    pub async fn contains(&self, uri: &str) -> bool {
        self.handlers.read().await.contains_key(uri)
    }

    /// Drop a resource from the registry.
    pub async fn deregister(&self, uri: &str) -> Result<()> {
        self.handlers
            .write()
            .await
            .remove(uri)
            .map(|_| ())
            .ok_or_else(|| ResourceError::NotFound(uri.to_string()))
    }

    /// List all registered resources, bypassing policy. Used internally for
    /// bus-based discovery announcements.
    pub async fn list_all(&self) -> Vec<crate::meta::ResourceInfo> {
        let handlers = self.handlers.read().await;
        let mut out = Vec::new();
        for slot in handlers.values() {
            let guard = slot.lock().await;
            let meta = guard.meta();
            out.push(crate::meta::ResourceInfo {
                uri: meta.uri.clone(),
                kind: meta.kind,
                state: meta.state,
                owner: meta.owner.clone(),
            });
        }
        out
    }

    async fn slot(&self, uri: &str) -> Result<HandlerSlot> {
        self.handlers
            .read()
            .await
            .get(uri)
            .cloned()
            .ok_or_else(|| ResourceError::NotFound(uri.to_string()))
    }

    /// Get the slot for `uri`, auto-binding a new handler on first reference
    /// for address-backed schemes (`file://`, `folder://`). This is what makes
    /// a node behave like a local filesystem: any existing path is reachable
    /// by URI without an explicit registration step. Policy still gates the
    /// dispatch before we get here.
    async fn slot_or_bind(&self, uri: &str) -> Result<HandlerSlot> {
        if let Ok(slot) = self.slot(uri).await {
            return Ok(slot);
        }
        let (scheme, path) = uri.split_once("://")
            .ok_or_else(|| ResourceError::NotFound(uri.to_string()))?;
        // Auto-bind a URI against what's actually on the filesystem. A URI is
        // only a valid handle if the scheme matches the on-disk kind, so a
        // file never accidentally binds as a folder handle (or vice versa).
        let kind_matches = match scheme {
            "file" => tokio::fs::metadata(path).await.map(|m| m.is_file()).unwrap_or(false),
            "folder" => tokio::fs::metadata(path).await.map(|m| m.is_dir()).unwrap_or(false),
            _ => return Err(ResourceError::NotFound(uri.to_string())),
        };
if !kind_matches {
            return Err(ResourceError::NotFound(uri.to_string()));
        }
        let handler = crate::explorer::handler_for(uri)
            .ok_or_else(|| ResourceError::NotFound(uri.to_string()))?;
        let slot: HandlerSlot = Arc::new(Mutex::new(handler));
        let mut handlers = self.handlers.write().await;
        // Re-check under the write lock: a concurrent dispatch may have bound it.
        if let Some(existing) = handlers.get(uri) {
            return Ok(existing.clone());
        }
        debug!("auto-bound {}", uri);
        handlers.insert(uri.to_string(), slot.clone());
        Ok(slot)
    }
}

#[async_trait]
impl Dispatcher for ResourceManager {
    async fn dispatch(
        &self,
        agent: &str,
        uri: &str,
        action: ResourceAction,
    ) -> Result<ResourceOutput> {
        // Policy administration is gated by admin identity: an admin may update
        // the policy without a per-action allow rule. Non-admin agents are
        // denied outright.
        if action.is_policy_admin() {
            if !self.policy.is_admin(agent) {
                return Err(ResourceError::PolicyDenied {
                    agent: agent.to_string(),
                    uri: uri.to_string(),
                    action: action.name().to_string(),
                });
            }
            if let ResourceAction::PolicyUpdate { doc } = &action {
                let new_doc = crate::policy::PolicyDoc::from_json(doc)?;
                self.policy.update(new_doc);
                return Ok(ResourceOutput::PolicyUpdated);
            }
        }

        // Enforce (agent, uri, action) for everything else.
        self.policy.authorize(agent, uri, action.name())?;
        debug!("dispatch {} on {} for agent {}", action.name(), uri, agent);

        let slot = self.slot_or_bind(uri).await?;
        let mut guard = slot.lock().await;
        guard.handle(action).await
    }

    async fn subscribe(
        &self,
        agent: &str,
        uri: &str,
        events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>> {
        self.policy.authorize(agent, uri, "subscribe")?;
        let slot = self.slot(uri).await?;
        let mut guard = slot.lock().await;
        match guard.events() {
            Some(stream) => Ok(stream),
            None => {
                let _ = events;
                Err(ResourceError::Unsupported(format!(
                    "resource {uri} does not produce events"
                )))
            }
        }
    }

    async fn list(&self, agent: &str, kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>> {
        let handlers = self.handlers.read().await;
        let mut out = Vec::new();
        for slot in handlers.values() {
            let guard = slot.lock().await;
            let meta = guard.meta();
            if kind.is_some() && Some(meta.kind) != kind {
                continue;
            }
            // Discovery is policy-gated too: only list what `agent` may resolve.
            if self.policy.authorize(agent, &meta.uri, "list").is_err() {
                continue;
            }
            // Clone only the two owned strings; `kind`/`state` are Copy and the
            // (potentially large) `metadata` blob is skipped entirely.
            out.push(ResourceInfo {
                uri: meta.uri.clone(),
                kind: meta.kind,
                state: meta.state,
                owner: meta.owner.clone(),
            });
        }
        Ok(out)
    }

    async fn resolve(&self, agent: &str, uri: &str) -> Result<Option<ResourceInfo>> {
        // Policy gates discovery: a resource the agent may not touch must not
        // even be visible via resolve.
        self.policy.authorize(agent, uri, "resolve")?;
        // `resolve` may add a filesystem handler for `file://`/`folder://`
        // URIs that name an existing path, so discovery is consistent with
        // dispatch: anything the dispatcher can reach can also be resolved.
        let slot = match self.slot_or_bind(uri).await {
            Ok(s) => s,
            Err(ResourceError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let guard = slot.lock().await;
        let meta = guard.meta();
        Ok(Some(ResourceInfo {
            uri: meta.uri.clone(),
            kind: meta.kind,
            state: meta.state,
            owner: meta.owner.clone(),
        }))
    }

    async fn read_stream(
        &self,
        agent: &str,
        uri: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<crate::transport::ChunkStream> {
        self.policy.authorize(agent, uri, "read")?;
        let slot = self.slot_or_bind(uri).await?;
        let mut guard = slot.lock().await;
        // The returned stream owns its own I/O handle (see ResourceHandler::
        // read_stream), so releasing the handler lock here is safe.
        guard.read_stream(offset, len).await
    }

    async fn write_stream(
        &self,
        agent: &str,
        uri: &str,
        offset: u64,
    ) -> Result<Box<dyn crate::transport::ChunkWriter>> {
        self.policy.authorize(agent, uri, "write")?;
        let slot = self.slot_or_bind(uri).await?;
        let mut guard = slot.lock().await;
        guard.write_stream(offset).await
    }
}