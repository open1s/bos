//! Package sources: local directories and archives, Git checkouts, and
//! official/private HTTP registry indexes.
//!
//! A source is fetched into the cache, validated by the normal manifest and
//! signature path, and then installed by the store. Fetching never writes into
//! installed/: the only promotion is the store's staged rename.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::Archive;

use super::capability::unix_now;
use super::manifest::MANIFEST_FILE;
use super::package::{write_atomic, ExtensionStore, Package};
use super::types::{ExtensionError, ExtensionSource};

/// The largest archive or index this build downloads.
const MAX_DOWNLOAD_BYTES: usize = 256 * 1024 * 1024;

/// A fetched package directory and the source it came from.
#[derive(Debug, Clone)]
pub(crate) struct Fetched {
    /// The package root inside the cache.
    pub(crate) root: PathBuf,
    /// The source label recorded on install.
    pub(crate) origin: ExtensionSource,
}

/// One package entry in a registry index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct IndexEntry {
    /// Extension id.
    pub(crate) id: String,
    /// Exact version.
    pub(crate) version: String,
    /// Archive URL, when the package is distributed as a tar.gz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
    /// Git repository URL, when the package is distributed from Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repository: Option<String>,
    /// Git revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) revision: Option<String>,
    /// Package path inside the repository or archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    /// Expected SHA-256 of the archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sha256: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
}

/// A registry index document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct IndexDocument {
    /// Format generation.
    #[serde(default = "index_format")]
    pub(crate) format: u32,
    /// Index title.
    #[serde(default)]
    pub(crate) title: String,
    /// Packages.
    #[serde(default)]
    pub(crate) packages: Vec<IndexEntry>,
}

fn index_format() -> u32 {
    1
}

/// A named registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Registry {
    /// Index URL.
    pub(crate) url: String,
}

/// The configured registries.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RegistryList {
    /// Format generation.
    #[serde(default = "index_format")]
    pub(crate) format: u32,
    /// name -> registry.
    #[serde(default)]
    pub(crate) registries: BTreeMap<String, Registry>,
}

impl RegistryList {
    /// Load the list from the state root.
    pub(crate) fn load(state_root: &Path) -> Result<Self, ExtensionError> {
        let path = state_root.join("sources.toml");
        if !path.is_file() {
            return Ok(Self::default());
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

    /// Persist the list.
    pub(crate) fn save(&self, state_root: &Path) -> Result<(), ExtensionError> {
        let text = toml::to_string_pretty(self).map_err(|error| ExtensionError::Corrupt {
            path: state_root.join("sources.toml"),
            message: error.to_string(),
        })?;
        write_atomic(&state_root.join("sources.toml"), text.as_bytes())
    }

    /// Add or replace a named registry.
    pub(crate) fn add(&mut self, name: &str, url: &str) {
        self.registries.insert(
            name.to_string(),
            Registry {
                url: url.to_string(),
            },
        );
    }
}

/// Resolve a local directory, archive path, Git URL or HTTP archive.
pub(crate) fn resolve(spec: &str, cache_root: &Path) -> Result<Fetched, ExtensionError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(ExtensionError::Corrupt {
            path: PathBuf::from("<source>"),
            message: "source is empty".to_string(),
        });
    }
    if let Some(git) = spec.strip_prefix("git+") {
        let (url, revision) = split_fragment(git);
        return fetch_git(url, revision, None, cache_root);
    }
    let local = Path::new(spec);
    if local.is_dir() {
        return Ok(Fetched {
            root: local.to_path_buf(),
            origin: ExtensionSource::Development,
        });
    }
    if local.is_file() {
        let root = extract_archive(local, cache_root)?;
        return Ok(Fetched {
            root,
            origin: ExtensionSource::Installed,
        });
    }
    if spec.starts_with("https://") || spec.starts_with("http://") {
        let (url, hash) = split_hash(spec);
        let archive = download(url, cache_root, hash)?;
        let root = extract_archive(&archive, cache_root)?;
        return Ok(Fetched {
            root,
            origin: ExtensionSource::Registry {
                url: url.to_string(),
            },
        });
    }
    Err(ExtensionError::Corrupt {
        path: PathBuf::from(spec),
        message: "source is not a directory, archive, Git URL or HTTP URL".to_string(),
    })
}

fn split_fragment(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once('#') {
        Some((url, revision)) if !revision.is_empty() => (url, Some(revision)),
        _ => (raw, None),
    }
}

fn split_hash(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once("#sha256=") {
        Some((url, hash)) if !hash.is_empty() => (url, Some(hash)),
        _ => (raw, None),
    }
}

