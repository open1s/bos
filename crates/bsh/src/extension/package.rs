//! Package discovery, integrity, the managed store and atomic installation.
//!
//! A package is a directory with an `extension.toml`. The store owns four
//! separate roots so code, data, state and logs survive each other's upgrades:
//!
//! - `installed/<id>/<version>/` - immutable package code and assets;
//! - `extension-data/<id>/` - the package's business data;
//! - `extension-state/` - enabled flags, grants, registry and lockfile;
//! - `logs/extensions/` - audit and runtime logs.
//!
//! Installation copies into `staging/`, hashes what was copied, then renames
//! into place. A failed install leaves the staging directory for inspection and
//! never half-populates an installed version.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::capability::unix_now;
use super::manifest::{Manifest, MANIFEST_FILE};
use super::signing::{PackageSignature, SignatureVerification, TrustStore, SIGNATURE_FILE};
use super::types::{ExtensionError, ExtensionId, ExtensionSource, TrustLevel, ValidationReport};

/// The largest package tree the installer will copy or hash.
pub(crate) const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;

/// The largest file count the installer will copy or hash.
pub(crate) const MAX_PACKAGE_FILES: usize = 10_000;

/// Directory names that are never part of a package.
const IGNORED_DIRECTORIES: [&str; 4] = [".git", "target", "node_modules", ".jj"];

/// One loaded package and the facts the host established about it.
#[derive(Debug, Clone)]
pub(crate) struct Package {
    /// Absolute package root.
    pub(crate) root: PathBuf,
    /// Parsed manifest.
    pub(crate) manifest: Manifest,
    /// Validation result; a package with errors is listed but never activated.
    pub(crate) report: ValidationReport,
    /// Where the package came from.
    pub(crate) source: ExtensionSource,
    /// How much the host trusts it.
    pub(crate) trust: TrustLevel,
    /// Canonical SHA-256 of the package tree, empty for a built-in.
    pub(crate) hash: String,
    /// Unix seconds the package was installed, zero when unknown.
    pub(crate) installed_at: u64,
    /// Signature metadata shipped with the package, if any.
    pub(crate) signature: Option<PackageSignature>,
    /// The trust store's verdict on that signature.
    pub(crate) verification: SignatureVerification,
    /// The directory or archive this install was copied from, when known.
    pub(crate) source_path: Option<PathBuf>,
}

