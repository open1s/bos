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
//! # Addressing
//!
//! A vnode mounts each member's content under a single namespace. `List` on
//! the base URI (`vnode://<name>`) returns the union of member List entries
//! (the *contents*, not the member URIs). Sub-paths map the same relative path
//! onto every member: `vnode://<name>/dir/file.txt` → each member's
//! `path/dir/file.txt` (both `file://` and `folder://` variants — whichever
//! exists remotely wins).
//! The manager binds sub-paths on demand via the
//! [`sub_handler`](crate::handler::ResourceHandler::sub_handler) hook.
//!
//! # Aggregation semantics
//!
//! | Action | Behavior |
//! |--------|----------|
//! | `Open` / `Close` | first target that serves the URI (deep paths bind the right scheme) |
//! | `Status` | the vnode's own lifecycle state |
//! | `List` (base) | the union of each member's `List` entries (member content) |
//! | `List` (sub-path) | **union** of each matched member's `List` entries; unreachable members contribute nothing |
//! | `Get` / `Query` / `Invoke` / `Read` / `Recv` / `Stat` | first success wins, members tried in list order |
//! | `Put` / `Write` / `MkDir` / `Remove` / `Truncate` / `Rename` / `Lock` / `Unlock` | all-or-error: attempt all, any failure → `Err` naming failed members (no rollback) |
//!
//! Deliberately out of scope: quorum, rollback, event fan-in.

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
    /// Full resource URI on the member (e.g. `folder://host/base/dir`).
    uri: String,
    transport: Arc<dyn Transport>,
}

/// Shared state between the base vnode and its spawned sub-path handlers.
struct SharedState {
    members: RwLock<Vec<MemberNode>>,
}

/// A virtual node aggregating remote members behind one `vnode://` URI.
pub struct VirtualNodeResource {
    meta: ResourceMeta,
    /// Agent identity presented to members on `list`/`resolve`/`subscribe`
    /// (transports that authenticate derive identity from the connection, so
    /// this is informational for QUIC but meaningful for in-process tests).
    #[allow(dead_code)] // used by QUIC transport identity; reserved for future event fan-in
    agent: String,
    shared: Arc<SharedState>,
    /// Path after `vnode://<name>` this handler is responsible for
    /// (`""` for the base handler).
    relpath: String,
}

/// Snapshot of a member reference, taken under the read lock and released
/// before any network call. Cheap to clone.
#[derive(Clone)]
struct MemberRef {
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
            shared: Arc::new(SharedState {
                members: RwLock::new(
                    members
                        .into_iter()
                        .map(|(uri, transport)| MemberNode { uri, transport })
                        .collect(),
                ),
            }),
            relpath: String::new(),
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

    /// Internal constructor for sub-path handlers.
    fn sub(&self, relpath: &str) -> Box<dyn ResourceHandler> {
        let relpath = relpath.trim_matches('/').to_string();
        let mut meta = self.meta.clone();
        meta.uri = format!("{}/{relpath}", self.meta.uri);
        Box::new(Self {
            meta,
            agent: self.agent.clone(),
            shared: self.shared.clone(),
            relpath,
        })
    }

    /// Target `(uri, transport)` pairs this handler's actions apply to.
    ///
    /// - Base handler (`relpath == ""`): the members themselves.
    /// - `vnode://<name>/<sub>`: each member URI with `/<sub>` appended, in
    ///   both `folder://` and `file://` variants (the remote's auto-bind picks
    ///   whichever exists on disk; the other fails and is skipped).
    async fn targets(&self) -> Vec<MemberRef> {
        let members = self.shared.members.read().await;
        if self.relpath.is_empty() {
            return members
                .iter()
                .map(|m| MemberRef { uri: m.uri.clone(), transport: m.transport.clone() })
                .collect();
        }
        let mut out = Vec::new();
        for m in members.iter() {
            let base = m.uri.trim_end_matches('/');
            let path = base.split_once("://").map(|(_, p)| p).unwrap_or(base);
            out.push(MemberRef {
                uri: format!("file://{path}/{}", self.relpath),
                transport: m.transport.clone(),
            });
            out.push(MemberRef {
                uri: format!("folder://{path}/{}", self.relpath),
                transport: m.transport.clone(),
            });
        }
        out
    }

