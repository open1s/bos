//! Per-extension storage: one namespace per id, path-contained, size-capped
//! and separate from package code.
//!
//! The root is `extension-data/<id>/`. A key is a package-relative path, so a
//! package can name `config/settings.json` but never `../another-extension`.
//! The canonical parent is checked after creation, which catches a symlink
//! planted inside the namespace as well as a textual traversal.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest::validate_relative_path;
use super::types::{ExtensionError, ExtensionId};

/// The largest single value a package may store.
pub const MAX_VALUE_BYTES: usize = 8 * 1024 * 1024;

/// The largest number of files a namespace may hold.
pub const MAX_FILES: usize = 10_000;

/// A handle to one extension's data namespace.
#[derive(Debug, Clone)]
pub struct ExtensionStorage {
    root: PathBuf,
    id: ExtensionId,
}

impl ExtensionStorage {
    /// Open the namespace for `id` below `data_root`.
    pub fn open(data_root: &Path, id: &ExtensionId) -> Result<Self, ExtensionError> {
        let root = data_root.join(id.as_str());
        std::fs::create_dir_all(&root).map_err(|source| ExtensionError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(Self {
            root,
            id: id.clone(),
        })
    }

    /// The namespace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The extension id.
    pub fn id(&self) -> &ExtensionId {
        &self.id
    }

    /// Resolve a key to a path, refusing traversal and symlinked parents.
    pub fn path(&self, key: &str) -> Result<PathBuf, ExtensionError> {
        let relative = validate_relative_path(key).map_err(|reason| ExtensionError::Corrupt {
            path: self.root.join(key),
            message: reason,
        })?;
        let path = self.root.join(&relative);
        // Walk the parent one component at a time from the namespace root:
        // creating the whole parent first would follow a symlink planted inside
        // the namespace before the containment check could refuse it.
        let mut cursor = self.root.clone();
        if let Some(parent) = relative.parent() {
            for component in parent.components() {
                cursor.push(component);
                match std::fs::symlink_metadata(&cursor) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(ExtensionError::Corrupt {
                            path: cursor,
                            message: "key escapes the extension data namespace".to_string(),
                        });
                    }
                    Ok(metadata) if metadata.is_dir() => {}
                    Ok(_) => {
                        return Err(ExtensionError::Corrupt {
                            path: cursor,
                            message: "a parent component is not a directory".to_string(),
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        std::fs::create_dir(&cursor).map_err(|source| ExtensionError::Io {
                            path: cursor.clone(),
                            source,
                        })?;
                    }
                    Err(source) => {
                        return Err(ExtensionError::Io {
                            path: cursor,
                            source,
                        });
                    }
                }
            }
        }
        if path
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(ExtensionError::Corrupt {
                path,
                message: "stored values cannot be symbolic links".to_string(),
            });
        }
        Ok(path)
    }

    /// Read a value, returning None when it does not exist.
    pub fn read(&self, key: &str) -> Result<Option<Vec<u8>>, ExtensionError> {
        let path = self.path(key)?;
        if !path.is_file() {
            return Ok(None);
        }
        let metadata = std::fs::metadata(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_VALUE_BYTES as u64 {
            return Err(ExtensionError::Limit {
                extension: self.id.to_string(),
                message: format!(
                    "stored value {key:?} is {} bytes; the ceiling is {MAX_VALUE_BYTES}",
                    metadata.len()
                ),
            });
        }
        let bytes = std::fs::read(&path).map_err(|source| ExtensionError::Io { path, source })?;
        Ok(Some(bytes))
    }

    /// Read a UTF-8 value.
    pub fn read_text(&self, key: &str) -> Result<Option<String>, ExtensionError> {
        match self.read(key)? {
            None => Ok(None),
            Some(bytes) => {
                String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|error| ExtensionError::Corrupt {
                        path: self.root.join(key),
                        message: format!("stored value is not UTF-8: {error}"),
                    })
            }
        }
    }

    /// Write a value atomically.
    pub fn write(&self, key: &str, bytes: &[u8]) -> Result<(), ExtensionError> {
        if bytes.len() > MAX_VALUE_BYTES {
            return Err(ExtensionError::Limit {
                extension: self.id.to_string(),
                message: format!(
                    "value {key:?} is {} bytes; the ceiling is {MAX_VALUE_BYTES}",
                    bytes.len()
                ),
            });
        }
        let path = self.path(key)?;
        // The namespace cap has to guard the write, not only the listing: a
        // guest that never lists could otherwise grow the namespace without
        // bound and only discover the limit when listing breaks.
        if !path.exists() && self.at_file_limit() {
            return Err(ExtensionError::Limit {
                extension: self.id.to_string(),
                message: format!("namespace holds more than {MAX_FILES} files"),
            });
        }
        super::package::write_atomic(&path, bytes)
    }

