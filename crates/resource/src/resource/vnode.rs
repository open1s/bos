//! Virtual-node resource: aggregates multiple *remote* nodes into a single
//! addressable super node, `vnode://<name>`.
//!
//! Unlike [`crate::resource::combine::CombineResource`] (which owns in-process
//! child handlers), a `VirtualNodeResource` talks to *remote* members through
//! [`Transport`] endpoints. It is a plain `ResourceHandler`, so it can be
//! registered in a [`crate::manager::ResourceManager`] and served —
//! policy-gated, discoverable, and remotely invocable — to any agent with a
//! single call; the fan-out happens server-side.
//!
//! # Aggregation semantics
//!
//! | Action | Behavior |
//! |--------|----------|
//! | `Open` / `Close` | forwarded to all members; any failure → `Err` naming the failed member URIs |
//! | `Status` | the virtual node's own lifecycle state |
//! | `List` | **union** of each member's `List` entries; unreachable members contribute nothing |
//! | `Get` / `Query` / `Invoke` / `Read` / `Recv` / `Stat` | first success wins, members tried in list order |
//! | `Put` / `Write` / `MkDir` / `Remove` / `Truncate` / `Rename` / `Lock` / `Unlock` | all-or-error: attempt all, any failure → `Err` naming failed members (no rollback) |
//!
//! Deliberately out of scope: quorum, rollback, event fan-in, per-member
//! prefix addressing.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tokio::sync::RwLock;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};
use crate::transport::Transport;

/// One remote member of the virtual node.
struct MemberNode {
    /// Full resource URI on the member (e.g. `mem://host/store`).
    uri: String,
    transport: Arc<dyn Transport>,
}

/// A virtual node aggregating remote members behind one `vnode://` URI.
pub struct VirtualNodeResource {
    meta: ResourceMeta,
    /// Agent identity presented to members on `list`/`resolve`/`subscribe`
    /// (transports that authenticate derive identity from the connection, so
    /// this is informational for QUIC but meaningful for in-process tests).
    #[allow(dead_code)] // used by QUIC transport identity; reserved for future event fan-in
    agent: String,
    members: RwLock<Vec<MemberNode>>,
}

/// Snapshot of the member list, taken under the read lock and released before
/// any network call. cheap to clone.
#[derive(Clone)]
struct MemberSnapshot {
    uri: String,
    transport: Arc<dyn Transport>,
}

impl VirtualNodeResource {
    /// Create a virtual node named `name` (`vnode://<name>`) that aggregates
    /// the given `(member_uri, transport)` pairs.
    pub fn new(
        name: impl Into<String>,
        agent: impl Into<String>,
        members: Vec<(String, Arc<dyn Transport>)>,
    ) -> Self {
        let name = name.into();
        Self {
            meta: ResourceMeta {
                uri: format!("vnode://{name}"),
                kind: ResourceType::Combine,
                state: ResourceStateLabel::Closed,
                owner: String::new(),
                metadata: None,
            },
            agent: agent.into(),
            members: RwLock::new(
                members
                    .into_iter()
                    .map(|(uri, transport)| MemberNode { uri, transport })
                    .collect(),
            ),
        }
    }

    /// Convenience constructor: all members live on the same host, addressed
    /// by `(resource_uri_on_host, transport)`.
    pub fn with_transport(
        name: impl Into<String>,
        agent: impl Into<String>,
        transport: Arc<dyn Transport>,
        member_uris: Vec<String>,
    ) -> Self {
        Self::new(
            name,
            agent,
            member_uris
                .into_iter()
                .map(|uri| (uri, transport.clone()))
                .collect(),
        )
    }

    /// Snapshot the member list and release the lock before network calls.
    async fn snapshot(&self) -> Vec<MemberSnapshot> {
        self.members
            .read()
            .await
            .iter()
            .map(|m| MemberSnapshot {
                uri: m.uri.clone(),
                transport: m.transport.clone(),
            })
            .collect()
    }

    /// Union of entries from each member's `List`, skipping unreachable ones.
    async fn list_entries(&self) -> Vec<String> {
        let members = self.snapshot().await;
        let mut entries: Vec<String> = Vec::new();
        for member in &members {
            match member
                .transport
                .invoke(
                    &member.uri,
                    ResourceAction::List { pattern: None },
                )
                .await
            {
                Ok(ResourceOutput::Listed { entries: member_entries }) => {
                    entries.extend(member_entries);
                }
                _ => continue,
            }
        }
        entries.sort();
        entries.dedup();
        entries
    }

