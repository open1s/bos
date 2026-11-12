//! A Git-backed extension marketplace.
//!
//! The provider is a Git repository with an `index.json` at its root. The host
//! clones it shallowly into the managed extension root and reads the index; no
//! JavaScript registry, no Codex/DSH package shim. Installing an entry copies
//! the package directory named by the index into the normal hashed store with
//! community trust, so the same lifecycle, policy and audit apply.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use ferris_sdk::package::{package_hash_at_rev, ExtensionStore, Package};
use ferris_sdk::types::{ExtensionError, ExtensionSource, TrustLevel};

/// The official provider repository.
pub(crate) const DEFAULT_GIT_MARKET: &str = "https://github.com/open1s/bsh_extensions";

/// The market URL for this launch: environment override, then the reader's
/// setting, then the official provider. The environment wins so a developer
/// can test a local provider without editing the settings file.
pub(crate) fn configured_url(settings: &crate::settings::Settings) -> String {
    if let Ok(url) = std::env::var("BSH_EXTENSION_MARKET") {
        let url = url.trim();
        if !url.is_empty() {
            return url.to_string();
        }
    }
    let configured = settings.global.extension_market_url.trim();
    if configured.is_empty() {
        DEFAULT_GIT_MARKET.to_string()
    } else {
        configured.to_string()
    }
}

/// One package advertised by the provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct MarketEntry {
    /// Reverse-domain extension id.
    pub(crate) id: String,
    /// Display name.
    pub(crate) name: String,
    /// Version string.
    pub(crate) version: String,
    /// One-line description.
    #[serde(default)]
    pub(crate) description: String,
    /// Package directory, relative to the repository root.
    pub(crate) path: String,
    /// Optional SHA-256 of the package directory.
    #[serde(default)]
    pub(crate) hash: Option<String>,
}

/// The provider's index document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct MarketIndex {
    /// Index schema, exactly 1.
    pub(crate) schema_version: u32,
    /// Advertised packages.
    #[serde(default)]
    pub(crate) extensions: Vec<MarketEntry>,
}

impl MarketIndex {
    /// Parse and validate one index document.
    pub(crate) fn parse(text: &str, path: &Path) -> Result<Self, ExtensionError> {
        let index: Self = serde_json::from_str(text).map_err(|error| ExtensionError::Json {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if index.schema_version != 1 {
            return Err(ExtensionError::Manifest {
                path: path.to_path_buf(),
                message: format!(
                    "unsupported market index schema {}; this host reads 1",
                    index.schema_version
                ),
            });
        }
        for entry in &index.extensions {
            validate_entry(entry, path)?;
        }
        Ok(index)
    }

    /// Entries whose id, name or description contains `query`.
    pub(crate) fn matching(&self, query: &str) -> Vec<&MarketEntry> {
        let query = query.trim().to_ascii_lowercase();
        self.extensions
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.id.to_ascii_lowercase().contains(&query)
                    || entry.name.to_ascii_lowercase().contains(&query)
                    || entry.description.to_ascii_lowercase().contains(&query)
            })
            .collect()
    }

    /// The advertised entry with this exact id and version.
    pub(crate) fn entry(&self, id: &str, version: &str) -> Option<&MarketEntry> {
        self.extensions
            .iter()
            .find(|entry| entry.id == id && entry.version == version)
    }

    /// The newest advertised version of one id.
    pub(crate) fn newest(&self, id: &str) -> Option<&MarketEntry> {
        let mut best: Option<(&MarketEntry, semver::Version)> = None;
        for entry in self.extensions.iter().filter(|entry| entry.id == id) {
            let Ok(version) = semver::Version::parse(&entry.version) else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, current)| version > *current) {
                best = Some((entry, version));
            }
        }
        best.map(|(entry, _)| entry)
    }
}

