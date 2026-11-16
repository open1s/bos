//! Storage seam for workspace annotations.

use std::io;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

/// Read/write one workspace's annotation bytes.
pub(crate) trait NotesStore: Send + Sync {
    /// Read the raw annotation document; absent is empty.
    fn read(&self, root: &Path) -> io::Result<Vec<u8>>;
    /// Write the raw annotation document.
    fn write(&self, root: &Path, bytes: &[u8]) -> io::Result<()>;
}

/// Default file-backed store.
pub(crate) struct FileNotesStore;

impl NotesStore for FileNotesStore {
    fn read(&self, root: &Path) -> io::Result<Vec<u8>> {
        match std::fs::read(crate::notes::Notes::path(root)) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    fn write(&self, root: &Path, bytes: &[u8]) -> io::Result<()> {
        let path = crate::notes::Notes::path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)
    }
}

/// Store used when the first-party annotations package is disabled.
pub(crate) struct DisabledNotesStore;

impl NotesStore for DisabledNotesStore {
    fn read(&self, _root: &Path) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn write(&self, _root: &Path, _bytes: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the annotations extension is disabled",
        ))
    }
}

static STORE: OnceLock<RwLock<Arc<dyn NotesStore>>> = OnceLock::new();

fn slot() -> &'static RwLock<Arc<dyn NotesStore>> {
    STORE.get_or_init(|| RwLock::new(Arc::new(FileNotesStore)))
}

/// The active notes store.
pub(crate) fn store() -> Arc<dyn NotesStore> {
    slot()
        .read()
        .map(|guard| Arc::clone(&guard))
        .unwrap_or_else(|_| Arc::new(FileNotesStore))
}

/// Replace the store, used by the first-party annotations provider.
pub(crate) fn set_store(store: Arc<dyn NotesStore>) {
    if let Ok(mut guard) = slot().write() {
        *guard = store;
    }
}
