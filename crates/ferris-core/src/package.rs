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
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::capability::unix_now;
use super::manifest::{Manifest, MANIFEST_FILE};
use super::signing::{PackageSignature, SignatureVerification, TrustStore, SIGNATURE_FILE};
use super::types::{ExtensionError, ExtensionId, ExtensionSource, TrustLevel, ValidationReport};

/// The largest package tree the installer will copy or hash.
pub const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;

/// The largest file count the installer will copy or hash.
pub const MAX_PACKAGE_FILES: usize = 10_000;

/// Directory names that are never part of a package.
const IGNORED_DIRECTORIES: [&str; 5] = [".git", "target", "node_modules", ".jj", "__pycache__"];

/// File suffixes that are never part of a package.
///
/// These are build artifacts a checkout can hold but a commit does not, so
/// skipping them is what makes a working tree and the commit it came from
/// hash the same.
const IGNORED_SUFFIXES: [&str; 3] = [".tmp", ".pyc", ".pyo"];

/// One loaded package and the facts the host established about it.
#[derive(Debug, Clone)]
pub struct Package {
    /// Absolute package root.
    pub root: PathBuf,
    /// Parsed manifest.
    pub manifest: Manifest,
    /// Validation result; a package with errors is listed but never activated.
    pub report: ValidationReport,
    /// Where the package came from.
    pub source: ExtensionSource,
    /// How much the host trusts it.
    pub trust: TrustLevel,
    /// Canonical SHA-256 of the package tree, empty for a built-in.
    pub hash: String,
    /// Unix seconds the package was installed, zero when unknown.
    pub installed_at: u64,
    /// Signature metadata shipped with the package, if any.
    pub signature: Option<PackageSignature>,
    /// The trust store's verdict on that signature.
    pub verification: SignatureVerification,
    /// The directory or archive this install was copied from, when known.
    pub source_path: Option<PathBuf>,
}