/// Order two version strings; unparsable ones sort lexically.
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(left), semver::Version::parse(right)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// Group entries by id in first-mention order, versions newest first.
///
/// An index advertises one entry per version, so the same id appears once for
/// each release. One row per extension is what the reader installs, and the
/// version list under it is the choice that row now offers.
pub(crate) fn group_by_id(entries: &[MarketEntry]) -> Vec<Vec<MarketEntry>> {
    let mut groups: Vec<Vec<MarketEntry>> = Vec::new();
    let mut positions: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for entry in entries {
        match positions.get(entry.id.as_str()) {
            Some(&at) => groups[at].push(entry.clone()),
            None => {
                positions.insert(entry.id.as_str(), groups.len());
                groups.push(vec![entry.clone()]);
            }
        }
    }
    for group in &mut groups {
        group.sort_by(|left, right| compare_versions(&right.version, &left.version));
    }
    groups
}

/// The versions one row offers, newest first.
///
/// The index's entries come first, then any version already on disk: a local
/// install can outlive its index entry, and the reader must still be able to
/// switch back to it instead of only seeing what the provider still advertises.
pub(crate) fn version_choices(entries: &[MarketEntry], installed: &[String]) -> Vec<String> {
    let mut versions: Vec<String> = entries
        .iter()
        .map(|entry| entry.version.clone())
        .chain(installed.iter().cloned())
        .collect();
    versions.sort_by(|left, right| compare_versions(right, left));
    versions.dedup();
    versions
}

/// The version a row shows: the reader's pick while it is still offered, else
/// the version the host resolves, else the newest offered.
pub(crate) fn resolve_version(
    choice: Option<&str>,
    active: Option<&str>,
    offered: &[String],
) -> Option<String> {
    let is_offered = |version: &&str| offered.iter().any(|candidate| candidate == version);
    choice
        .filter(is_offered)
        .or_else(|| active.filter(is_offered))
        .map(str::to_string)
        .or_else(|| offered.first().cloned())
}

fn validate_entry(entry: &MarketEntry, path: &Path) -> Result<(), ExtensionError> {
    ferris_sdk::types::ExtensionId::parse(entry.id.clone()).map_err(|error| {
        ExtensionError::Manifest {
            path: path.to_path_buf(),
            message: format!("market entry {}: {error}", entry.id),
        }
    })?;
    semver::Version::parse(&entry.version).map_err(|error| ExtensionError::Manifest {
        path: path.to_path_buf(),
        message: format!("market entry {} has invalid version: {error}", entry.id),
    })?;
    // The same grammar every other package-relative path must satisfy: it
    // rejects absolute paths, "." / ".." components and backslashes (which a
    // split-on-'/' check let through on Windows).
    ferris_sdk::core_manifest::validate_relative_path(&entry.path).map_err(|reason| {
        ExtensionError::Manifest {
            path: path.to_path_buf(),
            message: format!(
                "market entry {} has an unsafe path {:?}: {reason}",
                entry.id, entry.path
            ),
        }
    })?;
    Ok(())
}

/// The refusal a package earns when its bytes disagree with the index.
///
/// Both digests travel with it: the reader's next step is to work out which
/// side moved (an index built from a working tree, or a package replaced after
/// publication), and naming only the fact turns that into a hunt through the
/// provider's history.
fn integrity_denial(extension: &str, declared: &str, actual: &str) -> ExtensionError {
    ExtensionError::Denied {
        extension: extension.to_string(),
        capability: "package.integrity".to_string(),
        reason: format!(
            "market package does not match the hash its index declares (index {declared}, package {actual})"
        ),
    }
}

