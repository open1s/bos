//! Storage-class resource: a host file, addressable as `file://<path>`.

use std::io::SeekFrom;
use std::path::PathBuf;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};
use crate::resource::lock::LockState;
use crate::transport::{ChunkStream, ChunkWriter};

/// Data-plane chunk size for streamed file I/O.
const STREAM_CHUNK: usize = 64 * 1024;

fn map_io(path: &std::path::Path, e: std::io::Error) -> ResourceError {
    match e.kind() {
        std::io::ErrorKind::NotFound => ResourceError::NotFound(path.display().to_string()),
        std::io::ErrorKind::PermissionDenied => {
            ResourceError::PermissionDenied(path.display().to_string())
        }
        _ => ResourceError::Io(e),
    }
}

/// A file-backed resource. State is guarded by a mutex so concurrent invokes
/// serialize safely. All I/O is asynchronous (`tokio::fs`) so it never blocks
/// the runtime worker.
pub struct FileResource {
    meta: ResourceMeta,
    inner: Mutex<FileInner>,
}

struct FileInner {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    lock: LockState,
}

impl FileResource {
    /// Create (does not open) a file resource at `path`.
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let uri = format!("file://{}", path.display());
        let meta = ResourceMeta {
            uri: uri.clone(),
            kind: ResourceType::Storage,
            state: ResourceStateLabel::Closed,
            owner: String::new(),
            metadata: None,
        };
        Self {
            meta,
            inner: Mutex::new(FileInner {
                path,
                file: None,
                lock: LockState::default(),
            }),
        }
    }
}