impl Package {
    /// Load the package at `root`.
    pub(crate) fn load(
        root: &Path,
        source: ExtensionSource,
        trust: TrustLevel,
    ) -> Result<Self, ExtensionError> {
        let manifest = Manifest::load(root)?;
        let report = manifest.validate();
        let signature = PackageSignature::load(root)?;
        let hash = if report.is_valid() {
            package_hash(root)?
        } else {
            String::new()
        };
        let installed_at = std::fs::metadata(root)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let source_path =
            matches!(&source, ExtensionSource::Development).then(|| root.to_path_buf());
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            report,
            source,
            trust,
            hash,
            installed_at,
            signature,
            verification: SignatureVerification::Missing,
            source_path,
        })
    }

    /// Load a first-party package whose manifest is compiled into the binary.
    pub(crate) fn builtin(manifest_toml: &str) -> Result<Self, ExtensionError> {
        let origin = PathBuf::from("<builtin>").join(MANIFEST_FILE);
        let manifest = Manifest::parse(manifest_toml, &origin)?;
        let report = manifest.validate();
        Ok(Self {
            root: PathBuf::from("<builtin>"),
            manifest,
            report,
            source: ExtensionSource::Builtin,
            trust: TrustLevel::Builtin,
            hash: String::new(),
            installed_at: 0,
            signature: None,
            verification: SignatureVerification::Missing,
            source_path: None,
        })
    }

    /// The extension id.
    pub(crate) fn id(&self) -> &ExtensionId {
        self.manifest.id()
    }

    /// The package version as text.
    pub(crate) fn version(&self) -> String {
        self.manifest.extension.version.to_string()
    }

    /// Whether the package may be activated.
    pub(crate) fn is_valid(&self) -> bool {
        self.report.is_valid()
    }

    /// The validation errors joined into one sentence, for a status row.
    pub(crate) fn error_message(&self) -> String {
        self.report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Hash a package tree deterministically with SHA-256.
///
/// The digest covers each file's relative path, byte length and content
/// digest, in sorted path order. Directory metadata, mtimes and modes are
/// deliberately excluded: two copies of the same package with different
/// timestamps must hash the same.
pub(crate) fn package_hash(root: &Path) -> Result<String, ExtensionError> {
    let mut hasher = Sha256::new();
    let mut files = 0usize;
    let mut bytes = 0u64;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_ignored(entry.path()))
    {
        let entry = entry.map_err(|error| ExtensionError::Io {
            path: error.path().unwrap_or(root).to_path_buf(),
            source: std::io::Error::other(error.to_string()),
        })?;
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(ExtensionError::Limit {
                extension: root.display().to_string(),
                message: format!(
                    "package contains a symbolic link at {}; packages must be self-contained",
                    entry.path().display()
                ),
            });
        }
        if is_ignored(entry.path()) || entry.file_name().to_str() == Some(SIGNATURE_FILE) {
            // The signature covers the tree, so it cannot be part of the hash
            // it signs; copy_tree still installs it.
            continue;
        }
        files += 1;
        if files > MAX_PACKAGE_FILES {
            return Err(ExtensionError::Limit {
                extension: root.display().to_string(),
                message: format!("package contains more than {MAX_PACKAGE_FILES} files"),
            });
        }
        let relative = relative_path(root, entry.path());
        let mut file = std::fs::File::open(entry.path()).map_err(|source| ExtensionError::Io {
            path: entry.path().to_path_buf(),
            source,
        })?;
        let mut file_hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut file_bytes = 0u64;
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|source| ExtensionError::Io {
                    path: entry.path().to_path_buf(),
                    source,
                })?;
            if read == 0 {
                break;
            }
            file_bytes += read as u64;
            bytes += read as u64;
            if bytes > MAX_PACKAGE_BYTES {
                return Err(ExtensionError::Limit {
                    extension: root.display().to_string(),
                    message: format!("package is larger than {MAX_PACKAGE_BYTES} bytes"),
                });
            }
            file_hasher.update(&buffer[..read]);
        }
        let digest = file_hasher.finalize();
        hasher.update(relative.as_bytes());
        hasher.update([0u8]);
        hasher.update(file_bytes.to_le_bytes());
        hasher.update(digest);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn is_ignored(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    name == ".DS_Store"
        || name.ends_with(".tmp")
        || (path.is_dir() && IGNORED_DIRECTORIES.contains(&name))
}

fn relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Write a file atomically: a sibling temp file, fsync, rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ExtensionError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ExtensionError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("document");
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary).map_err(|source| ExtensionError::Io {
            path: temporary.clone(),
            source,
        })?;
        file.write_all(bytes).map_err(|source| ExtensionError::Io {
            path: temporary.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| ExtensionError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    #[cfg(windows)]
    let _ = std::fs::remove_file(path);
    std::fs::rename(&temporary, path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Copy a package tree, refusing symlinks and enforcing the same caps the hash
/// does. Existing files are replaced, so this is only safe into an empty
/// staging directory.
pub(crate) fn copy_tree(source: &Path, destination: &Path) -> Result<(), ExtensionError> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    for entry in WalkDir::new(source)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_ignored(entry.path()))
    {
        let entry = entry.map_err(|error| ExtensionError::Io {
            path: error.path().unwrap_or(source).to_path_buf(),
            source: std::io::Error::other(error.to_string()),
        })?;
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(ExtensionError::Limit {
                extension: source.display().to_string(),
                message: format!(
                    "package contains a symbolic link at {}; it was not installed",
                    entry.path().display()
                ),
            });
        }
        let relative = entry
            .path()
            .strip_prefix(source)
            .unwrap_or(entry.path())
            .to_path_buf();
        let target = destination.join(&relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).map_err(|error| ExtensionError::Io {
                path: target.clone(),
                source: error,
            })?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| ExtensionError::Io {
                path: parent.to_path_buf(),
                source: error,
            })?;
        }
        let copied = std::fs::copy(entry.path(), &target).map_err(|error| ExtensionError::Io {
            path: target.clone(),
            source: error,
        })?;
        files += 1;
        bytes += copied;
        if files > MAX_PACKAGE_FILES {
            return Err(ExtensionError::Limit {
                extension: source.display().to_string(),
                message: format!("package contains more than {MAX_PACKAGE_FILES} files"),
            });
        }
        if bytes > MAX_PACKAGE_BYTES {
            return Err(ExtensionError::Limit {
                extension: source.display().to_string(),
                message: format!("package is larger than {MAX_PACKAGE_BYTES} bytes"),
            });
        }
    }
    Ok(())
}