/// A deterministic, filesystem-safe cache name for a repository URL.
///
/// Normalizes away the scheme, query, fragment, trailing slashes and a `.git`
/// suffix, keeps the last two identity segments (owner/repository) sanitized
/// to `[A-Za-z0-9._-]`, and appends a short digest of the full normalized
/// identity so two repositories that sanitize alike never share a checkout.
/// No component can be `.`, `..`, empty, or carry a separator or a
/// Windows-reserved character, so the joined path stays under the market root
/// on every host (issue #305).
fn cache_identity(url: &str) -> String {
    use sha2::Digest as _;
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let trimmed = without_query.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    // The path portion: after scheme://authority, or after the ':' of an
    // scp-style git@host:owner/repo. A bare path is used as written.
    let path = if let Some(rest) = trimmed.split_once("://") {
        rest.1.split_once('/').map(|(_, p)| p).unwrap_or("")
    } else if let Some((_, rest)) = trimmed.split_once(':') {
        rest
    } else {
        trimmed
    };
    let segments: Vec<String> = path
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .map(|seg| {
            seg.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .trim_matches('.')
                .to_string()
        })
        .filter(|seg| !seg.is_empty())
        .collect();
    let tail: Vec<String> = segments.iter().rev().take(2).rev().cloned().collect();
    let digest = sha2::Sha256::digest(trimmed.as_bytes());
    let suffix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let base = if tail.is_empty() {
        "market".to_string()
    } else {
        tail.join("-")
    };
    // A Windows reserved stem (CON, NUL, ...) would be unsafe as a directory
    // name; the digest suffix already prevents a bare reserved name, but keep
    // the base from being exactly one by construction.
    format!("{base}-{suffix}")
}

/// A shallow checkout of one provider.
#[derive(Debug, Clone)]
pub(crate) struct GitMarket {
    cache: PathBuf,
    url: String,
    revision: String,
}

impl GitMarket {
    /// The checkout under `root/market/<repository identity>`.
    ///
    /// The directory name is derived from the validated repository identity,
    /// never from a raw URL substring (issue #305): query/fragment text, dot
    /// segments, separators and platform-reserved characters cannot become a
    /// path component, and equivalent URLs normalize to the same name.
    pub(crate) fn open(root: &Path, url: &str) -> Self {
        let name = cache_identity(url);
        Self {
            cache: root.join("market").join(name),
            url: url.to_string(),
            revision: String::new(),
        }
    }

    /// The provider URL.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// The checkout path.
    pub(crate) fn path(&self) -> &Path {
        &self.cache
    }

    /// Read the index from the existing checkout without touching the network.
    pub(crate) fn cached(&self) -> Result<MarketIndex, ExtensionError> {
        let index = self.cache.join("index.json");
        let text = std::fs::read_to_string(&index).map_err(|source| ExtensionError::Io {
            path: index.clone(),
            source,
        })?;
        MarketIndex::parse(&text, &index)
    }

