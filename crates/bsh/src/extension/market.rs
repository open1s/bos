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

use ferris_sdk::package::{ExtensionStore, Package};
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

/// A shallow checkout of one provider.
#[derive(Debug, Clone)]
pub(crate) struct GitMarket {
    cache: PathBuf,
    url: String,
    revision: String,
}

impl GitMarket {
    /// The checkout under `root/market/<repository name>`.
    pub(crate) fn open(root: &Path, url: &str) -> Self {
        let name = url
            .trim_end_matches(".git")
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("market");
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
                .args([
                    "clone",
                    "--depth",
                    "1",
                    "--single-branch",
                    &self.url,
                    &self.cache.to_string_lossy(),
                ])
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
        // must be the hash of the bytes that install.
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
        if let Some(hash) = entry.hash.as_deref().filter(|hash| !hash.is_empty()) {
            if declared.hash != hash {
                return Err(ExtensionError::Denied {
                    extension: entry.id.clone(),
                    capability: "package.integrity".to_string(),
                    reason: "market package does not match the hash its index declares".to_string(),
                });
            }
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
    fn an_escaping_path_is_refused() {
        let text = r#"{"schema_version":1,"extensions":[
            {"id":"com.example.hello","name":"Hello","version":"1.0.0","path":"../outside"}
        ]}"#;
        assert!(MarketIndex::parse(text, Path::new("index.json")).is_err());
    }
}