    /// Whether the namespace already holds the maximum number of files.
    fn at_file_limit(&self) -> bool {
        let mut count = 0usize;
        for entry in walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                count += 1;
                if count >= MAX_FILES {
                    return true;
                }
            }
        }
        false
    }

    /// Write a JSON value.
    pub fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<(), ExtensionError> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| ExtensionError::Json {
            path: self.root.join(key),
            message: error.to_string(),
        })?;
        self.write(key, &bytes)
    }

    /// Read and parse a JSON value.
    pub fn read_json<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>, ExtensionError> {
        match self.read(key)? {
            None => Ok(None),
            Some(bytes) => {
                serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|error| ExtensionError::Json {
                        path: self.root.join(key),
                        message: error.to_string(),
                    })
            }
        }
    }

    /// Remove a value.
    pub fn remove(&self, key: &str) -> Result<bool, ExtensionError> {
        let path = self.path(key)?;
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).map_err(|source| ExtensionError::Io { path, source })?;
        Ok(true)
    }

    /// List stored keys, sorted.
    pub fn list(&self) -> Result<Vec<String>, ExtensionError> {
        let mut keys = Vec::new();
        for entry in walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry.map_err(|error| ExtensionError::Io {
                path: error.path().unwrap_or(&self.root).to_path_buf(),
                source: std::io::Error::other(error.to_string()),
            })?;
            if entry.depth() == 0 || !entry.file_type().is_file() {
                continue;
            }
            if keys.len() >= MAX_FILES {
                return Err(ExtensionError::Limit {
                    extension: self.id.to_string(),
                    message: format!("namespace holds more than {MAX_FILES} files"),
                });
            }
            let relative = entry
                .path()
                .strip_prefix(&self.root)
                .unwrap_or(entry.path());
            let key = relative
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/");
            keys.push(key);
        }
        Ok(keys)
    }

    /// Total bytes stored.
    pub fn size(&self) -> u64 {
        walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum()
    }

    /// Remove the whole namespace.
    pub fn purge(&self) -> Result<(), ExtensionError> {
        if self.root.exists() {
            std::fs::remove_dir_all(&self.root).map_err(|source| ExtensionError::Io {
                path: self.root.clone(),
                source,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let id = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bsh-ext-storage-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn storage(scratch: &Scratch) -> ExtensionStorage {
        let id = ExtensionId::parse("com.example.storage").expect("id");
        ExtensionStorage::open(&scratch.0, &id).expect("storage")
    }

    #[test]
    fn values_round_trip_and_list() {
        let scratch = Scratch::new("roundtrip");
        let storage = storage(&scratch);
        storage.write("config/a.json", b"{}").expect("write");
        storage
            .write_json("config/b.json", &serde_json::json!({"n": 1}))
            .expect("write json");
        assert_eq!(
            storage.read_text("config/a.json").expect("read"),
            Some("{}".to_string())
        );
        let value: Option<serde_json::Value> =
            storage.read_json("config/b.json").expect("read json");
        assert_eq!(value.expect("value")["n"], 1);
        assert_eq!(
            storage.list().expect("list"),
            vec!["config/a.json", "config/b.json"]
        );
        assert!(storage.remove("config/a.json").expect("remove"));
        assert!(!storage.remove("config/a.json").expect("remove again"));
    }

    #[test]
    fn traversal_is_refused() {
        let scratch = Scratch::new("traversal");
        let storage = storage(&scratch);
        for key in ["../escape", "a/../../escape", "/etc/passwd", ""] {
            assert!(storage.path(key).is_err(), "{key:?}");
        }
    }

    #[test]
    fn a_symlinked_value_is_refused() {
        #[cfg(unix)]
        {
            let scratch = Scratch::new("symlink");
            let storage = storage(&scratch);
            let target = scratch.0.join("outside");
            std::fs::write(&target, b"secret").expect("target");
            std::os::unix::fs::symlink(&target, storage.root().join("link.json")).expect("symlink");
            assert!(storage.read("link.json").is_err());
        }
    }

    #[test]
    fn write_ceiling_is_enforced() {
        let scratch = Scratch::new("limit");
        let storage = storage(&scratch);
        let bytes = vec![0u8; MAX_VALUE_BYTES + 1];
        assert!(storage.write("big.bin", &bytes).is_err());
    }

    #[test]
    fn purge_removes_the_namespace() {
        let scratch = Scratch::new("purge");
        let storage = storage(&scratch);
        storage.write("a.txt", b"a").expect("write");
        storage.purge().expect("purge");
        assert!(!storage.root().exists());
    }
}