    /// Clone or refresh the checkout and read its index.
    pub(crate) fn sync(&mut self) -> Result<MarketIndex, ExtensionError> {
        if self.cache.join(".git").is_dir() {
            self.git(&["fetch", "--depth", "1", "origin"])?;
            self.git(&["reset", "--hard", "FETCH_HEAD"])?;
        } else {
            if self.cache.exists() {
                std::fs::remove_dir_all(&self.cache).map_err(|source| ExtensionError::Io {
                    path: self.cache.clone(),
                    source,
                })?;
            }
            let parent = self.cache.parent().unwrap_or(Path::new("."));
            std::fs::create_dir_all(parent).map_err(|source| ExtensionError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
            let status = Command::new("git")
                .args(["clone", "--depth", "1", "--single-branch", &self.url])
                .arg(&self.cache)
                .env("GIT_TERMINAL_PROMPT", "0")
                .status()
                .map_err(|source| ExtensionError::Io {
                    path: self.cache.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(ExtensionError::Manifest {
                    path: self.cache.clone(),
                    message: format!("git clone {} failed with {status}", self.url),
                });
            }
        }
        self.revision = String::from_utf8_lossy(&self.output(&["rev-parse", "HEAD"])?)
            .trim()
            .to_string();
        let index = self.cache.join("index.json");
        let text = std::fs::read_to_string(&index).map_err(|source| ExtensionError::Io {
            path: index.clone(),
            source,
        })?;
        MarketIndex::parse(&text, &index)
    }

    /// Install one entry from the current checkout.
    pub(crate) fn install(
        &self,
        store: &ExtensionStore,
        entry: &MarketEntry,
    ) -> Result<ferris_sdk::package::Package, ExtensionError> {
        validate_entry(entry, &self.cache.join("index.json"))?;
        let source = self.cache.join(&entry.path);
        if !source.join("extension.toml").is_file() {
            return Err(ExtensionError::Manifest {
                path: source,
                message: format!("market entry {} has no extension.toml", entry.id),
            });
        }
        // The index is not trusted to describe what the directory contains: the
        // installed id and version must match the entry, and a declared hash
        // must be the hash of the commit that install. The check asks the same
        // function the index was built with (the revision, not the checkout).
        let declared = Package::load(&source, ExtensionSource::Development, TrustLevel::Community)?;
        if declared.id().as_str() != entry.id || declared.version() != entry.version {
            return Err(ExtensionError::Manifest {
                path: source,
                message: format!(
                    "market entry {} {} does not match the package it names ({} {})",
                    entry.id,
                    entry.version,
                    declared.id(),
                    declared.version()
                ),
            });
        }
        let committed = package_hash_at_rev(&source, "HEAD")?;
        if let Some(hash) = entry.hash.as_deref().filter(|hash| !hash.is_empty()) {
            if committed != hash {
                return Err(integrity_denial(&entry.id, hash, &committed));
            }
        }
        // The store installs the checkout's bytes, not the commit's blobs, so a
        // checkout that drifted from its commit is refused even when the index
        // agrees with the commit: those bytes are not the ones it describes.
        if !declared.hash.is_empty() && declared.hash != committed {
            return Err(ExtensionError::Denied {
                extension: entry.id.clone(),
                capability: "package.integrity".to_string(),
                reason: format!(
                    "the market checkout does not match the commit the index describes (commit {committed}, checkout {})",
                    declared.hash
                ),
            });
        }
        store.install_community(
            &source,
            ExtensionSource::Git {
                repository: self.url.clone(),
                revision: self.revision.clone(),
            },
        )
    }

    fn git(&self, args: &[&str]) -> Result<(), ExtensionError> {
        let _ = self.output(args)?;
        Ok(())
    }

    fn output(&self, args: &[&str]) -> Result<Vec<u8>, ExtensionError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.cache)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|source| ExtensionError::Io {
                path: self.cache.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(ExtensionError::Manifest {
                path: self.cache.clone(),
                message: format!(
                    "git {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(output.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_is_validated_and_searched() {
        let text = r#"{"schema_version":1,"extensions":[
            {"id":"com.example.hello","name":"Hello","version":"1.0.0","path":"packages/hello"},
            {"id":"com.example.git","name":"Git","version":"1.0.0","path":"packages/git"}
        ]}"#;
        let index = MarketIndex::parse(text, Path::new("index.json")).expect("index");
        assert_eq!(index.matching("git").len(), 1);
        assert_eq!(index.matching("").len(), 2);
        assert_eq!(
            index.newest("com.example.hello").expect("hello").name,
            "Hello"
        );
    }

    fn entry(id: &str, version: &str) -> MarketEntry {
        MarketEntry {
            id: id.to_string(),
            name: "Extension".to_string(),
            version: version.to_string(),
            description: String::new(),
            path: format!("packages/{id}/{version}"),
            hash: None,
        }
    }

    #[test]
    fn one_id_is_one_group_with_its_versions_newest_first() {
        let entries = vec![
            entry("com.example.devkit", "1.1.0"),
            entry("com.example.other", "1.0.0"),
            entry("com.example.devkit", "2.0.0"),
        ];
        let groups = group_by_id(&entries);
        assert_eq!(groups.len(), 2, "two ids make two rows, not three");
        assert_eq!(groups[0][0].id, "com.example.devkit");
        assert_eq!(
            groups[0]
                .iter()
                .map(|entry| entry.version.as_str())
                .collect::<Vec<_>>(),
            ["2.0.0", "1.1.0"],
            "the row offers the newest version first"
        );
    }

    #[test]
    fn the_choices_include_an_installed_version_the_index_dropped() {
        let entries = vec![
            entry("com.example.devkit", "2.0.0"),
            entry("com.example.devkit", "1.1.0"),
        ];
        let choices = version_choices(&entries, &["1.0.0".to_string()]);
        assert_eq!(choices, ["2.0.0", "1.1.0", "1.0.0"]);
    }

    #[test]
    fn a_row_shows_the_pick_then_the_active_version_then_the_newest() {
        let offered = ["2.0.0".to_string(), "1.1.0".to_string()];
        assert_eq!(
            resolve_version(Some("1.1.0"), Some("2.0.0"), &offered).as_deref(),
            Some("1.1.0"),
            "the reader's pick wins while it is still offered"
        );
        assert_eq!(
            resolve_version(Some("9.9.9"), Some("1.1.0"), &offered).as_deref(),
            Some("1.1.0"),
            "a vanished pick falls back to the version the host resolves"
        );
        assert_eq!(
            resolve_version(None, None, &offered).as_deref(),
            Some("2.0.0"),
            "an uninstalled extension starts on its newest version"
        );
    }

    #[test]
    fn a_hash_mismatch_names_both_digests() {
        let error = integrity_denial("com.example.devkit", "aaaa", "bbbb");
        let text = error.to_string();
        assert!(text.contains("aaaa"), "the index's digest is named: {text}");
        assert!(text.contains("bbbb"), "and the package's own: {text}");
        assert!(
            text.contains("package.integrity"),
            "under the denied capability: {text}"
        );
    }

    #[test]
    fn an_escaping_path_is_refused() {
        let text = r#"{"schema_version":1,"extensions":[
            {"id":"com.example.hello","name":"Hello","version":"1.0.0","path":"../outside"}
        ]}"#;
        assert!(MarketIndex::parse(text, Path::new("index.json")).is_err());
    }

