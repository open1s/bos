//! File-backed memory that survives process restarts.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use super::{InMemoryMemory, MemoryItem, MemoryStore};

/// A [`MemoryStore`] that persists items to a JSON-lines file.
///
/// Each line is one serialized [`MemoryItem`], so the file stays readable and
/// diff-friendly. Mutations rewrite the whole file through a sibling temporary
/// file and an atomic rename, so an interrupted write cannot truncate the
/// store. Reads are served from an in-memory cache, and writes are serialized
/// so concurrent callers cannot interleave snapshots.
///
/// Treat a given path as single-writer: two processes appending to the same
/// file concurrently will race, because the snapshot is replaced wholesale.
pub struct FileMemory {
    path: PathBuf,
    inner: InMemoryMemory,
    write_guard: Mutex<()>,
}

impl std::fmt::Debug for FileMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileMemory")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl FileMemory {
    /// Open (or create) a store backed by `path`.
    ///
    /// A missing file yields an empty store. A file whose lines do not parse as
    /// [`MemoryItem`] returns [`std::io::ErrorKind::InvalidData`], naming the
    /// offending line, rather than silently dropping data.
    pub async fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let items = match tokio::fs::read_to_string(&path).await {
            Ok(text) => parse_items(&path, &text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            inner: InMemoryMemory::from_items(items),
            write_guard: Mutex::new(()),
        })
    }

    /// The backing file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persist the current items atomically.
    async fn persist(&self) {
        let mut body = String::new();
        for item in self.inner.all().await {
            match serde_json::to_string(&item) {
                Ok(line) => {
                    body.push_str(&line);
                    body.push('\n');
                }
                Err(error) => {
                    log::warn!("memory: cannot serialize item {}: {error}", item.id);
                    return;
                }
            }
        }
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(error) = tokio::fs::create_dir_all(parent).await {
                    log::warn!("memory: cannot create {}: {error}", parent.display());
                    return;
                }
            }
        }
        let tmp = self.path.with_extension("tmp");
        if let Err(error) = tokio::fs::write(&tmp, body).await {
            log::warn!("memory: cannot write {}: {error}", tmp.display());
            return;
        }
        if let Err(error) = tokio::fs::rename(&tmp, &self.path).await {
            log::warn!("memory: cannot replace {}: {error}", self.path.display());
        }
    }
}

fn parse_items(path: &Path, text: &str) -> std::io::Result<Vec<MemoryItem>> {
    let mut items = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<MemoryItem>(line) {
            Ok(item) => items.push(item),
            Err(error) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}:{}: {error}", path.display(), index + 1),
                ));
            }
        }
    }
    Ok(items)
}

#[async_trait]
impl MemoryStore for FileMemory {
    async fn add(&self, content: String, metadata: Option<Value>) -> MemoryItem {
        let _guard = self.write_guard.lock().await;
        let item = self.inner.add(content, metadata).await;
        self.persist().await;
        item
    }

    async fn all(&self) -> Vec<MemoryItem> {
        self.inner.all().await
    }

    async fn search(&self, query: &str, limit: usize) -> Vec<MemoryItem> {
        self.inner.search(query, limit).await
    }

    async fn remove(&self, id: &str) -> bool {
        let _guard = self.write_guard.lock().await;
        let removed = self.inner.remove(id).await;
        if removed {
            self.persist().await;
        }
        removed
    }

    async fn clear(&self) {
        let _guard = self.write_guard.lock().await;
        self.inner.clear().await;
        self.persist().await;
    }

    async fn len(&self) -> usize {
        self.inner.len().await
    }
}
