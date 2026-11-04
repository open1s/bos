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

/// The largest total size a namespace may hold.
pub const MAX_NAMESPACE_BYTES: u64 = 64 * 1024 * 1024;

/// A handle to one extension's data namespace.
#[derive(Debug, Clone)]
pub struct ExtensionStorage {
    root: PathBuf,
    id: ExtensionId,
    max_bytes: u64,
    /// An open handle to the namespace root. Operations are anchored to it so
    /// a path component swapped after validation cannot redirect a read or
    /// write outside the namespace.
    #[cfg(unix)]
    root_fd: std::sync::Arc<std::os::fd::OwnedFd>,
}

impl ExtensionStorage {
    /// Open the namespace for `id` below `data_root`.
    pub fn open(data_root: &Path, id: &ExtensionId) -> Result<Self, ExtensionError> {
        std::fs::create_dir_all(data_root).map_err(|source| ExtensionError::Io {
            path: data_root.to_path_buf(),
            source,
        })?;
        let canonical_data = data_root
            .canonicalize()
            .map_err(|source| ExtensionError::Io {
                path: data_root.to_path_buf(),
                source,
            })?;
        let root = data_root.join(id.as_str());
        // Reject a namespace root that is itself a symlink before anything
        // creates or writes through it; a planted link would escape the data
        // root despite every key-level check.
        match std::fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ExtensionError::Corrupt {
                    path: root,
                    message: "the storage namespace root is a symlink".to_string(),
                });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(ExtensionError::Corrupt {
                    path: root,
                    message: "the storage namespace root is not a directory".to_string(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&root).map_err(|source| ExtensionError::Io {
                    path: root.clone(),
                    source,
                })?;
            }
            Err(source) => {
                return Err(ExtensionError::Io { path: root, source });
            }
        }
        let canonical_root = root.canonicalize().map_err(|source| ExtensionError::Io {
            path: root.clone(),
            source,
        })?;
        if !canonical_root.starts_with(&canonical_data) {
            return Err(ExtensionError::Corrupt {
                path: root,
                message: "the storage namespace escapes the data root".to_string(),
            });
        }
        #[cfg(unix)]
        let root_fd = {
            use rustix::fs::{open, Mode, OFlags};
            let fd = open(
                &canonical_root,
                OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| ExtensionError::Io {
                path: root.clone(),
                source: std::io::Error::from_raw_os_error(error.raw_os_error()),
            })?;
            std::sync::Arc::new(fd)
        };
        Ok(Self {
            root,
            id: id.clone(),
            max_bytes: MAX_NAMESPACE_BYTES,
            #[cfg(unix)]
            root_fd,
        })
    }

    /// Override the namespace size ceiling (tests only).
    #[cfg(test)]
    fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
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

    /// Walk a validated key from the root handle and return the parent
    /// directory handle together with the final component. Intermediate
    /// components are opened with O_NOFOLLOW, so a symlink planted after the
    /// lexical check cannot be followed.
    #[cfg(unix)]
    fn anchored_parent(
        &self,
        key: &str,
        create: bool,
    ) -> Result<(std::os::fd::OwnedFd, std::ffi::OsString), ExtensionError> {
        use rustix::fs::{mkdirat, openat, Mode, OFlags};
        let relative = validate_relative_path(key).map_err(|reason| ExtensionError::Corrupt {
            path: self.root.join(key),
            message: reason,
        })?;
        let mut current = openat(
            &*self.root_fd,
            ".",
            OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| self.io_error(&self.root, error))?;
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let name = component.as_os_str();
            if components.peek().is_none() {
                return Ok((current, name.to_os_string()));
            }
            current = match openat(
                &current,
                name,
                OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(error) if create && error == rustix::io::Errno::NOENT => {
                    mkdirat(&current, name, Mode::from_raw_mode(0o755))
                        .map_err(|error| self.io_error(&self.root, error))?;
                    openat(
                        &current,
                        name,
                        OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|error| self.io_error(&self.root, error))?
                }
                Err(error) => return Err(self.io_error(&self.root, error)),
            };
        }
        Err(ExtensionError::Corrupt {
            path: self.root.clone(),
            message: "empty storage key".to_string(),
        })
    }

    #[cfg(unix)]
    fn io_error(&self, path: &Path, error: rustix::io::Errno) -> ExtensionError {
        ExtensionError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::from_raw_os_error(error.raw_os_error()),
        }
    }

    /// Write through the anchored root: temp file in the parent, fsync, then
    /// renameat within that same directory handle.
    #[cfg(unix)]
    fn write_anchored(&self, key: &str, bytes: &[u8]) -> Result<(), ExtensionError> {
        use rustix::fs::{openat, renameat, unlinkat, AtFlags, Mode, OFlags};
        use std::io::Write as _;

        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let (parent, leaf) = self.anchored_parent(key, true)?;
        let temp = format!(
            ".{}.tmp-{}-{}",
            leaf.to_string_lossy(),
            std::process::id(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let fd = openat(
            &parent,
            temp.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|error| self.io_error(&self.root.join(key), error))?;
        {
            let mut file = std::fs::File::from(fd);
            file.write_all(bytes).map_err(|source| ExtensionError::Io {
                path: self.root.join(key),
                source,
            })?;
            file.sync_all().map_err(|source| ExtensionError::Io {
                path: self.root.join(key),
                source,
            })?;
        }
        if let Err(error) = renameat(&parent, temp.as_str(), &parent, &leaf) {
            let _ = unlinkat(&parent, temp.as_str(), AtFlags::empty());
            return Err(self.io_error(&self.root.join(key), error));
        }
        Ok(())
    }

    /// Read a value, returning None when it does not exist.
    pub fn read(&self, key: &str) -> Result<Option<Vec<u8>>, ExtensionError> {
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags};
            use std::io::Read as _;

            // The lexical/existence check still runs (it also creates the
            // parent directories read paths historically expected), but the
            // actual open is anchored to the root handle with O_NOFOLLOW.
            let _ = self.path(key)?;
            let (parent, leaf) = self.anchored_parent(key, false)?;
            let fd = match rustix::fs::openat(
                &parent,
                &leaf,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                Err(error) => return Err(self.io_error(&self.root.join(key), error)),
            };
            let metadata = rustix::fs::fstat(&fd)
                .map_err(|error| self.io_error(&self.root.join(key), error))?;
            if metadata.st_size > MAX_VALUE_BYTES as i64 {
                return Err(ExtensionError::Limit {
                    extension: self.id.to_string(),
                    message: format!(
                        "stored value {key:?} is {} bytes; the ceiling is {MAX_VALUE_BYTES}",
                        metadata.st_size
                    ),
                });
            }
            let mut bytes = Vec::new();
            std::fs::File::from(fd)
                .read_to_end(&mut bytes)
                .map_err(|source| ExtensionError::Io {
                    path: self.root.join(key),
                    source,
                })?;
            Ok(Some(bytes))
        }
        #[cfg(not(unix))]
        {
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
            let bytes =
                std::fs::read(&path).map_err(|source| ExtensionError::Io { path, source })?;
            Ok(Some(bytes))
        }
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
        let existing = std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let delta = (bytes.len() as u64).saturating_sub(existing);
        if delta > 0 && self.namespace_bytes().saturating_add(delta) > self.max_bytes {
            return Err(ExtensionError::Limit {
                extension: self.id.to_string(),
                message: format!("namespace would exceed its {} byte budget", self.max_bytes),
            });
        }
        #[cfg(unix)]
        {
            self.write_anchored(key, bytes)?;
            Ok(())
        }
        #[cfg(not(unix))]
        super::package::write_atomic(&path, bytes)
    }

    /// The total bytes stored in the namespace.
    fn namespace_bytes(&self) -> u64 {
        let mut total = 0u64;
        for entry in walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                if let Ok(metadata) = entry.metadata() {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
        total
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
        #[cfg(unix)]
        {
            let path = self.path(key)?;
            if !path.exists() {
                return Ok(false);
            }
            let (parent, leaf) = self.anchored_parent(key, false)?;
            match rustix::fs::unlinkat(&parent, &leaf, rustix::fs::AtFlags::empty()) {
                Ok(()) => Ok(true),
                Err(error) if error == rustix::io::Errno::NOENT => Ok(false),
                Err(error) => Err(self.io_error(&path, error)),
            }
        }
        #[cfg(not(unix))]
        {
            let path = self.path(key)?;
            if !path.exists() {
                return Ok(false);
            }
            std::fs::remove_file(&path).map_err(|source| ExtensionError::Io { path, source })?;
            Ok(true)
        }
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

    #[cfg(unix)]
    #[test]
    fn a_symlinked_namespace_root_is_refused() {
        let scratch = Scratch::new("symlink-root");
        let outside = scratch.0.with_extension("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        let id = ExtensionId::parse("com.example.storage").expect("id");
        std::os::unix::fs::symlink(&outside, scratch.0.join(id.as_str())).expect("symlink");
        let error = ExtensionStorage::open(&scratch.0, &id).expect_err("symlinked root");
        match error {
            ExtensionError::Corrupt { message, .. } => {
                assert!(message.contains("symlink"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn the_namespace_byte_budget_is_enforced() {
        let scratch = Scratch::new("quota");
        let storage = storage(&scratch).with_max_bytes(8);
        storage.write("a.bin", b"12345678").expect("at quota");
        assert!(storage.write("b.bin", b"9").is_err(), "over quota");
        // Replacing a value within the budget is allowed.
        storage.write("a.bin", b"1234").expect("shrinks");
        storage.write("a.bin", b"12345678").expect("restores");
    }

    #[cfg(unix)]
    #[test]
    fn the_anchored_walk_refuses_a_swapped_parent_symlink() {
        let scratch = Scratch::new("anchored-symlink");
        let storage = storage(&scratch);
        std::fs::create_dir_all(storage.root().join("sub")).expect("sub");
        std::fs::remove_dir_all(storage.root().join("sub")).expect("remove sub");
        let outside = scratch.0.with_extension("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        std::os::unix::fs::symlink(&outside, storage.root().join("sub")).expect("symlink");
        assert!(
            storage.anchored_parent("sub/value", false).is_err(),
            "the anchored walk must not follow a swapped parent symlink"
        );
    }
}
