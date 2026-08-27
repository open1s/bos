//! Storage-class resource: a host directory, addressable as `folder://<path>`.

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};

/// A directory-backed resource.
pub struct FolderResource {
    meta: ResourceMeta,
    inner: Mutex<FolderInner>,
}

struct FolderInner {
    path: PathBuf,
}

impl FolderResource {
    /// Create (does not create on disk) a folder resource at `path`.
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let uri = format!("folder://{}", path.display());
        let meta = ResourceMeta {
            uri,
            kind: ResourceType::Storage,
            state: ResourceStateLabel::Closed,
            owner: String::new(),
            metadata: None,
        };
        Self {
            meta,
            inner: Mutex::new(FolderInner { path }),
        }
    }
}

#[async_trait]
impl ResourceHandler for FolderResource {
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
                match tokio::fs::metadata(&inner.path).await {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(ResourceError::NotFound(inner.path.display().to_string()));
                    }
                    Err(e) => return Err(ResourceError::Io(e)),
                }
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
            ResourceAction::List { pattern } => {
                let mut read_dir = tokio::fs::read_dir(&inner.path)
                    .await
                    .map_err(|e| match e.kind() {
                        std::io::ErrorKind::NotFound => {
                            ResourceError::NotFound(inner.path.display().to_string())
                        }
                        _ => ResourceError::Io(e),
                    })?;
                let mut entries = Vec::new();
                loop {
                    let entry = match read_dir.next_entry().await {
                        Ok(Some(e)) => e,
                        Ok(None) => break,
                        Err(e) => return Err(ResourceError::Io(e)),
                    };
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(p) = &pattern {
                        if p != "*" && !name.contains(p) {
                            continue;
                        }
                    }
                    entries.push(name);
                }
                Ok(ResourceOutput::Listed { entries })
            }
            ResourceAction::MkDir { recursive } => {
                if recursive {
                    tokio::fs::create_dir_all(&inner.path)
                        .await
                        .map_err(ResourceError::Io)?;
                } else {
                    tokio::fs::create_dir(&inner.path)
                        .await
                        .map_err(ResourceError::Io)?;
                }
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::MkDirOk)
            }
            ResourceAction::Remove { recursive } => {
                if recursive {
                    tokio::fs::remove_dir_all(&inner.path)
                        .await
                        .map_err(ResourceError::Io)?;
                } else {
                    tokio::fs::remove_dir(&inner.path)
                        .await
                        .map_err(ResourceError::Io)?;
                }
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Removed)
            }
            ResourceAction::Rename { new_uri } => {
                let new_path = new_uri
                    .strip_prefix("folder://")
                    .ok_or_else(|| {
                        ResourceError::Unsupported(format!(
                            "folder rename target must be folder://, got {new_uri}"
                        ))
                    })?;
                tokio::fs::rename(&inner.path, new_path)
                    .await
                    .map_err(ResourceError::Io)?;
                inner.path = PathBuf::from(new_path);
                self.meta.uri = new_uri.clone();
                Ok(ResourceOutput::Renamed)
            }
            ResourceAction::Stat => {
                let md = tokio::fs::metadata(&inner.path)
                    .await
                    .map_err(|e| match e.kind() {
                        std::io::ErrorKind::NotFound => {
                            ResourceError::NotFound(inner.path.display().to_string())
                        }
                        _ => ResourceError::Io(e),
                    })?;
                let modified_secs = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(-1);
                Ok(ResourceOutput::StatOk {
                    size: md.len(),
                    is_dir: md.is_dir(),
                    readonly: md.permissions().readonly(),
                    modified_secs,
                })
            }
            ResourceAction::Truncate { .. }
            | ResourceAction::Lock { .. }
            | ResourceAction::Unlock => Err(ResourceError::Unsupported(format!(
                "folder resource does not support {:?}",
                action.name()
            ))),
            other => Err(ResourceError::Unsupported(format!(
                "folder resource does not support {:?}",
                other.name()
            ))),
        }
    }

    fn events(&mut self) -> Option<std::pin::Pin<Box<dyn futures::Stream<Item = ResourceEvent> + Send + 'static>>>
    {
        let path = {
            let inner = self.inner.try_lock().ok()?;
            inner.path.clone()
        };
        crate::resource::watch::watch(&path).ok()
    }
}