impl Package {
    /// Load the package at `root`.
    pub fn load(
        root: &Path,
        source: ExtensionSource,
        trust: TrustLevel,
    ) -> Result<Self, ExtensionError> {
        let manifest = crate::manifest_v2::load_package_manifest(root)?;
        let mut report = manifest.validate();
        if manifest.runtime.kind == crate::types::RuntimeKind::Builtin
            && source != ExtensionSource::Builtin
        {
            report.error(
                "runtime.kind",
                "a manifest read from disk may not claim the builtin runtime; the built-in registry owns those ids",
            );
        }
        // §3: every declared contribution must have its file. Skills were the
        // one kind no code path checked, so a declaration could list a skill
        // the package never shipped.
        for skill in &manifest.contributes.skills {
            let path = root.join("skills").join(skill.as_str()).join("SKILL.md");
            if !path.is_file() {
                report.error(
                    format!("contributes.skills.{}", skill.as_str()),
                    format!("skills/{}/SKILL.md is missing", skill.as_str()),
                );
            }
        }
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
    pub fn builtin(manifest_toml: &str) -> Result<Self, ExtensionError> {
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
    pub fn id(&self) -> &ExtensionId {
        self.manifest.id()
    }

    /// The package version as text.
    pub fn version(&self) -> String {
        self.manifest.extension.version.to_string()
    }

    /// Whether the package may be activated.
    pub fn is_valid(&self) -> bool {
        self.report.is_valid()
    }

    /// The validation errors joined into one sentence, for a status row.
    pub fn error_message(&self) -> String {
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
/// timestamps must hash the same. Build artifacts a commit cannot carry
/// (`.git`, `target`, `node_modules`, `.jj`, `__pycache__`, `*.tmp`,
/// `*.pyc`, `*.pyo`, `.DS_Store`) are excluded too, so hashing a working
/// tree and hashing the clone a publisher ships give one digest rather than
/// two.
pub fn package_hash(root: &Path) -> Result<String, ExtensionError> {
    let mut digest = PackageDigest::new();
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
        digest.file(&relative, file_bytes, file_hasher.finalize());
    }
    Ok(digest.finish())
}

/// The one framing every package digest uses.
struct PackageDigest(Sha256);

impl PackageDigest {
    fn new() -> Self {
        Self(Sha256::new())
    }

    /// Fold one file into the digest.
    fn file(&mut self, relative: &str, bytes: u64, digest: impl AsRef<[u8]>) {
        self.0.update(relative.as_bytes());
        self.0.update([0u8]);
        self.0.update(bytes.to_le_bytes());
        self.0.update(digest);
    }

    fn finish(self) -> String {
        hex_digest(self.0.finalize())
    }
}

/// Hash the package tree as one Git revision commits it.
///
/// This is the digest a market index must carry. The host installs from a
/// clone, and a clone carries committed files only, so reading the revision
/// instead of the disk is what keeps a build artifact, an editor swapfile or
/// an uncommitted edit from publishing a digest no clone can reproduce.
pub fn package_hash_at_rev(root: &Path, rev: &str) -> Result<String, ExtensionError> {
    if git_text(root, &["rev-parse", "--show-toplevel"])?.is_empty() {
        return Err(ExtensionError::Corrupt {
            path: root.to_path_buf(),
            message: format!("{} is not inside a Git work tree", root.display()),
        });
    }
    let prefix = git_text(root, &["rev-parse", "--show-prefix"])?;
    let prefix = prefix.trim_end_matches('/').to_string();
    let spec = if prefix.is_empty() {
        ".".to_string()
    } else {
        prefix.clone()
    };
    let listing = git_bytes(
        root,
        &["ls-tree", "-r", "-z", "--full-tree", rev, "--", &spec],
    )?;

    let mut entries: Vec<(String, String)> = Vec::new();
    for record in listing.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| ExtensionError::Corrupt {
                path: root.to_path_buf(),
                message: format!("git ls-tree returned a record without a path at {rev}"),
            })?;
        let head =
            std::str::from_utf8(&record[..tab]).map_err(|error| ExtensionError::Corrupt {
                path: root.to_path_buf(),
                message: format!("git ls-tree returned an unreadable entry: {error}"),
            })?;
        let path =
            std::str::from_utf8(&record[tab + 1..]).map_err(|error| ExtensionError::Corrupt {
                path: root.to_path_buf(),
                message: format!("git ls-tree returned an unreadable path: {error}"),
            })?;
        let mut fields = head.split(' ');
        let mode = fields.next().unwrap_or_default();
        let kind = fields.next().unwrap_or_default();
        let sha = fields.next().unwrap_or_default();
        let relative = if prefix.is_empty() {
            path.to_string()
        } else {
            let tail = format!("{prefix}/");
            path.strip_prefix(&tail)
                .map(str::to_string)
                .ok_or_else(|| ExtensionError::Corrupt {
                    path: root.to_path_buf(),
                    message: format!("git ls-tree returned {path}, outside {tail}"),
                })?
        };
        if kind != "blob" {
            return Err(ExtensionError::Limit {
                extension: root.display().to_string(),
                message: format!(
                    "{rev} carries {kind} at {relative}, which is not a file; packages must be self-contained"
                ),
            });
        }
        if mode == "120000" {
            return Err(ExtensionError::Limit {
                extension: root.display().to_string(),
                message: format!(
                    "{rev} carries a symbolic link at {relative}; packages must be self-contained"
                ),
            });
        }
        if is_ignored_file(&relative) || relative.rsplit('/').next() == Some(SIGNATURE_FILE) {
            continue;
        }
        entries.push((relative, sha.to_string()));
    }

    if entries.is_empty() {
        return Err(ExtensionError::Corrupt {
            path: root.to_path_buf(),
            message: format!("{rev} commits no files under {}", root.display()),
        });
    }

    let order = walk_order(
        &entries
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
    );
    let positions: std::collections::HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(index, path)| (path.as_str(), index))
        .collect();
    entries.sort_by_key(|(path, _)| positions[path.as_str()]);

    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ExtensionError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    let mut requests = String::new();
    for (_, sha) in &entries {
        requests.push_str(sha);
        requests.push('\n');
    }
    let mut stdin = child.stdin.take().ok_or_else(|| ExtensionError::Corrupt {
        path: root.to_path_buf(),
        message: "git cat-file stdin is not piped".to_string(),
    })?;
    // A writer thread keeps the request pipe from deadlocking the output pipe
    // on a package large enough to fill both.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(requests.as_bytes());
    });
    let stdout = child.stdout.take().ok_or_else(|| ExtensionError::Corrupt {
        path: root.to_path_buf(),
        message: "git cat-file stdout is not piped".to_string(),
    })?;
    let result = (|| -> Result<String, ExtensionError> {
        let mut reader = BufReader::new(stdout);
        let mut digest = PackageDigest::new();
        let mut files = 0usize;
        let mut bytes = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        for (relative, sha) in &entries {
            let mut header = String::new();
            reader
                .read_line(&mut header)
                .map_err(|source| ExtensionError::Io {
                    path: root.to_path_buf(),
                    source,
                })?;
            let mut fields = header.trim_end().split(' ');
            let _oid = fields.next();
            let kind = fields.next();
            let size = fields.next().and_then(|value| value.parse::<u64>().ok());
            let (Some("blob"), Some(size)) = (kind, size) else {
                return Err(ExtensionError::Corrupt {
                    path: root.to_path_buf(),
                    message: format!(
                        "git cat-file could not read blob {sha}: {}",
                        header.trim_end()
                    ),
                });
            };
            let mut file = Sha256::new();
            let mut remaining = size;
            while remaining > 0 {
                let want = remaining.min(buffer.len() as u64) as usize;
                reader
                    .read_exact(&mut buffer[..want])
                    .map_err(|source| ExtensionError::Io {
                        path: root.to_path_buf(),
                        source,
                    })?;
                file.update(&buffer[..want]);
                remaining -= want as u64;
                bytes += want as u64;
                if bytes > MAX_PACKAGE_BYTES {
                    return Err(ExtensionError::Limit {
                        extension: root.display().to_string(),
                        message: format!("package is larger than {MAX_PACKAGE_BYTES} bytes"),
                    });
                }
            }
            let mut newline = [0u8; 1];
            reader
                .read_exact(&mut newline)
                .map_err(|source| ExtensionError::Io {
                    path: root.to_path_buf(),
                    source,
                })?;
            files += 1;
            if files > MAX_PACKAGE_FILES {
                return Err(ExtensionError::Limit {
                    extension: root.display().to_string(),
                    message: format!("package contains more than {MAX_PACKAGE_FILES} files"),
                });
            }
            digest.file(relative, size, file.finalize());
        }
        Ok(digest.finish())
    })();
    if result.is_err() {
        // git is blocked writing blobs we stopped reading; end it before wait.
        let _ = child.kill();
    }
    let _ = writer.join();
    let _ = child.wait();
    result
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, ExtensionError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|source| ExtensionError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(ExtensionError::Corrupt {
            path: root.to_path_buf(),
            message: format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(output.stdout)
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, ExtensionError> {
    Ok(String::from_utf8_lossy(&git_bytes(root, args)?)
        .trim()
        .to_string())
}

/// A directory tree built from package-relative paths, so a committed listing
/// visits in the same pre-order the disk walk uses.
#[derive(Default)]
struct VersionTree(BTreeMap<String, VersionTree>);

fn walk_order(paths: &[&str]) -> Vec<String> {
    let mut root = VersionTree::default();
    for path in paths {
        let mut node = &mut root;
        for part in path.split('/') {
            node = node.0.entry(part.to_string()).or_default();
        }
    }
    fn visit(node: &VersionTree, prefix: &str, out: &mut Vec<String>) {
        for (name, child) in &node.0 {
            let path = format!("{prefix}{name}");
            if child.0.is_empty() {
                out.push(path);
            } else {
                visit(child, &format!("{path}/"), out);
            }
        }
    }
    let mut out = Vec::new();
    visit(&root, "", &mut out);
    out
}

/// The file half of is_ignored for a path that is known to be a file.
fn is_ignored_file(relative: &str) -> bool {
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_some() {
            if IGNORED_DIRECTORIES.contains(&part) {
                return true;
            }
        } else {
            return part == ".DS_Store"
                || IGNORED_SUFFIXES.iter().any(|suffix| part.ends_with(suffix));
        }
    }
    false
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Whether a path is build junk rather than package content.
///
/// [`package_hash`] and [`copy_tree`] share this one rule, which is what
/// guarantees an excluded path is both unhashed and uncopied. Archive
/// builders outside this crate reuse it for the same reason.
pub fn is_ignored(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    name == ".DS_Store"
        || IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
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

/// Serializes store mutations inside this process.
///
/// Two installs of the same id/version race the destination check and share a
/// staging directory; a state update interleaved with an install can lose one
/// writer's fields. The guard is process-wide (a second host process is a
/// separate concern), which is the granularity the manager has.
static STORE_MUTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn store_mutation_lock() -> std::sync::MutexGuard<'static, ()> {
    STORE_MUTATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Write a file atomically: a sibling temp file, fsync, rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ExtensionError> {
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
    {
        // fs::rename cannot replace an existing file on Windows, and the old
        // remove-then-rename destroyed the previous document when the second
        // rename failed. Move it aside and restore it on failure instead.
        let backup = path.with_extension("bak");
        let had_existing = path.exists();
        if had_existing {
            let _ = std::fs::remove_file(&backup);
            std::fs::rename(path, &backup).map_err(|source| ExtensionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        }
        if let Err(source) = std::fs::rename(&temporary, path) {
            if had_existing {
                let _ = std::fs::rename(&backup, path);
            }
            return Err(ExtensionError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
        if had_existing {
            let _ = std::fs::remove_file(&backup);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    std::fs::rename(&temporary, path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Copy a package tree, refusing symlinks and enforcing the same caps the hash
/// does. Existing files are replaced, so this is only safe into an empty
/// staging directory.
pub fn copy_tree(source: &Path, destination: &Path) -> Result<(), ExtensionError> {
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
pub struct ExtensionState {
    /// Whether the reader left the extension enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The last lifecycle state the host recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state: Option<String>,
    /// The last failure, kept across launches until a retry succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Unix seconds of the last successful start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_started: Option<u64>,
    /// Unix seconds the version was installed.
    #[serde(default)]
    pub installed_at: u64,
    /// The exact version the host resolves for this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    /// The data schema version the migrations last produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_version: Option<String>,
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
pub struct StateFile {
    /// Format generation.
    #[serde(default = "state_format")]
    pub format: u32,
    /// extension id -> state.
    #[serde(default)]
    pub extensions: BTreeMap<String, ExtensionState>,
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
pub struct InstalledRecord {
    /// Package-relative path below the store root.
    pub path: String,
    /// Canonical SHA-256 recorded at install time.
    pub hash: String,
    /// Where the package came from.
    #[serde(default)]
    pub source: ExtensionSource,
    /// Trust established at install time.
    #[serde(default = "default_trust")]
    pub trust: TrustLevel,
    /// Unix seconds of installation.
    #[serde(default)]
    pub installed_at: u64,
    /// Dependency ranges the manifest declared.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// The trust store's verdict on the package signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<SignatureVerification>,
    /// Publisher name from the signature, when one was present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// The directory or archive this install was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

fn default_trust() -> TrustLevel {
    TrustLevel::Local
}

/// The persisted installed-package index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryFile {
    /// Format generation.
    #[serde(default = "state_format")]
    pub format: u32,
    /// extension id -> version -> record.
    #[serde(default)]
    pub extensions: BTreeMap<String, BTreeMap<String, InstalledRecord>>,
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
    pub fn record(&self, id: &str, version: &str) -> Option<&InstalledRecord> {
        self.extensions
            .get(id)
            .and_then(|versions| versions.get(version))
    }

    /// Insert one record.
    pub fn insert(&mut self, id: &str, version: &str, record: InstalledRecord) {
        self.extensions
            .entry(id.to_string())
            .or_default()
            .insert(version.to_string(), record);
    }

    /// Remove every version of one extension.
    pub fn remove(&mut self, id: &str) -> bool {
        self.extensions.remove(id).is_some()
    }
}

/// One package found by a store scan that could not be loaded.
#[derive(Debug, Clone)]
pub struct ScanFailure {
    /// The directory that failed.
    pub path: PathBuf,
    /// Why it failed.
    pub error: String,
}

/// The managed extension store.
#[derive(Debug, Clone)]
pub struct ExtensionStore {
    root: PathBuf,
}

impl ExtensionStore {
    /// Open (and create) the store at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ExtensionError> {
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
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `installed/`.
    pub fn installed_root(&self) -> PathBuf {
        self.root.join("installed")
    }

    /// `staging/`.
    pub fn staging_root(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// `cache/`.
    pub fn cache_root(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `quarantine/`.
    pub fn quarantine_root(&self) -> PathBuf {
        self.root.join("quarantine")
    }

    /// `extension-data/`.
    pub fn data_root(&self) -> PathBuf {
        self.root.join("extension-data")
    }

    /// `extension-state/`.
    pub fn state_root(&self) -> PathBuf {
        self.root.join("extension-state")
    }

    /// `logs/extensions/`.
    pub fn log_root(&self) -> PathBuf {
        self.root.join("logs").join("extensions")
    }

    /// The installed registry document.
    pub fn registry_path(&self) -> PathBuf {
        self.state_root().join("registry.toml")
    }

    /// The dependency lockfile.
    pub fn lock_path(&self) -> PathBuf {
        self.state_root().join("lock.toml")
    }

    /// The enabled/failure state document.
    pub fn state_path(&self) -> PathBuf {
        self.state_root().join("state.toml")
    }

    /// The grant document.
    pub fn grants_path(&self) -> PathBuf {
        self.state_root().join("grants.toml")
    }

    /// The publisher trust document.
    pub fn trust_path(&self) -> PathBuf {
        self.state_root().join("trust.toml")
    }

    /// The capability audit log.
    pub fn audit_path(&self) -> PathBuf {
        self.log_root().join("audit.jsonl")
    }

    /// The retained load-crash history.
    pub fn crashes_path(&self) -> PathBuf {
        self.log_root().join("crashes.jsonl")
    }

    /// Append one load failure unless it repeats the last recorded line.
    pub fn record_crash(&self, id: &str, error: &str) {
        let path = self.crashes_path();
        let line = format!("{}\t{id}\t{error}\n", crate::capability::unix_now());
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
    pub fn crash_history(&self, limit: usize) -> Vec<String> {
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
    pub fn log_dir(&self, id: &ExtensionId) -> PathBuf {
        self.log_root().join(id.as_str())
    }

    /// The directory holding one installed version.
    pub fn version_root(&self, id: &ExtensionId, version: &str) -> PathBuf {
        self.installed_root().join(id.as_str()).join(version)
    }

    /// The data directory of one extension.
    pub fn data_dir(&self, id: &ExtensionId) -> PathBuf {
        self.data_root().join(id.as_str())
    }

    /// Read the registry, returning an empty one when absent.
    pub fn read_registry(&self) -> Result<RegistryFile, ExtensionError> {
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
    pub fn write_registry(&self, registry: &RegistryFile) -> Result<(), ExtensionError> {
        let text = toml::to_string_pretty(registry).map_err(|error| ExtensionError::Corrupt {
            path: self.registry_path(),
            message: error.to_string(),
        })?;
        write_atomic(&self.registry_path(), text.as_bytes())
    }

    /// Read the state document, returning defaults when absent.
    pub fn read_state(&self) -> Result<StateFile, ExtensionError> {
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
    pub fn write_state(&self, state: &StateFile) -> Result<(), ExtensionError> {
        let text = toml::to_string_pretty(state).map_err(|error| ExtensionError::Corrupt {
            path: self.state_path(),
            message: error.to_string(),
        })?;
        write_atomic(&self.state_path(), text.as_bytes())
    }

    /// The state record for one extension, with the default of enabled.
    ///
    /// A *missing* record means "never configured", which is enabled. A state
    /// document that cannot be read is different: falling back to the default
    /// would silently re-enable every package the reader had disabled, so an
    /// unreadable document reports every package as disabled instead.
    pub fn state_of(&self, id: &ExtensionId) -> ExtensionState {
        match self.read_state() {
            Ok(state) => state
                .extensions
                .get(id.as_str())
                .cloned()
                .unwrap_or_default(),
            Err(_) => ExtensionState {
                enabled: false,
                ..ExtensionState::default()
            },
        }
    }

    /// Persist one extension's state record.
    pub fn update_state(
        &self,
        id: &ExtensionId,
        update: impl FnOnce(&mut ExtensionState),
    ) -> Result<ExtensionState, ExtensionError> {
        let _guard = store_mutation_lock();
        self.update_state_locked(id, update)
    }

    /// The update body, for callers that already hold the store mutation lock.
    fn update_state_locked(
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
    pub fn set_enabled(
        &self,
        id: &ExtensionId,
        enabled: bool,
    ) -> Result<ExtensionState, ExtensionError> {
        self.update_state(id, |state| state.enabled = enabled)
    }

    /// Every installed version of an extension, sorted ascending.
    pub fn versions(&self, id: &ExtensionId) -> Result<Vec<String>, ExtensionError> {
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
    pub fn set_active(
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
    pub fn set_data_version(
        &self,
        id: &ExtensionId,
        version: &str,
    ) -> Result<ExtensionState, ExtensionError> {
        self.update_state(id, |state| {
            state.data_version = Some(version.to_string());
        })
    }

    /// The pinned version, if one was recorded.
    pub fn active_version(&self, id: &ExtensionId) -> Option<String> {
        self.state_of(id).active_version
    }

    /// Move every installed version of an extension into quarantine.
    ///
    /// This is the last-resort control: the code is no longer loadable, the
    /// state record is removed and the data is retained for a later review or
    /// deliberate purge.
    pub fn quarantine(&self, id: &ExtensionId, reason: &str) -> Result<PathBuf, ExtensionError> {
        let _guard = store_mutation_lock();
        let source = self.installed_root().join(id.as_str());
        if !source.exists() {
            return Err(ExtensionError::NotInstalled { id: id.to_string() });
        }
        // Create the parent, never the destination: a pre-created destination
        // makes rename fail on Windows (and on Unix for an existing directory).
        let quarantine_root = self.quarantine_root();
        std::fs::create_dir_all(&quarantine_root).map_err(|source| ExtensionError::Io {
            path: quarantine_root.clone(),
            source,
        })?;
        let base = format!("{}-{}", id.as_str(), unix_now());
        let mut destination = quarantine_root.join(&base);
        let mut suffix = 1u32;
        while destination.exists() {
            destination = quarantine_root.join(format!("{base}-{suffix}"));
            suffix = suffix.saturating_add(1);
        }
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
    pub fn scan_installed(&self) -> Result<(Vec<Package>, Vec<ScanFailure>), ExtensionError> {
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
                            // Re-check integrity on every load, not only in
                            // `doctor`: a file changed under installed/ must not
                            // keep the trust level the install recorded.
                            if !record.hash.is_empty()
                                && !package.hash.is_empty()
                                && record.hash != package.hash
                            {
                                package.report.error(
                                    "hash",
                                    "installed files no longer match the hash recorded at install time; reinstall the package",
                                );
                            } else if let Ok(trust_store) =
                                super::signing::TrustStore::load(self.trust_path())
                            {
                                let verification = super::signing::verify_package(
                                    &version_dir,
                                    package.id().as_str(),
                                    &package.version().to_string(),
                                    &package.hash,
                                    &trust_store,
                                );
                                if let SignatureVerification::Invalid { reason } = &verification {
                                    package.report.error(
                                        "signature",
                                        format!("installed signature is invalid: {reason}"),
                                    );
                                }
                                // A publisher removal or a key change must
                                // demote an installed package, not be frozen at
                                // the install-time verdict.
                                let fallback = if matches!(
                                    package.verification,
                                    SignatureVerification::Verified { .. }
                                ) {
                                    TrustLevel::Community
                                } else {
                                    package.trust
                                };
                                package.trust = verification.trust_level(fallback);
                                package.verification = verification;
                            }
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
    pub fn install(&self, source: &Path) -> Result<Package, ExtensionError> {
        self.install_with(source, ExtensionSource::Installed)
    }

    /// Install a package and record where it came from.
    pub fn install_with(
        &self,
        source: &Path,
        origin: ExtensionSource,
    ) -> Result<Package, ExtensionError> {
        self.install_with_trust(source, origin, TrustLevel::Local)
    }

    /// Install an unsigned package from an untrusted source such as a Git
    /// marketplace. A valid signature still wins over the fallback trust.
    pub fn install_community(
        &self,
        source: &Path,
        origin: ExtensionSource,
    ) -> Result<Package, ExtensionError> {
        self.install_with_trust(source, origin, TrustLevel::Community)
    }

    fn install_with_trust(
        &self,
        source: &Path,
        origin: ExtensionSource,
        fallback: TrustLevel,
    ) -> Result<Package, ExtensionError> {
        let _guard = store_mutation_lock();
        let staged = Package::load(source, ExtensionSource::Development, fallback)?;
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
        let trust_level = verification.trust_level(fallback);
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
        // The signature above covers the hash of the source tree that was read.
        // If the staging copy does not hash to the same value, the bytes being
        // installed are not the bytes that were verified.
        if !staged.hash.is_empty() && staged.hash != hash {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ExtensionError::Denied {
                extension: id.to_string(),
                capability: "package.integrity".to_string(),
                reason: "the copied package does not match the verified source hash".to_string(),
            });
        }
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
        // From here on the package exists on disk. A failure while recording
        // it must remove the directory again, or a retry reports AlreadyInstalled
        // for a package the registry never learned about.
        let installed = (|| -> Result<Package, ExtensionError> {
            let mut package = Package::load(&destination, origin.clone(), trust_level)?;
            package.hash = hash.clone();
            package.installed_at = unix_now();
            package.verification = verification.clone();
            package.signature = staged.signature.clone();
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
            self.update_state_locked(&id, |state| {
                // An upgrade must not switch a package the reader disabled back on:
                // the enabled flag is the reader's decision and survives the install
                // (§7). A fresh record already defaults to enabled.
                state.installed_at = package.installed_at;
                state.last_error = None;
                state.active_version = Some(version.clone());
                if state.data_version.is_none() {
                    state.data_version = Some(version.clone());
                }
            })?;
            Ok(package)
        })();
        if installed.is_err() {
            let _ = std::fs::remove_dir_all(&destination);
        }
        installed
    }

    /// Remove every installed version of `id`.
    ///
    /// Business data is retained unless `purge_data` is true. The state record
    /// is always removed, because the flags describe an installed package and
    /// a later install starts from the defaults.
    pub fn uninstall(&self, id: &ExtensionId, purge_data: bool) -> Result<(), ExtensionError> {
        let _guard = store_mutation_lock();
        let directory = self.installed_root().join(id.as_str());
        if !directory.exists() {
            return Err(ExtensionError::NotInstalled { id: id.to_string() });
        }
        // Move the tree aside before touching metadata: removing it first
        // left the registry naming files that no longer existed when a later
        // write failed. The trash directory and both metadata documents are
        // restored on that error.
        let previous_registry = self.read_registry()?;
        let previous_state = self.read_state()?;
        let trash_root = self.root.join("trash");
        std::fs::create_dir_all(&trash_root).map_err(|source| ExtensionError::Io {
            path: trash_root.clone(),
            source,
        })?;
        let trash = trash_root.join(format!(
            "{}-{}-{}",
            id.as_str(),
            std::process::id(),
            unix_now()
        ));
        std::fs::rename(&directory, &trash).map_err(|source| ExtensionError::Io {
            path: directory.clone(),
            source,
        })?;
        let metadata = (|| -> Result<(), ExtensionError> {
            let mut registry = self.read_registry()?;
            if registry.remove(id.as_str()) {
                self.write_registry(&registry)?;
            }
            let mut state = self.read_state()?;
            if state.extensions.remove(id.as_str()).is_some() {
                self.write_state(&state)?;
            }
            Ok(())
        })();
        if let Err(error) = metadata {
            // Best effort: put the package and the documents back exactly as
            // they were, so a retry sees an installed package rather than a
            // half-removed one.
            let _ = self.write_registry(&previous_registry);
            let _ = self.write_state(&previous_state);
            let _ = std::fs::rename(&trash, &directory);
            return Err(error);
        }
        let _ = std::fs::remove_dir_all(&trash);
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
    pub fn record_failure(&self, id: &ExtensionId, error: &str) -> Result<(), ExtensionError> {
        self.update_state(id, |state| {
            state.last_error = Some(error.to_string());
            state.last_state = Some("failed".to_string());
        })?;
        Ok(())
    }

    /// Clear a persisted failure so the next load can retry.
    pub fn clear_failure(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
        self.update_state(id, |state| {
            state.last_error = None;
            state.last_state = Some("loading".to_string());
        })?;
        Ok(())
    }

    /// Record a successful activation.
    pub fn record_started(&self, id: &ExtensionId) -> Result<(), ExtensionError> {
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
        let legacy = Manifest::parse(body, &root.join(MANIFEST_FILE)).expect("legacy");
        let v2 = crate::manifest_v2::ManifestV2::from_legacy(&legacy);
        std::fs::write(
            root.join(MANIFEST_FILE),
            v2.to_toml().expect("2.0 manifest"),
        )
        .expect("manifest");
        std::fs::write(root.join("tools").join("echo.json"), "{}").expect("tool");
    }

    fn write_raw_package(root: &Path, body: &str) {
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
    fn build_artifacts_a_commit_cannot_carry_do_not_change_the_hash() {
        let scratch = Scratch::new("hash-artifacts");
        let root = scratch.0.join("package");
        write_package(&root, &manifest());
        let committed = package_hash(&root).expect("clean hash");

        // A Python test run and a Finder visit leave files a clone never has.
        // The digest must describe the commit, not the disk.
        std::fs::create_dir_all(root.join("__pycache__")).expect("cache dir");
        std::fs::write(root.join("__pycache__").join("echo.pyc"), "bytecode").expect("cache");
        std::fs::write(root.join("tools").join("stray.pyc"), "bytecode").expect("stray bytecode");
        std::fs::write(root.join("tools").join("scratch.tmp"), "scratch").expect("scratch");
        std::fs::write(root.join("tools").join("stray.pyo"), "bytecode").expect("stray optimized");
        assert_eq!(package_hash(&root).expect("hash with artifacts"), committed);

        // A file the package really ships still moves it.
        std::fs::write(root.join("tools").join("notes.txt"), "notes").expect("real file");
        assert_ne!(
            package_hash(&root).expect("hash with a new file"),
            committed
        );

        // The copy must skip the same files the hash skipped, or the installed
        // tree would carry bytes no digest covers.
        std::fs::remove_file(root.join("tools").join("notes.txt")).expect("drop real file");
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        let installed = store.install(&root).expect("install");
        assert!(!installed.root.join("__pycache__").exists());
        assert!(!installed.root.join("tools").join("stray.pyc").exists());
        assert_eq!(installed.hash, committed);
    }
    #[test]
    fn a_revision_hashes_the_commit_not_the_checkout() {
        let scratch = Scratch::new("hash-rev");
        let root = scratch.0.join("package");
        write_package(&root, &manifest());
        git(&root, &["init", "-q"]);
        git(&root, &["add", "."]);
        git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "one",
            ],
        );
        let committed = package_hash(&root).expect("disk hash");
        assert_eq!(
            package_hash_at_rev(&root, "HEAD").expect("HEAD hash"),
            committed
        );

        // Build junk is invisible to both; an uncommitted edit is visible to
        // the checkout only, which is the whole point of hashing the commit.
        std::fs::create_dir_all(root.join("__pycache__")).expect("cache dir");
        std::fs::write(root.join("__pycache__").join("echo.pyc"), "bytecode").expect("cache");
        assert_eq!(package_hash(&root).expect("disk hash"), committed);
        assert_eq!(
            package_hash_at_rev(&root, "HEAD").expect("HEAD hash"),
            committed
        );

        std::fs::write(root.join("tools").join("echo.json"), "{\"changed\":true}").expect("edit");
        assert_ne!(package_hash(&root).expect("disk hash"), committed);
        assert_eq!(
            package_hash_at_rev(&root, "HEAD").expect("HEAD hash"),
            committed
        );
    }

    fn git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", root)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
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
    fn an_upgrade_keeps_a_disabled_package_disabled() {
        let scratch = Scratch::new("upgrade-enabled");
        let source = scratch.0.join("source");
        write_package(&source, &manifest());
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        let package = store.install(&source).expect("install");
        store.set_enabled(package.id(), false).expect("disable");
        let upgrade = scratch.0.join("upgrade");
        write_package(&upgrade, &manifest().replace("1.2.3", "1.2.4"));
        store.install(&upgrade).expect("upgrade");
        assert!(
            !store.state_of(package.id()).enabled,
            "an install must not switch a disabled package back on"
        );
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
        write_raw_package(
            &source,
            r#"
schema_version = "2.0"
[extension]
id = "com.example.echo"
name = "Echo"
version = "nope"
description = "invalid"
[compatibility]
host = ">=3.0.0"
extension_api = "^1.0"
[runtime]
kind = "declarative"
[[contribution]]
kind = "tool"
id = "echo"
"#,
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

    #[test]
    fn an_unreadable_state_document_disables_packages() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ExtensionStore::open(root.path()).expect("store");
        let id = ExtensionId::parse("com.example.hello").expect("id");
        assert!(
            store.state_of(&id).enabled,
            "a missing record means enabled"
        );
        std::fs::create_dir_all(store.state_path().parent().expect("state root"))
            .expect("state root");
        std::fs::write(store.state_path(), "this is not = [ toml").expect("corrupt state");
        assert!(
            !store.state_of(&id).enabled,
            "an unreadable state document must fail closed"
        );
    }

    #[test]
    fn quarantine_moves_the_tree_to_a_new_directory() {
        let scratch = Scratch::new("quarantine");
        let source = scratch.0.join("source");
        write_package(&source, &manifest());
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        store.install(&source).expect("install");
        let id = ExtensionId::parse("com.example.echo").expect("id");
        let destination = store.quarantine(&id, "test").expect("quarantine");
        assert!(
            destination.is_dir(),
            "the quarantine destination is a directory"
        );
        assert!(
            destination.join("1.2.3").join(MANIFEST_FILE).is_file(),
            "the installed version moved with the tree"
        );
        assert!(!store.installed_root().join(id.as_str()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_uninstall_restores_the_package_and_registry() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new("uninstall-rollback");
        let source = scratch.0.join("source");
        write_package(&source, &manifest());
        let store = ExtensionStore::open(scratch.0.join("store")).expect("store");
        store.install(&source).expect("install");
        let id = ExtensionId::parse("com.example.echo").expect("id");
        let state_parent = store
            .state_path()
            .parent()
            .expect("state root")
            .to_path_buf();
        let mut permissions = std::fs::metadata(&state_parent)
            .expect("state metadata")
            .permissions();
        permissions.set_mode(0o555);
        std::fs::set_permissions(&state_parent, permissions).expect("read-only state root");
        let result = store.uninstall(&id, false);
        let mut permissions = std::fs::metadata(&state_parent)
            .expect("state metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&state_parent, permissions).expect("writable state root");
        assert!(result.is_err(), "the state write must fail");
        assert!(
            store.installed_root().join(id.as_str()).is_dir(),
            "the package tree is restored"
        );
        let registry = store.read_registry().expect("registry");
        assert!(
            registry.extensions.contains_key(id.as_str()),
            "the registry entry is restored"
        );
    }
}