    fn manifest_v2() -> String {
        r#"
schema_version = "2.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "1.0.0"
description = "A market install fixture"
authors = ["BSH"]
license = "MIT"

[compatibility]
host = ">=3.0.0, <9.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]
"#
        .to_string()
    }

    fn git(root: &Path, args: &[&str]) {
        std::fs::create_dir_all(root).expect("git root");
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
    fn the_index_hash_is_the_commits_and_checkout_drift_is_refused() {
        let root = std::env::temp_dir().join(format!("bsh-market-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let package = repo
            .join("packages")
            .join("com.example.hello")
            .join("1.0.0");
        std::fs::create_dir_all(package.join("tools")).expect("package dirs");
        std::fs::write(package.join("extension.toml"), manifest_v2()).expect("manifest");
        std::fs::write(package.join("tools").join("echo.json"), "{}").expect("tool");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(
            &repo,
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

        let committed = package_hash_at_rev(&package, "HEAD").expect("commit hash");
        let market = GitMarket {
            cache: repo.clone(),
            url: "file://test".to_string(),
            revision: committed.clone(),
        };
        let mut entry = MarketEntry {
            id: "com.example.hello".to_string(),
            name: "Hello".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            path: "packages/com.example.hello/1.0.0".to_string(),
            hash: Some(committed.clone()),
        };

        let store = ExtensionStore::open(root.join("store-a")).expect("store");
        market.install(&store, &entry).expect("the commit installs");

        // An uncommitted edit is bytes the index never described, even though
        // the commit and therefore the index still hash the same.
        std::fs::write(
            package.join("tools").join("echo.json"),
            "{\"changed\":true}",
        )
        .expect("edit");
        let store = ExtensionStore::open(root.join("store-b")).expect("store");
        let error = market
            .install(&store, &entry)
            .expect_err("a drifted checkout is refused")
            .to_string();
        assert!(error.contains("does not match the commit"), "{error}");

        // A wrong declared digest is named alongside the commit's.
        entry.hash = Some("0".repeat(64));
        let store = ExtensionStore::open(root.join("store-c")).expect("store");
        let error = market
            .install(&store, &entry)
            .expect_err("a wrong digest is refused")
            .to_string();
        assert!(error.contains(&"0".repeat(64)), "{error}");
        assert!(error.contains(&committed), "{error}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