/// The persisted per-extension host state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExtensionState {
    /// Whether the reader left the extension enabled.
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    /// The last lifecycle state the host recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_state: Option<String>,
    /// The last failure, kept across launches until a retry succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_error: Option<String>,
    /// Unix seconds of the last successful start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_started: Option<u64>,
    /// Unix seconds the version was installed.
    #[serde(default)]
    pub(crate) installed_at: u64,
    /// The exact version the host resolves for this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_version: Option<String>,
    /// The data schema version the migrations last produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) data_version: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for ExtensionState {
    fn default() -> Self {
        Self {
            enabled: true,
            last_state: None,
            last_error: None,
            last_started: None,
            installed_at: 0,
            active_version: None,
            data_version: None,
        }
    }
}

/// The persisted state document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StateFile {
    /// Format generation.
    #[serde(default = "state_format")]
    pub(crate) format: u32,
    /// extension id -> state.
    #[serde(default)]
    pub(crate) extensions: BTreeMap<String, ExtensionState>,
}

fn state_format() -> u32 {
    1
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            format: state_format(),
            extensions: BTreeMap::new(),
        }
    }
}

/// One installed version's registry record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InstalledRecord {
    /// Package-relative path below the store root.
    pub(crate) path: String,
    /// Canonical SHA-256 recorded at install time.
    pub(crate) hash: String,
    /// Where the package came from.
    #[serde(default)]
    pub(crate) source: ExtensionSource,
    /// Trust established at install time.
    #[serde(default = "default_trust")]
    pub(crate) trust: TrustLevel,
    /// Unix seconds of installation.
    #[serde(default)]
    pub(crate) installed_at: u64,
    /// Dependency ranges the manifest declared.
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, String>,
    /// The trust store's verdict on the package signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verification: Option<SignatureVerification>,
    /// Publisher name from the signature, when one was present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) publisher: Option<String>,
    /// The directory or archive this install was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_path: Option<String>,
}

fn default_trust() -> TrustLevel {
    TrustLevel::Local
}

/// The persisted installed-package index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RegistryFile {
    /// Format generation.
    #[serde(default = "state_format")]
    pub(crate) format: u32,
    /// extension id -> version -> record.
    #[serde(default)]
    pub(crate) extensions: BTreeMap<String, BTreeMap<String, InstalledRecord>>,
}

impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            format: state_format(),
            extensions: BTreeMap::new(),
        }
    }
}

impl RegistryFile {
    /// Look up one installed version.
    pub(crate) fn record(&self, id: &str, version: &str) -> Option<&InstalledRecord> {
        self.extensions
            .get(id)
            .and_then(|versions| versions.get(version))
    }

    /// Insert one record.
    pub(crate) fn insert(&mut self, id: &str, version: &str, record: InstalledRecord) {
        self.extensions
            .entry(id.to_string())
            .or_default()
            .insert(version.to_string(), record);
    }

    /// Remove every version of one extension.
    pub(crate) fn remove(&mut self, id: &str) -> bool {
        self.extensions.remove(id).is_some()
    }
}

/// One package found by a store scan that could not be loaded.
#[derive(Debug, Clone)]
pub(crate) struct ScanFailure {
    /// The directory that failed.
    pub(crate) path: PathBuf,
    /// Why it failed.
    pub(crate) error: String,
}

/// The managed extension store.
#[derive(Debug, Clone)]
pub(crate) struct ExtensionStore {
    root: PathBuf,
}

