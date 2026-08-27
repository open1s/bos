//! Combine-class resource: a super virtual node that aggregates multiple
//! child resources into a single unified resource, addressable as
//! `combine://<name>`.
//!
//! # Fan-out / aggregate semantics
//!
//! A data action addressed to the combine node reaches its children as:
//!
//! - **Read-like** (`Get`, `Query`, `Invoke`, `Read`, `Recv`): children are
//!   tried in stable URI order; the first success wins. If every child fails,
//!   the last error surfaces.
//! - **Write-like** (`Put`, `Write`): the write is applied to *all* children;
//!   the first failure surfaces (writes already applied are not rolled back).
//! - **List**: union of every child's entries, deduplicated and sorted.
//! - **Open/Close**: forwarded to all children; the combine node mirrors the
//!   resulting lifecycle state.
//!
//! Routing-neutrality note: a combine node never rewrites the action; it
//! replays the same action to its children as-is.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tokio::sync::Mutex;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};

type ChildSlot = Arc<Mutex<Box<dyn ResourceHandler>>>;

/// A super virtual node that combines multiple child resources.
pub struct CombineResource {
    meta: ResourceMeta,
    children: Mutex<HashMap<String, ChildSlot>>,
}

impl CombineResource {
    /// Create a combine resource named `name` (`combine://<name>`).
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let uri = format!("combine://{name}");
        Self {
            meta: ResourceMeta {
                uri,
                kind: ResourceType::Combine,
                state: ResourceStateLabel::Closed,
                owner: String::new(),
                metadata: None,
            },
            children: Mutex::new(HashMap::new()),
        }
    }

    /// Add a child resource under `uri`.
    pub async fn add_child(&self, uri: String, handler: Box<dyn ResourceHandler>) {
        self.children
            .lock()
            .await
            .insert(uri, Arc::new(Mutex::new(handler)));
    }

    /// Number of child nodes.
    pub async fn child_count(&self) -> usize {
        self.children.lock().await.len()
    }

    /// Child slots in stable URI order (stale-free snapshot; the map lock is
    /// released before any child is touched, so child calls can never
    /// deadlock against the combine node itself).
    async fn snapshot(&self) -> Vec<ChildSlot> {
        let map = self.children.lock().await;
        let mut slots: Vec<(&String, &ChildSlot)> = map.iter().collect();
        slots.sort_by(|a, b| a.0.cmp(b.0));
        slots.into_iter().map(|(_, slot)| slot.clone()).collect()
    }

    /// First-success aggregation: run `action` on children in order, return
    /// the first `Ok`. If no child succeeds, return the last error (or
    /// `Unsupported` when there are no children at all).
    async fn first_success(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        let slots = self.snapshot().await;
        let mut last_err = ResourceError::Unsupported(format!(
            "combine {} has no child able to serve {:?}",
            self.meta.uri,
            action.name()
        ));
        for slot in slots {
            let mut guard = slot.lock().await;
            match guard.handle(action.clone()).await {
                Ok(out) => return Ok(out),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    /// Fan-out aggregation: run `action` on every child. Any failure is
    /// returned (previously-written children are not rolled back).
    async fn write_all(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        let slots = self.snapshot().await;
        for slot in &slots {
            let mut guard = slot.lock().await;
            guard.handle(action.clone()).await?;
        }
        match action {
            ResourceAction::Put { .. } => Ok(ResourceOutput::Put),
            ResourceAction::Write { ref data, .. } => Ok(ResourceOutput::WriteOk {
                written: data.len() as u64,
            }),
            _ => unreachable!("write_all is only used for write-like actions"),
        }
    }
}

#[async_trait]
impl ResourceHandler for CombineResource {
    fn meta(&self) -> &ResourceMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut ResourceMeta {
        &mut self.meta
    }

    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        match action {
            ResourceAction::Open => {
                for slot in self.snapshot().await {
                    let mut guard = slot.lock().await;
                    guard.handle(ResourceAction::Open).await?;
                }
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Opened)
            }
            ResourceAction::Close => {
                // Close is idempotent-by-agreement: a failing child does not
                // prevent closing the rest.
                for slot in self.snapshot().await {
                    let mut guard = slot.lock().await;
                    let _ = guard.handle(ResourceAction::Close).await;
                }
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Closed)
            }
            ResourceAction::Status => Ok(ResourceOutput::Status {
                state: self.meta.state,
            }),
            ResourceAction::List { pattern } => {
                let mut entries: Vec<String> = Vec::new();
                for slot in self.snapshot().await {
                    let mut guard = slot.lock().await;
                    if let Ok(ResourceOutput::Listed { entries: child_entries }) = guard
                        .handle(ResourceAction::List {
                            pattern: pattern.clone(),
                        })
                        .await
                    {
                        entries.extend(child_entries);
                    }
                }
                entries.sort();
                entries.dedup();
                Ok(ResourceOutput::Listed { entries })
            }
            // Write-like: replicate to every child.
            ResourceAction::Put { .. } | ResourceAction::Write { .. } => {
                self.write_all(action).await
            }
            // Read-like: first child that can serve it wins.
            ResourceAction::Get { .. }
            | ResourceAction::Query { .. }
            | ResourceAction::Invoke { .. }
            | ResourceAction::Read { .. }
            | ResourceAction::Recv { .. } => self.first_success(action).await,
            other => Err(ResourceError::Unsupported(format!(
                "combine resource does not support {:?}",
                other.name()
            ))),
        }
    }

    fn events(&mut self) -> Option<Pin<Box<dyn Stream<Item = ResourceEvent> + Send + 'static>>> {
        None
    }
}
