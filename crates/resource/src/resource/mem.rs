//! Abstract-class resource: an in-memory key/value store, `mem://<id>`.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::action::{ResourceAction, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};

/// A simple in-memory KV store resource.
pub struct MemResource {
    meta: ResourceMeta,
    inner: Mutex<MemInner>,
}

struct MemInner {
    store: HashMap<String, Vec<u8>>,
}

impl MemResource {
    /// Create a memory store resource identified by `id`.
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let uri = format!("mem://{id}");
        let meta = ResourceMeta {
            uri,
            kind: ResourceType::Abstract,
            state: ResourceStateLabel::Closed,
            owner: String::new(),
            metadata: None,
        };
        Self {
            meta,
            inner: Mutex::new(MemInner {
                store: HashMap::new(),
            }),
        }
    }
}

#[async_trait]
impl ResourceHandler for MemResource {
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
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Closed)
            }
            ResourceAction::Status => Ok(ResourceOutput::Status {
                state: self.meta.state,
            }),
            ResourceAction::Get { key } => Ok(ResourceOutput::Got {
                value: inner.store.get(&key).cloned(),
            }),
            ResourceAction::Put { key, value } => {
                inner.store.insert(key, value);
                Ok(ResourceOutput::Put)
            }
            ResourceAction::List { pattern } => {
                let entries: Vec<String> = inner
                    .store
                    .keys()
                    .filter(|k| match &pattern {
                        Some(p) if p != "*" => k.contains(p),
                        _ => true,
                    })
                    .cloned()
                    .collect();
                Ok(ResourceOutput::Listed { entries })
            }
            other => Err(ResourceError::Unsupported(format!(
                "mem resource does not support {:?}",
                other.name()
            ))),
        }
    }
}