    /// Union of entries from the `List` of each target, skipping
    /// unreachable/unmatched ones.
    async fn list_entries(&self) -> Vec<String> {
        let targets = self.targets().await;
        let mut entries: Vec<String> = Vec::new();
        for member in &targets {
            match member
                .transport
                .invoke(&member.uri, ResourceAction::List { pattern: None })
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

    /// Invoke `action` on every target; fail if any target fails.
    async fn all_or_error(&self, action: &ResourceAction) -> Result<Vec<String>> {
        let targets = self.targets().await;
        let mut failed: Vec<String> = Vec::new();
        let mut ok: Vec<String> = Vec::new();
        for member in &targets {
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

    /// Invoke `action` on targets in list order; first success wins.
    async fn first_success(&self, action: ResourceAction) -> Result<ResourceOutput> {
        let targets = self.targets().await;
        let mut last_err = ResourceError::Unsupported(format!(
            "{} has no member able to serve `{}`",
            self.meta.uri,
            action.name()
        ));
        for member in &targets {
            match member.transport.invoke(&member.uri, action.clone()).await {
                Ok(out) => return Ok(out),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    /// Return the list of member URIs.
    pub async fn member_uris(&self) -> Vec<String> {
        self.shared
            .members
            .read()
            .await
            .iter()
            .map(|m| m.uri.clone())
            .collect()
    }

    /// Add a member after construction.
    pub async fn add_member(&self, uri: String, transport: Arc<dyn Transport>) {
        self.shared
            .members
            .write()
            .await
            .push(MemberNode { uri, transport });
    }

    /// Remove a member by URI. Returns `true` if found and removed.
    pub async fn remove_member(&self, uri: &str) -> bool {
        let mut members = self.shared.members.write().await;
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
            ResourceAction::Open => match self.first_success(ResourceAction::Open).await {
                Ok(_) => {
                    self.meta.state = ResourceStateLabel::Open;
                    Ok(ResourceOutput::Opened)
                }
                Err(e) => Err(e),
            },
            ResourceAction::Close => match self.first_success(ResourceAction::Close).await {
                Ok(_) => {
                    self.meta.state = ResourceStateLabel::Closed;
                    Ok(ResourceOutput::Closed)
                }
                Err(e) => Err(e),
            },
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

    fn sub_handler(&self, relpath: &str) -> Option<Box<dyn ResourceHandler>> {
        Some(self.sub(relpath))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::ResourceManager;
    use crate::policy::{Effect, PolicyDoc, Rule, SharedPolicy};
    use crate::transport::inprocess::InProcessTransport;

    fn permit_all() -> SharedPolicy {
        SharedPolicy::new(PolicyDoc {
            admins: vec!["t".into()],
            rules: vec![Rule {
                agents: vec!["*".into()],
                uris: vec!["*".into()],
                actions: vec!["*".into()],
                effect: Effect::Allow,
            }],
            ..Default::default()
        })
    }

    /// Two member folders behind one vnode: base List returns the union of
    /// member contents; a sub-path handler lists a named member sub-dir.
    #[tokio::test]
    async fn base_lists_member_and_subhandler_lists_contents() {
        let root_a = std::env::temp_dir().join("bos_vnode_root_a");
        let root_b = std::env::temp_dir().join("bos_vnode_root_b");
        std::fs::create_dir_all(root_a.join("docs")).unwrap();
        std::fs::create_dir_all(root_b.join("docs")).unwrap();
        std::fs::write(root_a.join("docs").join("a.txt"), b"aa").unwrap();
        std::fs::write(root_b.join("docs").join("b.txt"), b"bb").unwrap();
        std::fs::write(root_a.join("top.txt"), b"tt").unwrap();

        let mgr = Arc::new(ResourceManager::new(permit_all()));
        let t: Arc<dyn Transport> = Arc::new(InProcessTransport::new(mgr.clone(), "t"));

        let mut vnode = VirtualNodeResource::new(
            "v",
            "t",
            vec![
                (format!("folder://{}", root_a.display()), t.clone()),
                (format!("folder://{}", root_b.display()), t),
            ],
        );
        // Base List: union of both members' contents.
        match vnode.handle(ResourceAction::List { pattern: None }).await.unwrap() {
            ResourceOutput::Listed { entries } => {
                assert_eq!(entries, vec!["docs".to_string(), "top.txt".to_string()]);
            }
            other => panic!("{other:?}"),
        }

        // Sub-path "docs": union of the two members' docs/ contents.
        let mut sub = vnode.sub_handler("docs").unwrap();
        assert_eq!(sub.meta().uri, "vnode://v/docs");
        match sub.handle(ResourceAction::List { pattern: None }).await.unwrap() {
            ResourceOutput::Listed { entries } => {
                assert_eq!(entries, vec!["a.txt".to_string(), "b.txt".to_string()]);
            }
            other => panic!("{other:?}"),
        }

        // Deep sub-path "docs/a.txt": read via member A's file variant.
        let mut sub2 = vnode.sub_handler("docs/a.txt").unwrap();
        sub2.handle(ResourceAction::Open).await.unwrap();
        match sub2.handle(ResourceAction::Read { offset: 0, len: 10 }).await.unwrap() {
            ResourceOutput::ReadOk { data } => assert_eq!(data, b"aa"),
            other => panic!("{other:?}"),
        }
    }
}