impl ExtensionStore {
    /// Open (and create) the store at `root`.
    pub(crate) fn open(root: impl Into<PathBuf>) -> Result<Self, ExtensionError> {
        let store = Self { root: root.into() };
        for directory in [
            store.installed_root(),
            store.staging_root(),
            store.cache_root(),
            store.quarantine_root(),
            store.data_root(),
            store.state_root(),
            store.log_root(),
        ] {
            std::fs::create_dir_all(&directory).map_err(|source| ExtensionError::Io {
                path: directory,
                source,
            })?;
        }
        Ok(store)
    }

    /// The store root.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// `installed/`.
    pub(crate) fn installed_root(&self) -> PathBuf {
        self.root.join("installed")
    }

    /// `staging/`.
    pub(crate) fn staging_root(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// `cache/`.
    pub(crate) fn cache_root(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `quarantine/`.
    pub(crate) fn quarantine_root(&self) -> PathBuf {
        self.root.join("quarantine")
    }

    /// `extension-data/`.
    pub(crate) fn data_root(&self) -> PathBuf {
        self.root.join("extension-data")
    }

    /// `extension-state/`.
    pub(crate) fn state_root(&self) -> PathBuf {
        self.root.join("extension-state")
    }

    /// `logs/extensions/`.
    pub(crate) fn log_root(&self) -> PathBuf {
        self.root.join("logs").join("extensions")
    }

    /// The installed registry document.
    pub(crate) fn registry_path(&self) -> PathBuf {
        self.state_root().join("registry.toml")
    }

    /// The dependency lockfile.
    pub(crate) fn lock_path(&self) -> PathBuf {
        self.state_root().join("lock.toml")
    }

    /// The enabled/failure state document.
    pub(crate) fn state_path(&self) -> PathBuf {
        self.state_root().join("state.toml")
    }

    /// The grant document.
    pub(crate) fn grants_path(&self) -> PathBuf {
        self.state_root().join("grants.toml")
    }

    /// The publisher trust document.
    pub(crate) fn trust_path(&self) -> PathBuf {
        self.state_root().join("trust.toml")
    }

    /// The capability audit log.
    pub(crate) fn audit_path(&self) -> PathBuf {
        self.log_root().join("audit.jsonl")
    }

    /// The retained load-crash history.
    pub(crate) fn crashes_path(&self) -> PathBuf {
        self.log_root().join("crashes.jsonl")
    }

    /// Append one load failure unless it repeats the last recorded line.
    pub(crate) fn record_crash(&self, id: &str, error: &str) {
        let path = self.crashes_path();
        let line = format!("{}\t{id}\t{error}\n", crate::settings::unix_now());
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if existing.lines().last() == Some(line.trim_end()) {
                return;
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// The last `limit` recorded load failures, newest last.
    pub(crate) fn crash_history(&self, limit: usize) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.crashes_path()) else {
            return Vec::new();
        };
        let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
        lines[lines.len().saturating_sub(limit)..]
            .iter()
            .map(|line| line.to_string())
            .collect()
    }

    /// One extension's log directory.
    pub(crate) fn log_dir(&self, id: &ExtensionId) -> PathBuf {
        self.log_root().join(id.as_str())
    }

    /// The directory holding one installed version.
    pub(crate) fn version_root(&self, id: &ExtensionId, version: &str) -> PathBuf {
        self.installed_root().join(id.as_str()).join(version)
    }

    /// The data directory of one extension.
    pub(crate) fn data_dir(&self, id: &ExtensionId) -> PathBuf {
        self.data_root().join(id.as_str())
    }

    /// Read the registry, returning an empty one when absent.
    pub(crate) fn read_registry(&self) -> Result<RegistryFile, ExtensionError> {
        let path = self.registry_path();
        if !path.is_file() {
            return Ok(RegistryFile::default());
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&text).map_err(|error| ExtensionError::Corrupt {
            path,
            message: error.to_string(),
        })
    }

    /// Write the registry with the current format.
    pub(crate) fn write_registry(&self, registry: &RegistryFile) -> Result<(), ExtensionError> {
        let text = toml::to_string_pretty(registry).map_err(|error| ExtensionError::Corrupt {
            path: self.registry_path(),
            message: error.to_string(),
        })?;
        write_atomic(&self.registry_path(), text.as_bytes())
    }

    /// Read the state document, returning defaults when absent.
    pub(crate) fn read_state(&self) -> Result<StateFile, ExtensionError> {
        let path = self.state_path();
        if !path.is_file() {
            return Ok(StateFile::default());
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&text).map_err(|error| ExtensionError::Corrupt {
            path,
            message: error.to_string(),
        })
    }

    /// Write the state document.
    pub(crate) fn write_state(&self, state: &StateFile) -> Result<(), ExtensionError> {
        let text = toml::to_string_pretty(state).map_err(|error| ExtensionError::Corrupt {
            path: self.state_path(),
            message: error.to_string(),
        })?;
        write_atomic(&self.state_path(), text.as_bytes())
    }

    /// The state record for one extension, with the default of enabled.
    pub(crate) fn state_of(&self, id: &ExtensionId) -> ExtensionState {
        self.read_state()
            .ok()
            .and_then(|state| state.extensions.get(id.as_str()).cloned())
            .unwrap_or_default()
    }

    /// Persist one extension's state record.
    pub(crate) fn update_state(
        &self,
        id: &ExtensionId,
        update: impl FnOnce(&mut ExtensionState),
    ) -> Result<ExtensionState, ExtensionError> {
        let mut document = self.read_state()?;
        let entry = document
            .extensions
            .entry(id.as_str().to_string())
            .or_default();
        update(entry);
        let snapshot = entry.clone();
        self.write_state(&document)?;
        Ok(snapshot)
    }

    /// Enable or disable one extension.
    pub(crate) fn set_enabled(
        &self,
        id: &ExtensionId,
        enabled: bool,
    ) -> Result<ExtensionState, ExtensionError> {
        self.update_state(id, |state| state.enabled = enabled)
    }

    /// Every installed version of an extension, sorted ascending.
    pub(crate) fn versions(&self, id: &ExtensionId) -> Result<Vec<String>, ExtensionError> {
        let directory = self.installed_root().join(id.as_str());
        if !directory.is_dir() {
            return Err(ExtensionError::NotInstalled { id: id.to_string() });
        }
        let mut versions: Vec<String> = std::fs::read_dir(&directory)
            .map_err(|source| ExtensionError::Io {
                path: directory.clone(),
                source,
            })?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().join(MANIFEST_FILE).is_file())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect();
        versions.sort_by_key(|version| {
            semver::Version::parse(version).unwrap_or_else(|_| semver::Version::new(0, 0, 0))
        });
        Ok(versions)
    }

    /// Pin the version the host resolves for an extension.
    pub(crate) fn set_active(
        &self,
        id: &ExtensionId,
        version: &str,
    ) -> Result<ExtensionState, ExtensionError> {
        if !self.version_root(id, version).join(MANIFEST_FILE).is_file() {
            return Err(ExtensionError::NotInstalled {
                id: format!("{id} {version}"),
            });
        }
        self.update_state(id, |state| {
            state.active_version = Some(version.to_string());
            state.last_error = None;
        })
    }

    /// Record the data schema version migrations produced.
    pub(crate) fn set_data_version(
        &self,
        id: &ExtensionId,
        version: &str,
    ) -> Result<ExtensionState, ExtensionError> {
        self.update_state(id, |state| {
            state.data_version = Some(version.to_string());
        })
    }

    /// The pinned version, if one was recorded.
    pub(crate) fn active_version(&self, id: &ExtensionId) -> Option<String> {
        self.state_of(id).active_version
    }

    /// Move every installed version of an extension into quarantine.
    ///
    /// This is the last-resort control: the code is no longer loadable, the
    /// state record is removed and the data is retained for a later review or
    /// deliberate purge.
    pub(crate) fn quarantine(
        &self,
        id: &ExtensionId,
        reason: &str,
    ) -> Result<PathBuf, ExtensionError> {
        let source = self.installed_root().join(id.as_str());
        if !source.exists() {
            return Err(ExtensionError::NotInstalled { id: id.to_string() });
        }
        let destination = self
            .quarantine_root()
            .join(format!("{}-{}", id.as_str(), unix_now()));
        std::fs::create_dir_all(&destination).map_err(|source| ExtensionError::Io {
            path: destination.clone(),
            source,
        })?;
        std::fs::rename(&source, &destination).map_err(|error| ExtensionError::Io {
            path: source.clone(),
            source: error,
        })?;
        let mut registry = self.read_registry()?;
        if registry.remove(id.as_str()) {
            self.write_registry(&registry)?;
        }
        let mut state = self.read_state()?;
        if state.extensions.remove(id.as_str()).is_some() {
            self.write_state(&state)?;
        }
        let note = destination.join("QUARANTINE.txt");
        let _ = std::fs::write(
            &note,
            format!("quarantined at {}\nreason: {reason}\n", unix_now()),
        );
        Ok(destination)
    }

    /// Scan every installed version.
    pub(crate) fn scan_installed(
        &self,
    ) -> Result<(Vec<Package>, Vec<ScanFailure>), ExtensionError> {
        let registry = self.read_registry()?;
        let mut packages = Vec::new();
        let mut failures = Vec::new();
        let installed = self.installed_root();
        let ids = match std::fs::read_dir(&installed) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok((packages, failures));
            }
            Err(source) => {
                return Err(ExtensionError::Io {
                    path: installed,
                    source,
                });
            }
        };
        let mut id_dirs: Vec<PathBuf> = ids
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        id_dirs.sort();
        for id_dir in id_dirs {
            let versions = match std::fs::read_dir(&id_dir) {
                Ok(entries) => entries,
                Err(error) => {
                    failures.push(ScanFailure {
                        path: id_dir.clone(),
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            let mut version_dirs: Vec<PathBuf> = versions
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.join(MANIFEST_FILE).is_file())
                .collect();
            version_dirs.sort();
            for version_dir in version_dirs {
                let record = version_dir
                    .file_name()
                    .and_then(|value| value.to_str())
                    .and_then(|version| {
                        version_dir
                            .parent()
                            .and_then(|parent| parent.file_name())
                            .and_then(|value| value.to_str())
                            .and_then(|id| registry.record(id, version))
                    })
                    .cloned();
                let source = record
                    .as_ref()
                    .map(|record| record.source.clone())
                    .unwrap_or(ExtensionSource::Installed);
                let trust = record
                    .as_ref()
                    .map(|record| record.trust)
                    .unwrap_or(TrustLevel::Local);
                match Package::load(&version_dir, source, trust) {
                    Ok(mut package) => {
                        if let Some(record) = record {
                            package.installed_at = record.installed_at;
                            package.verification = record
                                .verification
                                .unwrap_or(SignatureVerification::Missing);
                            package.source_path = record.source_path.map(PathBuf::from);
                        }
                        packages.push(package);
                    }
                    Err(error) => failures.push(ScanFailure {
                        path: version_dir,
                        error: error.to_string(),
                    }),
                }
            }
        }
        Ok((packages, failures))
    }

    /// Install a package from a local directory.
    ///
    /// The copy is staged, hashed, and only then renamed into
    /// `installed/<id>/<version>`. A version already present is refused rather
    /// than overwritten, so an upgrade is an explicit uninstall/install pair or
    /// a new version directory.
    pub(crate) fn install(&self, source: &Path) -> Result<Package, ExtensionError> {
        self.install_with(source, ExtensionSource::Installed)
    }

    /// Install a package and record where it came from.
    pub(crate) fn install_with(
        &self,
        source: &Path,
        origin: ExtensionSource,
    ) -> Result<Package, ExtensionError> {
        let staged = Package::load(source, ExtensionSource::Development, TrustLevel::Local)?;
        if !staged.report.is_valid() {
            return Err(ExtensionError::Manifest {
                path: source.join(MANIFEST_FILE),
                message: staged.error_message(),
            });
        }
        let id = staged.id().clone();
        let version = staged.version().to_string();
        let trust = TrustStore::load(self.trust_path())?;
        let verification =
            super::signing::verify_package(source, id.as_str(), &version, &staged.hash, &trust);
        if let super::signing::SignatureVerification::Invalid { reason } = &verification {
            return Err(ExtensionError::Denied {
                extension: id.to_string(),
                capability: "package.signature".to_string(),
                reason: reason.clone(),
            });
        }
        let trust_level = verification.trust_level(TrustLevel::Local);
        let publisher = staged
            .signature
            .as_ref()
            .map(|signature| signature.publisher.clone());
        let destination = self.version_root(&id, &version);
        if destination.exists() {
            return Err(ExtensionError::AlreadyInstalled {
                id: id.to_string(),
                version,
            });
        }
        let name = format!(
            "{}-{}-{}-{}",
            id.as_str(),
            version,
            std::process::id(),
            unix_now()
        );
        let staging = self.staging_root().join(name);
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(|error| ExtensionError::Io {
                path: staging.clone(),
                source: error,
            })?;
        }
        copy_tree(source, &staging)?;
        let hash = package_hash(&staging)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| ExtensionError::Io {
                path: parent.to_path_buf(),
                source: error,
            })?;
        }
        if destination.exists() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ExtensionError::AlreadyInstalled {
                id: id.to_string(),
                version,
            });
        }
        std::fs::rename(&staging, &destination).map_err(|error| ExtensionError::Io {
            path: destination.clone(),
            source: error,
        })?;
        let mut package = Package::load(&destination, origin.clone(), trust_level)?;
        package.hash = hash.clone();
        package.installed_at = unix_now();
        package.verification = verification.clone();
        package.signature = staged.signature.clone();
        package.source_path = Some(source.to_path_buf());
        package.source_path = Some(source.to_path_buf());
        let mut registry = self.read_registry()?;
        registry.insert(
            id.as_str(),
            &version,
            InstalledRecord {
                path: format!("installed/{}/{version}", id.as_str()),
                hash,
                source: origin,
                trust: trust_level,
                installed_at: package.installed_at,
                dependencies: staged
                    .manifest
                    .dependencies
                    .iter()
                    .map(|(id, requirement)| (id.to_string(), requirement.to_string()))
                    .collect(),
                verification: Some(verification),
                publisher,
                source_path: Some(source.display().to_string()),
            },
        );
        self.write_registry(&registry)?;
        self.update_state(&id, |state| {
            state.enabled = true;
            state.installed_at = package.installed_at;
            state.last_error = None;
            state.active_version = Some(version.clone());
            if state.data_version.is_none() {
                state.data_version = Some(version.clone());
            }
        })?;
        Ok(package)
    }

    /// Remove every installed version of `id`.
    ///
    /// Business data is retained unless `purge_data` is true. The state record
    /// is always removed, because the flags describe an installed package and
    /// a later install starts from the defaults.
    pub(crate) fn uninstall(
        &self,
        id: &ExtensionId,
        purge_data: bool,
    ) -> Result<(), ExtensionError> {
        let directory = self.installed_root().join(id.as_str());
        if !directory.exists() {
            return Err(ExtensionError::NotInstalled { id: id.to_string() });
        }
        std::fs::remove_dir_all(&directory).map_err(|source| ExtensionError::Io {
            path: directory,
            source,
        })?;
        let mut registry = self.read_registry()?;
        if registry.remove(id.as_str()) {
            self.write_registry(&registry)?;
        }
        let mut state = self.read_state()?;
        if state.extensions.remove(id.as_str()).is_some() {
            self.write_state(&state)?;
        }
        if purge_data {
            let data = self.data_dir(id);
            if data.exists() {
                std::fs::remove_dir_all(&data)
                    .map_err(|source| ExtensionError::Io { path: data, source })?;
            }
        }
        Ok(())
    }

    /// Record a failure in the state document.
    pub(crate) fn record_failure(
        &self,
        id: &ExtensionId,
        error: &str,
    ) -> Result<(), ExtensionError> {
        self.update_state(id, |state| {
            state.last_error = Some(error.to_string());
            state.last_state = Some("failed".to_string());
        })?;
        Ok(())
    }

    /// Record a successful activation.
    pub(crate) fn record_started(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        self.update_state(id, |state| {
            state.last_error = None;
            state.last_state = Some("active".to_string());
            state.last_started = Some(unix_now());
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn crash_history_is_retained_and_deduplicates_the_last_line() {
        let scratch = Scratch::new("crashes");
        let store = ExtensionStore::open(&scratch.0).expect("store");
        store.record_crash("com.example.bad", "boom");
        store.record_crash("com.example.bad", "boom");
        store.record_crash("com.example.worse", "kaput");
        let history = store.crash_history(16);
        assert_eq!(history.len(), 2);
        assert!(history[0].contains("com.example.bad"));
        assert!(history[1].contains("com.example.worse"));
        drop(store);
        let reopened = ExtensionStore::open(&scratch.0).expect("reopen");
        assert_eq!(reopened.crash_history(16).len(), 2);
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let id = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("bsh-ext-store-{label}-{}-{id}", std::process::id()));
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

    fn write_package(root: &Path, body: &str) {
        std::fs::create_dir_all(root.join("tools")).expect("tools");
        std::fs::write(root.join(MANIFEST_FILE), body).expect("manifest");
        std::fs::write(root.join("tools").join("echo.json"), "{}").expect("tool");
    }

    fn manifest() -> String {
        r#"
schema_version = "1.0"

[extension]
id = "com.example.echo"
name = "Echo"
version = "1.2.3"
description = "Echoes arguments"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo"]

[resources]
max_execution_ms = 1000
"#
        .to_string()
    }

    #[test]
    fn a_package_hashes_deterministically_and_changes_with_content() {
        let scratch = Scratch::new("hash");
        let root = scratch.0.join("package");
        write_package(&root, &manifest());
        let first = package_hash(&root).expect("hash");
        let second = package_hash(&root).expect("hash again");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        std::fs::write(root.join("tools").join("echo.json"), "{\"changed\":true}").expect("change");
        let third = package_hash(&root).expect("hash changed");
        assert_ne!(first, third);
    }

    #[test]
    fn install_is_staged_and_recorded_then_uninstall_retains_data() {
        let scratch = Scratch::new("install");
        let source = scratch.0.join("source");
        write_package(&source, &manifest());
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        let package = store.install(&source).expect("install");
        assert_eq!(package.manifest.extension.version.to_string(), "1.2.3");
        let id = package.id().clone();
        assert!(store.version_root(&id, "1.2.3").is_dir());
        let registry = store.read_registry().expect("registry");
        let record = registry.record(id.as_str(), "1.2.3").expect("record");
        assert_eq!(record.hash, package.hash);
        assert!(store.state_of(&id).enabled);

        std::fs::create_dir_all(store.data_dir(&id)).expect("data");
        std::fs::write(store.data_dir(&id).join("keep.json"), "{}").expect("data file");
        store.uninstall(&id, false).expect("uninstall");
        assert!(!store.version_root(&id, "1.2.3").exists());
        assert!(store.data_dir(&id).join("keep.json").is_file());
        assert!(!store
            .read_registry()
            .expect("registry")
            .extensions
            .contains_key(id.as_str()));

        let again = store.install(&source).expect("reinstall");
        assert_eq!(again.id(), &id);
        store.uninstall(&id, true).expect("uninstall with data");
        assert!(!store.data_dir(&id).exists());
    }

    #[test]
    fn a_second_install_of_the_same_version_is_refused() {
        let scratch = Scratch::new("duplicate");
        let source = scratch.0.join("source");
        write_package(&source, &manifest());
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        store.install(&source).expect("first");
        let error = store.install(&source).expect_err("second");
        assert!(matches!(error, ExtensionError::AlreadyInstalled { .. }));
    }

    #[test]
    fn an_invalid_manifest_is_not_installed() {
        let scratch = Scratch::new("invalid");
        let source = scratch.0.join("source");
        write_package(
            &source,
            &manifest().replace("version = \"1.2.3\"", "version = \"nope\""),
        );
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        assert!(store.install(&source).is_err());
        assert!(store.scan_installed().expect("scan").0.is_empty());
    }

    #[test]
    fn a_symlink_is_refused() {
        #[cfg(unix)]
        {
            let scratch = Scratch::new("symlink");
            let source = scratch.0.join("source");
            write_package(&source, &manifest());
            std::os::unix::fs::symlink("/tmp", source.join("escape")).expect("symlink");
            assert!(package_hash(&source).is_err());
        }
    }

    #[test]
    fn state_survives_a_reopen() {
        let scratch = Scratch::new("state");
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        let id = ExtensionId::parse("com.example.echo").expect("id");
        store.set_enabled(&id, false).expect("disable");
        let reopened = ExtensionStore::open(scratch.0.join("store")).expect("reopen");
        assert!(!reopened.state_of(&id).enabled);
    }
}