/// Clone a Git repository into the cache and return its package root.
pub(crate) fn fetch_git(
    repository: &str,
    revision: Option<&str>,
    subpath: Option<&str>,
    cache_root: &Path,
) -> Result<Fetched, ExtensionError> {
    std::fs::create_dir_all(cache_root).map_err(|source| ExtensionError::Io {
        path: cache_root.to_path_buf(),
        source,
    })?;
    let name = format!("git-{}-{}", sanitize(repository), unix_now());
    let destination = cache_root.join(name);
    let mut clone = Command::new("git");
    clone.args(["clone", "--quiet"]);
    if revision.is_none() {
        clone.args(["--depth", "1"]);
    }
    clone.arg(repository).arg(&destination);
    let output = clone.output().map_err(|source| ExtensionError::Io {
        path: destination.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(ExtensionError::Corrupt {
            path: destination,
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    if let Some(revision) = revision {
        let checkout = Command::new("git")
            .args(["-C"])
            .arg(&destination)
            .args(["checkout", "--quiet", revision])
            .output()
            .map_err(|source| ExtensionError::Io {
                path: destination.clone(),
                source,
            })?;
        if !checkout.status.success() {
            return Err(ExtensionError::Corrupt {
                path: destination,
                message: String::from_utf8_lossy(&checkout.stderr).trim().to_string(),
            });
        }
    }
    let root = match subpath {
        Some(subpath) => destination.join(subpath),
        None => destination,
    };
    if !root.join(MANIFEST_FILE).is_file() {
        return Err(ExtensionError::Corrupt {
            path: root,
            message: "the Git checkout has no extension.toml".to_string(),
        });
    }
    Ok(Fetched {
        root,
        origin: ExtensionSource::Git {
            repository: repository.to_string(),
            revision: revision.unwrap_or("HEAD").to_string(),
        },
    })
}

/// Download an archive into the cache.
pub(crate) fn download(
    url: &str,
    cache_root: &Path,
    expected_hash: Option<&str>,
) -> Result<PathBuf, ExtensionError> {
    std::fs::create_dir_all(cache_root).map_err(|source| ExtensionError::Io {
        path: cache_root.to_path_buf(),
        source,
    })?;
    let target = cache_root.join(format!("download-{}-{}.tar.gz", sanitize(url), unix_now()));
    let response = reqwest::blocking::get(url).map_err(|error| ExtensionError::Corrupt {
        path: target.clone(),
        message: error.to_string(),
    })?;
    if !response.status().is_success() {
        return Err(ExtensionError::Corrupt {
            path: target,
            message: format!("HTTP {}", response.status()),
        });
    }
    let bytes = response.bytes().map_err(|error| ExtensionError::Corrupt {
        path: target.clone(),
        message: error.to_string(),
    })?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(ExtensionError::Limit {
            extension: url.to_string(),
            message: format!("download is larger than {MAX_DOWNLOAD_BYTES} bytes"),
        });
    }
    if let Some(expected) = expected_hash {
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if !digest.eq_ignore_ascii_case(expected) {
            return Err(ExtensionError::Corrupt {
                path: target,
                message: "archive hash does not match the index".to_string(),
            });
        }
    }
    write_atomic(&target, &bytes)?;
    Ok(target)
}

/// Extract a tar.gz archive into a cache directory and find its package root.
pub(crate) fn extract_archive(
    archive_path: &Path,
    cache_root: &Path,
) -> Result<PathBuf, ExtensionError> {
    let destination = cache_root.join(format!(
        "extract-{}-{}",
        sanitize(
            &archive_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ),
        unix_now()
    ));
    std::fs::create_dir_all(&destination).map_err(|source| ExtensionError::Io {
        path: destination.clone(),
        source,
    })?;
    let file = std::fs::File::open(archive_path).map_err(|source| ExtensionError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive.entries().map_err(|error| ExtensionError::Corrupt {
        path: archive_path.to_path_buf(),
        message: error.to_string(),
    })?;
    for entry in entries {
        let mut entry = entry.map_err(|error| ExtensionError::Corrupt {
            path: archive_path.to_path_buf(),
            message: error.to_string(),
        })?;
        let path = entry
            .path()
            .map_err(|error| ExtensionError::Corrupt {
                path: archive_path.to_path_buf(),
                message: error.to_string(),
            })?
            .to_path_buf();
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ExtensionError::Corrupt {
                path: archive_path.to_path_buf(),
                message: format!("archive entry {} escapes the package", path.display()),
            });
        }
        let unpacked = entry
            .unpack_in(&destination)
            .map_err(|error| ExtensionError::Corrupt {
                path: archive_path.to_path_buf(),
                message: error.to_string(),
            })?;
        if !unpacked {
            return Err(ExtensionError::Corrupt {
                path: archive_path.to_path_buf(),
                message: "archive entry could not be placed inside the package".to_string(),
            });
        }
    }
    package_root_in(&destination)
}

fn package_root_in(directory: &Path) -> Result<PathBuf, ExtensionError> {
    if directory.join(MANIFEST_FILE).is_file() {
        return Ok(directory.to_path_buf());
    }
    let mut directories: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|source| ExtensionError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    let mut matching: Vec<PathBuf> = directories
        .into_iter()
        .filter(|path| path.join(MANIFEST_FILE).is_file())
        .collect();
    if matching.len() == 1 {
        return Ok(matching.remove(0));
    }
    Err(ExtensionError::Corrupt {
        path: directory.to_path_buf(),
        message: "archive has no single extension.toml root".to_string(),
    })
}

/// Fetch and parse a registry index.
pub(crate) fn fetch_index(url: &str) -> Result<IndexDocument, ExtensionError> {
    let response = reqwest::blocking::get(url).map_err(|error| ExtensionError::Corrupt {
        path: PathBuf::from(url),
        message: error.to_string(),
    })?;
    if !response.status().is_success() {
        return Err(ExtensionError::Corrupt {
            path: PathBuf::from(url),
            message: format!("HTTP {}", response.status()),
        });
    }
    let bytes = response.bytes().map_err(|error| ExtensionError::Corrupt {
        path: PathBuf::from(url),
        message: error.to_string(),
    })?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(ExtensionError::Limit {
            extension: url.to_string(),
            message: "registry index is too large".to_string(),
        });
    }
    serde_json::from_slice(&bytes).map_err(|error| ExtensionError::Corrupt {
        path: PathBuf::from(url),
        message: error.to_string(),
    })
}

/// Resolve one package from a registry index and fetch it.
pub(crate) fn fetch_index_package(
    index_url: &str,
    id: &str,
    version: Option<&str>,
    cache_root: &Path,
) -> Result<Fetched, ExtensionError> {
    let index = fetch_index(index_url)?;
    let mut matches: Vec<&IndexEntry> = index
        .packages
        .iter()
        .filter(|entry| entry.id == id)
        .filter(|entry| version.is_none_or(|version| entry.version == version))
        .collect();
    matches.sort_by(|left, right| left.version.cmp(&right.version));
    let entry = matches
        .pop()
        .ok_or_else(|| ExtensionError::NoMatchingVersion {
            id: id.to_string(),
            requirement: version.unwrap_or("*").to_string(),
        })?;
    if let Some(repository) = &entry.repository {
        let mut fetched = fetch_git(
            repository,
            entry.revision.as_deref(),
            entry.path.as_deref(),
            cache_root,
        )?;
        fetched.origin = ExtensionSource::Git {
            repository: repository.clone(),
            revision: entry.revision.clone().unwrap_or_else(|| "HEAD".to_string()),
        };
        return Ok(fetched);
    }
    let Some(url) = &entry.url else {
        return Err(ExtensionError::Corrupt {
            path: PathBuf::from(index_url),
            message: format!(
                "{id} {version:?} has neither url nor repository",
                version = entry.version
            ),
        });
    };
    let archive = download(url, cache_root, entry.sha256.as_deref())?;
    let root = extract_archive(&archive, cache_root)?;
    Ok(Fetched {
        root,
        origin: ExtensionSource::Registry {
            url: index_url.to_string(),
        },
    })
}

/// Install a fetched package through the store.
pub(crate) fn install_fetched(
    store: &ExtensionStore,
    fetched: &Fetched,
) -> Result<Package, ExtensionError> {
    store.install_with(&fetched.root, fetched.origin.clone())
}

fn sanitize(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    sanitized.trim_matches('-').chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;

    fn scratch(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("bsh-ext-source-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    fn package(directory: &Path) {
        std::fs::create_dir_all(directory.join("tools")).expect("tools");
        std::fs::write(
            directory.join(MANIFEST_FILE),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.archive"
name = "Archive"
version = "1.0.0"
description = "An archived test package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo"]
"#,
        )
        .expect("manifest");
        std::fs::write(directory.join("tools/echo.json"), "{}").expect("tool");
    }

    #[test]
    fn an_archive_extracts_to_its_package_root() {
        let root = scratch("archive");
        let source = root.join("source");
        package(&source);
        let archive_path = root.join("package.tar.gz");
        let file = std::fs::File::create(&archive_path).expect("archive file");
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        archive.append_dir_all("package", &source).expect("append");
        let encoder = archive.into_inner().expect("tar");
        encoder.finish().expect("finish");
        let fetched =
            resolve(archive_path.to_str().expect("path"), &root.join("cache")).expect("fetch");
        assert!(fetched.root.join(MANIFEST_FILE).is_file());
        assert!(fetched.root.ends_with("package") || fetched.root == source);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_archive_without_a_single_package_root_is_refused() {
        let root = scratch("rootless");
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).expect("empty");
        assert!(package_root_in(&empty).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