    /// Invoke `action` on every member; fail if any member fails.
    async fn all_or_error(&self, action: &ResourceAction) -> Result<Vec<String>> {
        let members = self.snapshot().await;
        let mut failed: Vec<String> = Vec::new();
        let mut ok: Vec<String> = Vec::new();
        for member in &members {
            match member.transport.invoke(&member.uri, action.clone()).await {
                Ok(_) => ok.push(member.uri.clone()),
                Err(_) => failed.push(member.uri.clone()),
            }
        }
        if failed.is_empty() {
            Ok(ok)
        } else {
            Err(ResourceError::Other(format!(
                "{}: action `{}` failed on members [{}]",
                self.meta.uri,
                action.name(),
                failed.join(", ")
            )))
        }
    }

    /// Invoke `action` on members in list order; first success wins.
    async fn first_success(&self, action: ResourceAction) -> Result<ResourceOutput> {
        let members = self.snapshot().await;
        let mut last_err = ResourceError::Unsupported(format!(
            "{} has no member able to serve `{}`",
            self.meta.uri,
            action.name()
        ));
        for member in &members {
            match member.transport.invoke(&member.uri, action.clone()).await {
                Ok(out) => return Ok(out),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    /// Return the list of member URIs.
    pub async fn member_uris(&self) -> Vec<String> {
        self.members
            .read()
            .await
            .iter()
            .map(|m| m.uri.clone())
            .collect()
    }

    /// Add a member after construction.
    pub async fn add_member(&self, uri: String, transport: Arc<dyn Transport>) {
        self.members
            .write()
            .await
            .push(MemberNode { uri, transport });
    }

    /// Remove a member by URI. Returns `true` if found and removed.
    pub async fn remove_member(&self, uri: &str) -> bool {
        let mut members = self.members.write().await;
        let before = members.len();
        members.retain(|m| m.uri != uri);
        members.len() < before
    }
}

#[async_trait]
impl ResourceHandler for VirtualNodeResource {
    fn meta(&self) -> &ResourceMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut ResourceMeta {
        &mut self.meta
    }

    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        match action {
            ResourceAction::Open => {
                self.all_or_error(&ResourceAction::Open).await?;
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Opened)
            }
            ResourceAction::Close => {
                self.all_or_error(&ResourceAction::Close).await?;
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Closed)
            }
            ResourceAction::Status => Ok(ResourceOutput::Status {
                state: self.meta.state,
            }),
            ResourceAction::List { .. } => Ok(ResourceOutput::Listed {
                entries: self.list_entries().await,
            }),
            ResourceAction::Put { .. }
            | ResourceAction::Write { .. }
            | ResourceAction::MkDir { .. }
            | ResourceAction::Remove { .. }
            | ResourceAction::Truncate { .. }
            | ResourceAction::Rename { .. }
            | ResourceAction::Lock { .. }
            | ResourceAction::Unlock => {
                self.all_or_error(&action).await?;
                match action {
                    ResourceAction::Put { .. } => Ok(ResourceOutput::Put),
                    ResourceAction::Write { ref data, .. } => Ok(ResourceOutput::WriteOk {
                        written: data.len() as u64,
                    }),
                    ResourceAction::MkDir { .. } => Ok(ResourceOutput::MkDirOk),
                    ResourceAction::Remove { .. } => Ok(ResourceOutput::Removed),
                    ResourceAction::Truncate { .. } => Ok(ResourceOutput::Truncated),
                    ResourceAction::Rename { .. } => Ok(ResourceOutput::Renamed),
                    ResourceAction::Lock { .. } => Ok(ResourceOutput::Locked),
                    ResourceAction::Unlock => Ok(ResourceOutput::Unlocked),
                    _ => unreachable!(),
                }
            }
            ResourceAction::Get { .. }
            | ResourceAction::Query { .. }
            | ResourceAction::Invoke { .. }
            | ResourceAction::Read { .. }
            | ResourceAction::Recv { .. }
            | ResourceAction::Stat => self.first_success(action).await,
            other => Err(ResourceError::Unsupported(format!(
                "vnode resource does not support {:?}",
                other.name()
            ))),
        }
    }

    fn events(&mut self) -> Option<Pin<Box<dyn Stream<Item = ResourceEvent> + Send + 'static>>> {
        None
    }
}
