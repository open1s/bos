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

use super::package::ExtensionStore;
use super::types::{ExtensionError, ExtensionSource};

/// The official provider repository.
pub(crate) const DEFAULT_GIT_MARKET: &str = "https://github.com/open1s/bsh_extensions";

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

fn validate_entry(entry: &MarketEntry, path: &Path) -> Result<(), ExtensionError> {
    super::types::ExtensionId::parse(entry.id.clone()).map_err(|error| {
        ExtensionError::Manifest {
            path: path.to_path_buf(),
            message: format!("market entry {}: {error}", entry.id),
        }
    })?;
    semver::Version::parse(&entry.version).map_err(|error| ExtensionError::Manifest {
        path: path.to_path_buf(),
        message: format!("market entry {} has invalid version: {error}", entry.id),
    })?;
    if entry.path.is_empty()
        || entry.path.starts_with('/')
        || entry.path.split('/').any(|segment| segment == "..")
    {
        return Err(ExtensionError::Manifest {
            path: path.to_path_buf(),
            message: format!(
                "market entry {} has an unsafe path {:?}",
                entry.id, entry.path
            ),
        });
    }
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
    ) -> Result<super::package::Package, ExtensionError> {
        validate_entry(entry, &self.cache.join("index.json"))?;
        let source = self.cache.join(&entry.path);
        if !source.join("extension.toml").is_file() {
            return Err(ExtensionError::Manifest {
                path: source,
                message: format!("market entry {} has no extension.toml", entry.id),
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

    #[test]
    fn an_escaping_path_is_refused() {
        let text = r#"{"schema_version":1,"extensions":[
            {"id":"com.example.hello","name":"Hello","version":"1.0.0","path":"../outside"}
        ]}"#;
        assert!(MarketIndex::parse(text, Path::new("index.json")).is_err());
    }
}