#[async_trait]
impl ResourceHandler for FileResource {
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
                let f = tokio::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&inner.path)
                    .await
                    .map_err(|e| match e.kind() {
                        std::io::ErrorKind::NotFound => {
                            ResourceError::NotFound(inner.path.display().to_string())
                        }
                        std::io::ErrorKind::PermissionDenied => {
                            ResourceError::PermissionDenied(inner.path.display().to_string())
                        }
                        _ => ResourceError::Io(e),
                    })?;
                inner.file = Some(f);
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Opened)
            }
            ResourceAction::Close => {
                if let Some(mut f) = inner.file.take() {
                    f.flush().await.map_err(ResourceError::Io)?;
                }
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Closed)
            }
            ResourceAction::Status => Ok(ResourceOutput::Status {
                state: self.meta.state,
            }),
            ResourceAction::Read { offset, len } => {
                let f = inner.file.as_mut().ok_or(ResourceError::Closed)?;
                f.seek(SeekFrom::Start(offset))
                    .await
                    .map_err(ResourceError::Io)?;
                let to_read = len.min(8 * 1024 * 1024) as usize;
                let mut buf = vec![0u8; to_read];
                let n = f.read(&mut buf).await.map_err(ResourceError::Io)?;
                buf.truncate(n);
                Ok(ResourceOutput::ReadOk { data: buf })
            }
            ResourceAction::Write { offset, data } => {
                let f = inner.file.as_mut().ok_or(ResourceError::Closed)?;
                f.seek(SeekFrom::Start(offset))
                    .await
                    .map_err(ResourceError::Io)?;
                f.write_all(&data).await.map_err(ResourceError::Io)?;
                Ok(ResourceOutput::WriteOk {
                    written: data.len() as u64,
                })
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
                Ok(ResourceOutput::MkDirOk)
            }
            ResourceAction::Remove { recursive } => {
                let is_dir = match tokio::fs::metadata(&inner.path).await {
                    Ok(m) => m.is_dir(),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(ResourceError::NotFound(inner.path.display().to_string()));
                    }
                    Err(e) => return Err(ResourceError::Io(e)),
                };
                if is_dir {
                    if recursive {
                        tokio::fs::remove_dir_all(&inner.path)
                            .await
                            .map_err(ResourceError::Io)?;
                    } else {
                        tokio::fs::remove_dir(&inner.path)
                            .await
                            .map_err(ResourceError::Io)?;
                    }
                } else {
                    tokio::fs::remove_file(&inner.path)
                        .await
                        .map_err(ResourceError::Io)?;
                }
                Ok(ResourceOutput::Removed)
            }
            ResourceAction::List { .. } => Err(ResourceError::Unsupported(format!(
                "file {} is not a directory",
                inner.path.display()
            ))),
            ResourceAction::Truncate { len } => {
                let f = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(&inner.path)
                    .await
                    .map_err(|e| map_io(&inner.path, e))?;
                f.set_len(len).await.map_err(|e| map_io(&inner.path, e))?;
                Ok(ResourceOutput::Truncated)
            }
            ResourceAction::Rename { new_uri } => {
                let new_path = new_uri.strip_prefix("file://").ok_or_else(|| {
                    ResourceError::Unsupported(format!(
                        "rename target must be file://, got {new_uri}"
                    ))
                })?;
                tokio::fs::rename(&inner.path, new_path)
                    .await
                    .map_err(|e| map_io(&inner.path, e))?;
                inner.path = PathBuf::from(new_path);
                self.meta.uri = new_uri.clone();
                Ok(ResourceOutput::Renamed)
            }
            ResourceAction::Stat => {
                let md = tokio::fs::metadata(&inner.path)
                    .await
                    .map_err(|e| map_io(&inner.path, e))?;
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
            ResourceAction::Lock { exclusive } => {
                inner.lock.lock(exclusive)?;
                Ok(ResourceOutput::Locked)
            }
            ResourceAction::Unlock => {
                inner.lock.unlock();
                Ok(ResourceOutput::Unlocked)
            }
            other => Err(ResourceError::Unsupported(format!(
                "file resource does not support {:?}",
                other.name()
            ))),
        }
    }

    /// True streaming read: opens a dedicated read handle on the path (so the
    /// shared `Open` handle's file offset is untouched), seeks to `offset`,
    /// and yields `STREAM_CHUNK`-sized reads. `len: None` streams to EOF.
    async fn read_stream(&mut self, offset: u64, len: Option<u64>) -> Result<ChunkStream> {
        let path = {
            let inner = self.inner.lock().await;
            inner.path.clone()
        };
        let mut f = tokio::fs::File::open(&path)
            .await
            .map_err(|e| map_io(&path, e))?;
        f.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| map_io(&path, e))?;
        let stream = async_stream::stream! {
            let mut remaining = len;
            loop {
                let cap = match remaining {
                    Some(0) => break,
                    Some(r) => r.min(STREAM_CHUNK as u64) as usize,
                    None => STREAM_CHUNK,
                };
                let mut buf = vec![0u8; cap];
                match f.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        if let Some(r) = &mut remaining {
                            *r -= n as u64;
                        }
                        yield Ok(buf);
                    }
                    Err(e) => {
                        yield Err(e.to_string());
                        break;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }

    /// Streaming write: a dedicated write handle positioned at `offset`,
    /// writing each chunk as it arrives.
    async fn write_stream(&mut self, offset: u64) -> Result<Box<dyn ChunkWriter>> {
        let path = {
            let inner = self.inner.lock().await;
            inner.path.clone()
        };
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .await
            .map_err(|e| map_io(&path, e))?;
        f.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| map_io(&path, e))?;
        Ok(Box::new(FileChunkWriter { f, written: 0 }))
    }

    fn events(
        &mut self,
    ) -> Option<std::pin::Pin<Box<dyn futures::Stream<Item = ResourceEvent> + Send + 'static>>>
    {
        // Watch the path; events stream until the subscriber drops.
        let path = {
            // `path` is stable: Set via constructor, only mutated on Rename
            // (which re-points both inner.path and meta.uri).
            let inner = self.inner.try_lock().ok()?;
            inner.path.clone()
        };
        crate::resource::watch::watch(&path).ok()
    }
}

/// Chunked writer over a positioned `tokio::fs::File`.
struct FileChunkWriter {
    f: tokio::fs::File,
    written: u64,
}

#[async_trait]
impl ChunkWriter for FileChunkWriter {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        self.f.write_all(chunk).await.map_err(ResourceError::Io)?;
        self.written += chunk.len() as u64;
        Ok(())
    }

    async fn finish(&mut self) -> Result<u64> {
        self.f.flush().await.map_err(ResourceError::Io)?;
        Ok(self.written)
    }
}
