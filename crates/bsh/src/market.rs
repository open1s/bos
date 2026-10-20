//! The plugin market: local Codex/DSH packages and remote format-1 market
//! indexes, lowered onto the same [`MarketPlugin`] row the Extensions pane
//! draws.
//!
//! Local sources:
//!
//! - Codex: [`~/.codex/plugins/cache`], [`~/.codex/.tmp/bundled-marketplaces`],
//!   and [`~/.codex/.tmp/plugins/plugins`];
//! - DSH: each [`~/.dsh/profiles/<profile>/package.json`] dependency or
//!   [`dsh.profile.bundles`] entry whose package has a [`dsh`] manifest,
//!   excluding the [`@deepseek-ai/`] namespace (those are the runtime's own
//!   packages, not installed plugins).
//!
//! Remote sources are the DeepSeek Harness plugin-market index format 1: a JSON
//! document with [`format: 1`], an index title, and plugins whose versions name
//! an npm package or a commit-pinned Git repository. The format is the one
//! validated by [`@deepseek-ai/dsh-plugin-market-index`]; this module reads the
//! subset the market pane needs (identity, description, keywords, versions,
//! source) and refuses a malformed document by naming the offending path.
//!
//! Discovery parses each local package through [`agent::extensions::ExtensionRegistry`],
//! so the row is the same metadata the loader will use and a package that cannot
//! be read is reported instead of listed under a guessed name.
//!
//! Two catalog providers ship built in: dshmarket (displayed at
//! `https://dshmarket.com/browse/`, read from its published catalog JSON) and
//! Codex Marketplace (displayed at `https://www.codex-marketplace.com/`, read
//! from its `/api/plugins` endpoint). The dsh catalog contributes its npm
//! entries; the Codex catalog contributes plugin entries with a GitHub
//! repository and package path. Standalone Codex skills and hooks are not
//! plugin rows.
//!
//! A remote install runs npm (or pnpm for a Git package subdirectory) with
//! `--ignore-scripts` into `~/.bos/bsh/extensions`, then registers the installed
//! package directory in the settings inventory. Uninstalling removes that
//! managed directory; a local package is never deleted.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use agent::extensions::{Capability, ExtensionRegistry, ExtensionStandard, RefusedFeature};
use serde_json::Value;

/// Where a market row came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarketOrigin {
    /// A package already installed on this machine.
    Local,
    /// One entry of a configured remote index.
    Remote {
        /// The index URL the entry was read from.
        index_url: String,
        /// The index title, for the card's small print.
        market: String,
    },
}

/// How a market row is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarketInstall {
    /// The local package directory is already on disk; register it.
    Local(PathBuf),
    /// Download the exact package spec into the managed extension directory.
    Remote {
        /// The npm or commit-pinned Git spec.
        spec: String,
        /// The registry the index names, when it names one.
        registry: Option<String>,
    },
    /// Check out one directory of a GitHub repository into the managed root.
    GitHub {
        /// A github.com repository URL.
        repository: String,
        /// Package directory inside the repository.
        path: String,
    },
}

/// One plugin the market can install.
#[derive(Debug, Clone)]
pub(crate) struct MarketPlugin {
    /// Package id, also the settings inventory id when installed.
    pub(crate) id: String,
    /// Display name.
    pub(crate) name: String,
    /// Package version.
    pub(crate) version: String,
    /// Which standard the package was read as.
    pub(crate) standard: ExtensionStandard,
    /// Where the row came from.
    pub(crate) origin: MarketOrigin,
    /// How to install it.
    pub(crate) install: MarketInstall,
    /// Display description.
    pub(crate) description: String,
    /// Search keywords (remote indexes declare these).
    pub(crate) keywords: Vec<String>,
    /// Declared capabilities.
    pub(crate) capabilities: Vec<Capability>,
    /// How many non-MCP parts the shim had to refuse (for example a Codex app
    /// connector). Declared MCP servers are started by BSH's MCP pool.
    pub(crate) refusals: usize,
    /// Where the row came from, for the card's small print.
    pub(crate) source_label: String,
}

impl MarketPlugin {
    /// Whether this row came from a remote index.
    pub(crate) fn is_remote(&self) -> bool {
        matches!(self.origin, MarketOrigin::Remote { .. })
    }
}

/// One discovery pass.
#[derive(Debug, Clone, Default)]
pub(crate) struct MarketScan {
    /// Plugins found, ordered by standard and name.
    pub(crate) plugins: Vec<MarketPlugin>,
    /// Sources that carried a plugin marker but could not be read.
    pub(crate) errors: Vec<String>,
}

/// Scan the local market rooted at the user's home directory.
pub(crate) fn discover() -> MarketScan {
    let Some(home) = dirs::home_dir() else {
        return MarketScan {
            plugins: Vec::new(),
            errors: vec!["no home directory".to_string()],
        };
    };
    discover_in(&home)
}

/// Scan a market rooted at [`home`], for tests and for a sandboxed HOME.
pub(crate) fn discover_in(home: &Path) -> MarketScan {
    let mut scan = MarketScan::default();
    let mut seen: HashSet<(ExtensionStandard, String)> = HashSet::new();
    for (path, label) in codex_candidates(home) {
        add_candidate(&mut scan, &mut seen, &path, label);
    }
    for (path, label) in dsh_candidates(home, &mut scan.errors) {
        add_candidate(&mut scan, &mut seen, &path, &label);
    }
    sort_market(&mut scan.plugins);
    scan
}

/// Order market rows: remote-index rows first, then local rows, each group by
/// source label and display name.
pub(crate) fn sort_market(plugins: &mut [MarketPlugin]) {
    plugins.sort_by(|left, right| {
        right
            .is_remote()
            .cmp(&left.is_remote())
            .then_with(|| left.source_label.cmp(&right.source_label))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn add_candidate(
    scan: &mut MarketScan,
    seen: &mut HashSet<(ExtensionStandard, String)>,
    path: &Path,
    label: &str,
) {
    if !path.is_dir() {
        return;
    }
    match ExtensionRegistry::load(path) {
        Ok(package) => {
            let extension = package.extension;
            if !seen.insert((extension.standard, extension.id.clone())) {
                return;
            }
            let description = extension
                .description
                .clone()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| extension.name.clone());
            scan.plugins.push(MarketPlugin {
                id: extension.id,
                name: extension.name,
                version: extension.version,
                standard: extension.standard,
                origin: MarketOrigin::Local,
                install: MarketInstall::Local(path.to_path_buf()),
                description,
                keywords: Vec::new(),
                capabilities: extension.capabilities,
                // BSH starts a package's MCP servers itself (mcp::extension_servers),
                // so the shim's MCP refusal is not a user-visible gap here.
                refusals: package
                    .refusals
                    .iter()
                    .filter(|refusal| refusal.feature != RefusedFeature::McpServer)
                    .count(),
                source_label: label.to_string(),
            });
        }
        Err(error) => scan.errors.push(format!("{}: {error}", path.display())),
    }
}

/// Largest remote index body this client accepts.
const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;

/// One validated remote market index.
#[derive(Debug, Clone)]
pub(crate) struct RemoteIndex {
    /// The URL the document was read from.
    pub(crate) url: String,
    /// The market title, English.
    pub(crate) title: String,
    /// Plugins in document order.
    pub(crate) plugins: Vec<RemotePlugin>,
}

/// One plugin in a remote index.
#[derive(Debug, Clone)]
pub(crate) struct RemotePlugin {
    /// npm package name.
    pub(crate) name: String,
    /// English display title.
    pub(crate) title: String,
    /// English description.
    pub(crate) description: String,
    /// Search keywords.
    pub(crate) keywords: Vec<String>,
    /// Versions, highest first.
    pub(crate) versions: Vec<RemoteVersion>,
}

/// One version of a remote plugin.
#[derive(Debug, Clone)]
pub(crate) struct RemoteVersion {
    /// Exact SemVer version.
    pub(crate) version: String,
    /// Where the version can be installed from.
    pub(crate) source: RemoteSource,
}

/// The install source of one remote version.
#[derive(Debug, Clone)]
pub(crate) enum RemoteSource {
    /// An npm package, optionally from a named registry.
    Npm {
        /// The registry URL the index names.
        registry: Option<String>,
    },
    /// A commit-pinned Git repository, optionally below its root.
    Git {
        /// Repository URL.
        url: String,
        /// Lowercase 40-hex commit.
        commit: String,
        /// Package directory inside the repository.
        path: Option<String>,
    },
}

impl RemotePlugin {
    /// The version the market offers: the highest non-prerelease version, else
    /// the highest prerelease.
    pub(crate) fn default_version(&self) -> Option<&RemoteVersion> {
        self.versions
            .iter()
            .find(|version| !version.version.contains('-'))
            .or_else(|| self.versions.first())
    }

    /// The exact install target for the default version.
    pub(crate) fn install_target(&self) -> Option<MarketInstall> {
        let version = self.default_version()?;
        let install = match &version.source {
            RemoteSource::Npm { registry } => MarketInstall::Remote {
                spec: format!("{}@{}", self.name, version.version),
                registry: registry.clone(),
            },
            RemoteSource::Git { url, commit, path } => {
                let repository = if url.ends_with(".git") {
                    url.clone()
                } else {
                    format!("{url}.git")
                };
                let mut spec = format!("git+{repository}#{commit}");
                if let Some(path) = path {
                    spec.push_str(&format!("&path:/{path}"));
                }
                MarketInstall::Remote {
                    spec,
                    registry: None,
                }
            }
        };
        Some(install)
    }
}

/// Turn one fetched index into market rows.
pub(crate) fn remote_market_plugins(index: &RemoteIndex) -> Vec<MarketPlugin> {
    index
        .plugins
        .iter()
        .filter_map(|plugin| {
            let install = plugin.install_target()?;
            let version = plugin.default_version()?.version.clone();
            Some(MarketPlugin {
                id: plugin.name.clone(),
                name: plugin.title.clone(),
                version,
                standard: ExtensionStandard::DeepSeekHarness,
                origin: MarketOrigin::Remote {
                    index_url: index.url.clone(),
                    market: index.title.clone(),
                },
                install,
                description: plugin.description.clone(),
                keywords: plugin.keywords.clone(),
                capabilities: Vec::new(),
                refusals: 0,
                source_label: format!("Remote · {}", index.title),
            })
        })
        .collect()
}

/// Validate and normalize one configured index URL.
pub(crate) fn validate_index_url(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("an index URL is required".to_string());
    }
    let url = reqwest::Url::parse(text).map_err(|error| format!("invalid URL: {error}"))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => return Err(format!("unsupported scheme '{scheme}'; use http or https")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the index URL must not contain credentials".to_string());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("the index URL must not contain a query or fragment".to_string());
    }
    Ok(url.to_string())
}

/// Fetch and validate one remote index.
pub(crate) async fn fetch_index(raw: &str) -> Result<RemoteIndex, String> {
    let url = validate_index_url(raw)?;
    let bytes = fetch_bytes(&url, MAX_INDEX_BYTES).await?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("{url} is not JSON: {error}"))?;
    parse_remote_index(&value, &url)
}

/// One catalog provider BSH ships with, independent of the user's own index list.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BuiltinProvider {
    /// Display name shown on the source row.
    pub(crate) title: &'static str,
    /// The URL the request names (shown to the reader).
    pub(crate) display_url: &'static str,
    /// The JSON endpoint actually fetched.
    pub(crate) fetch_url: &'static str,
    /// The catalog shape the endpoint serves.
    pub(crate) format: CatalogFormat,
}

/// A built-in catalog's JSON shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogFormat {
    /// dshmarket's catalog: plugins with npm name and version.
    DshCatalog,
    /// Codex Marketplace's catalog: plugins with a GitHub repository and path.
    CodexCatalog,
}

/// The two built-in providers, in display order.
pub(crate) const BUILTIN_PROVIDERS: &[BuiltinProvider] = &[
    BuiltinProvider {
        title: "dshmarket",
        display_url: "https://dshmarket.com/browse/",
        fetch_url: "https://awesome-dsh-plugin.com/plugins.json",
        format: CatalogFormat::DshCatalog,
    },
    BuiltinProvider {
        title: "Codex Marketplace",
        display_url: "https://www.codex-marketplace.com/",
        fetch_url: "https://www.codex-marketplace.com/api/plugins?limit=1000",
        format: CatalogFormat::CodexCatalog,
    },
];

/// Fetch and parse one built-in catalog.
pub(crate) async fn fetch_builtin(provider: &BuiltinProvider) -> Result<Vec<MarketPlugin>, String> {
    let bytes = fetch_bytes(provider.fetch_url, 32 * 1024 * 1024).await?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not JSON: {error}", provider.fetch_url))?;
    match provider.format {
        CatalogFormat::DshCatalog => parse_dsh_catalog(&value, provider),
        CatalogFormat::CodexCatalog => parse_codex_catalog(&value, provider),
    }
}

fn parse_dsh_catalog(
    value: &Value,
    provider: &BuiltinProvider,
) -> Result<Vec<MarketPlugin>, String> {
    let plugins = value
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| "the dshmarket catalog has no plugins array".to_string())?;
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for plugin in plugins {
        let Some(npm) = plugin
            .get("npm")
            .and_then(Value::as_str)
            .filter(|npm| valid_package_name(npm))
        else {
            // The catalog also lists GitHub-only entries; they have no npm
            // package the installer can fetch.
            continue;
        };
        if !seen.insert(npm.to_string()) {
            continue;
        }
        let name = plugin
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(npm)
            .to_string();
        let version = plugin
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let spec = if version.is_empty() {
            npm.to_string()
        } else {
            format!("{npm}@{version}")
        };
        let description = optional_localized(plugin.get("description"), "description")
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut keywords: Vec<String> = plugin
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(category) = plugin.get("category").and_then(Value::as_str) {
            keywords.push(category.to_string());
        }
        rows.push(MarketPlugin {
            id: npm.to_string(),
            name,
            version,
            standard: ExtensionStandard::DeepSeekHarness,
            origin: MarketOrigin::Remote {
                index_url: provider.display_url.to_string(),
                market: provider.title.to_string(),
            },
            install: MarketInstall::Remote {
                spec,
                registry: None,
            },
            description,
            keywords,
            capabilities: Vec::new(),
            refusals: 0,
            source_label: provider.title.to_string(),
        });
    }
    Ok(rows)
}

fn parse_codex_catalog(
    value: &Value,
    provider: &BuiltinProvider,
) -> Result<Vec<MarketPlugin>, String> {
    let plugins = value
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| "the Codex catalog has no plugins array".to_string())?;
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for plugin in plugins {
        // The endpoint also lists standalone skills and hooks; this market
        // installs plugin packages only.
        if plugin.get("type").and_then(Value::as_str) != Some("plugin") {
            continue;
        }
        let Some(slug) = plugin.get("slug").and_then(Value::as_str) else {
            continue;
        };
        let Some(repository) = plugin.get("repository").and_then(Value::as_str) else {
            continue;
        };
        let repository = match validate_github_repository(repository) {
            Ok(repository) => repository,
            Err(_) => continue,
        };
        let raw_path = plugin
            .get("pluginPath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let path = if raw_path.is_empty() {
            String::new()
        } else {
            match validate_package_path(raw_path) {
                Ok(path) => path,
                Err(_) => continue,
            }
        };
        if !seen.insert(slug.to_string()) {
            continue;
        }
        let name = plugin
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or(slug)
            .to_string();
        let version = plugin
            .get("version")
            .and_then(Value::as_str)
            .filter(|version| !version.trim().is_empty())
            .unwrap_or("latest")
            .to_string();
        let description = plugin
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let keywords: Vec<String> = plugin
            .get("keywords")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        rows.push(MarketPlugin {
            id: slug.to_string(),
            name,
            version,
            standard: ExtensionStandard::Codex,
            origin: MarketOrigin::Remote {
                index_url: provider.display_url.to_string(),
                market: provider.title.to_string(),
            },
            install: MarketInstall::GitHub { repository, path },
            description,
            keywords,
            capabilities: Vec::new(),
            refusals: 0,
            source_label: provider.title.to_string(),
        });
    }
    Ok(rows)
}

async fn fetch_bytes(url: &str, max: usize) -> Result<Vec<u8>, String> {
    // The Codex endpoint has been observed closing a body mid-read under load.
    // One retry turns that transient failure into a usable catalog instead of a
    // red banner the reader cannot act on.
    let mut last = String::new();
    for attempt in 0..2 {
        match fetch_bytes_once(url, max).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                last = error;
                if attempt == 0 {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            }
        }
    }
    Err(last)
}

async fn fetch_bytes_once(url: &str, max: usize) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("HTTP client: {error}"))?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, "BOS-Harness market")
        .send()
        .await
        .map_err(|error| format!("fetching {url}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("fetching {url}: HTTP {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("reading {url}: {error}"))?;
    if bytes.len() > max {
        return Err(format!(
            "{url} is {} bytes; the limit is {max}",
            bytes.len()
        ));
    }
    Ok(bytes.to_vec())
}

/// Check out one directory of a GitHub repository into `root`.
///
/// A sparse, blob-filtered clone downloads only the committed tree and the
/// requested package's blobs; the checkout is removed after the package is
/// copied. `--ignore-scripts` does not apply here because no package manager
/// runs: the checkout is data.
pub(crate) fn install_github(
    root: &Path,
    name: &str,
    repository: &str,
    path: &str,
) -> Result<PathBuf, String> {
    let repository = validate_github_repository(repository)?;
    let path = if path.trim().is_empty() {
        None
    } else {
        Some(validate_package_path(path)?)
    };
    let git = find_program("BOS_GIT", &["git"])
        .ok_or_else(|| "git was not found; install Git to use the Codex Marketplace".to_string())?;
    let prefix = root.join(safe_directory_name(name));
    if prefix.exists() {
        std::fs::remove_dir_all(&prefix)
            .map_err(|error| format!("clearing {}: {error}", prefix.display()))?;
    }
    std::fs::create_dir_all(&prefix)
        .map_err(|error| format!("creating {}: {error}", prefix.display()))?;
    let checkout = prefix.join("checkout");
    let mut clone = Command::new(&git);
    clone
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["clone", "--depth", "1"]);
    if path.is_some() {
        clone.args(["--filter=blob:none", "--sparse"]);
    }
    let output = clone
        .arg(&repository)
        .arg(&checkout)
        .output()
        .map_err(|error| format!("running git: {error}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&prefix);
        return Err(format!("git clone failed: {}", stderr_tail(&output.stderr)));
    }
    if let Some(path) = &path {
        let output = Command::new(&git)
            .arg("-C")
            .arg(&checkout)
            .args(["sparse-checkout", "set", path])
            .output()
            .map_err(|error| format!("running git sparse-checkout: {error}"))?;
        if !output.status.success() {
            let _ = std::fs::remove_dir_all(&prefix);
            return Err(format!(
                "git sparse-checkout failed: {}",
                stderr_tail(&output.stderr)
            ));
        }
    }
    let source = path
        .as_ref()
        .map_or_else(|| checkout.clone(), |path| checkout.join(path));
    if !source.is_dir() {
        let _ = std::fs::remove_dir_all(&prefix);
        return Err(format!(
            "the repository does not contain {}",
            source.display()
        ));
    }
    let package = prefix.join("package");
    copy_directory(&source, &package)?;
    let _ = std::fs::remove_dir_all(&checkout);
    Ok(package)
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ")
}

fn validate_github_repository(raw: &str) -> Result<String, String> {
    let text = raw.split('#').next().unwrap_or(raw).trim_end_matches('/');
    let url =
        reqwest::Url::parse(text).map_err(|error| format!("invalid repository URL: {error}"))?;
    if url.host_str() != Some("github.com") {
        return Err("the repository must be on github.com".to_string());
    }
    let segments: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() < 2 {
        return Err("the repository URL must name an owner and a repository".to_string());
    }
    let owner = segments[0];
    let repository = segments[1].trim_end_matches(".git");
    if owner.is_empty() || repository.is_empty() {
        return Err("the repository URL must name an owner and a repository".to_string());
    }
    Ok(format!("https://github.com/{owner}/{repository}"))
}

fn copy_directory(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("creating {}: {error}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|error| format!("reading {}: {error}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("reading {}: {error}", from.display()))?;
        let source = entry.path();
        if entry.file_name() == ".git" {
            continue;
        }
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_directory(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)
                .map_err(|error| format!("copying {}: {error}", source.display()))?;
        }
    }
    Ok(())
}

/// Parse a validated format-1 index document.
pub(crate) fn parse_remote_index(value: &Value, url: &str) -> Result<RemoteIndex, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "the index must be a JSON object".to_string())?;
    if object.get("format").and_then(Value::as_u64) != Some(1) {
        return Err("`format` must be 1".to_string());
    }
    let title = localized(object.get("title"), "title")?;
    let plugins = object
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| "`plugins` must be an array".to_string())?;
    let mut names = HashSet::new();
    let mut parsed = Vec::with_capacity(plugins.len());
    for (index, value) in plugins.iter().enumerate() {
        let plugin =
            parse_remote_plugin(value).map_err(|error| format!("plugins[{index}]: {error}"))?;
        if !names.insert(plugin.name.clone()) {
            return Err(format!("plugins[{index}].name duplicates {}", plugin.name));
        }
        parsed.push(plugin);
    }
    Ok(RemoteIndex {
        url: url.to_string(),
        title,
        plugins: parsed,
    })
}

fn localized(value: Option<&Value>, field: &str) -> Result<String, String> {
    optional_localized(value, field)?.ok_or_else(|| format!("`{field}` is required"))
}

fn optional_localized(value: Option<&Value>, field: &str) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.clone())),
        Some(Value::Object(languages)) => languages
            .get("en")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(|text| Some(text.to_string()))
            .ok_or_else(|| format!("`{field}` must contain an English string")),
        Some(_) => Err(format!("`{field}` must be a string or a language map")),
    }
}

fn parse_remote_plugin(value: &Value) -> Result<RemotePlugin, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "a plugin must be an object".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| "`name` must be a non-empty string".to_string())?
        .to_string();
    if !valid_package_name(&name) {
        return Err("`name` must be a valid npm package name".to_string());
    }
    let title = optional_localized(object.get("title"), "title")?.unwrap_or_else(|| name.clone());
    let description =
        optional_localized(object.get("description"), "description")?.unwrap_or_default();
    let keywords = object
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("keywords"))
        .and_then(Value::as_array)
        .map(|keywords| {
            keywords
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let versions = object
        .get("versions")
        .and_then(Value::as_array)
        .filter(|versions| !versions.is_empty())
        .ok_or_else(|| "`versions` must be a non-empty array".to_string())?;
    let mut parsed = Vec::with_capacity(versions.len());
    for (index, value) in versions.iter().enumerate() {
        parsed.push(
            parse_remote_version(value).map_err(|error| format!("versions[{index}]: {error}"))?,
        );
    }
    parsed.sort_by(|left, right| compare_versions(&right.version, &left.version));
    Ok(RemotePlugin {
        name,
        title,
        description,
        keywords,
        versions: parsed,
    })
}

fn parse_remote_version(value: &Value) -> Result<RemoteVersion, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "a version must be an object".to_string())?;
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| parse_version(version).is_some())
        .ok_or_else(|| "`version` must be an exact SemVer version".to_string())?
        .to_string();
    let source = parse_remote_source(
        object
            .get("source")
            .ok_or_else(|| "`source` is required".to_string())?,
    )?;
    Ok(RemoteVersion { version, source })
}

fn parse_remote_source(value: &Value) -> Result<RemoteSource, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "`source` must be an object".to_string())?;
    match object.get("type").and_then(Value::as_str) {
        Some("npm") => {
            let registry = object
                .get("registry")
                .and_then(Value::as_str)
                .map(validate_registry_url)
                .transpose()?;
            Ok(RemoteSource::Npm { registry })
        }
        Some("git") => {
            let url = object
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| url.starts_with("https://"))
                .ok_or_else(|| "`url` must be an https URL".to_string())?
                .to_string();
            let commit = object
                .get("commit")
                .and_then(Value::as_str)
                .filter(|commit| {
                    commit.len() == 40
                        && commit
                            .chars()
                            .all(|character| character.is_ascii_hexdigit())
                })
                .ok_or_else(|| "`commit` must be a 40-hex-digit SHA".to_string())?
                .to_string();
            let path = object
                .get("path")
                .and_then(Value::as_str)
                .map(validate_package_path)
                .transpose()?;
            Ok(RemoteSource::Git { url, commit, path })
        }
        _ => Err("`type` must be \"npm\" or \"git\"".to_string()),
    }
}

fn validate_registry_url(raw: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("invalid registry URL: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the registry URL must not contain credentials".to_string());
    }
    match url.scheme() {
        "http" | "https" => Ok(url.to_string()),
        scheme => Err(format!("registry URL uses unsupported scheme '{scheme}'")),
    }
}

fn valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && !name.starts_with('-')
        && !name.chars().any(char::is_whitespace)
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '@' | '/' | '.' | '_' | '~' | '-')
        })
        && !name
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
}

fn validate_package_path(raw: &str) -> Result<String, String> {
    if raw.is_empty()
        || raw
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("`path` must be a relative package directory".to_string());
    }
    Ok(raw.to_string())
}

fn parse_version(raw: &str) -> Option<(u64, u64, u64, Vec<String>)> {
    let (core, prerelease) = raw
        .split_once('-')
        .map_or((raw, None), |(core, rest)| (core, Some(rest)));
    let core = core.split('+').next().unwrap_or(core);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let prerelease = prerelease
        .and_then(|rest| rest.split('+').next())
        .map(|rest| rest.split('.').map(str::to_string).collect())
        .unwrap_or_default();
    Some((major, minor, patch, prerelease))
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    match (parse_version(left), parse_version(right)) {
        (
            Some((left_major, left_minor, left_patch, left_pre)),
            Some((right_major, right_minor, right_patch, right_pre)),
        ) => left_major
            .cmp(&right_major)
            .then_with(|| left_minor.cmp(&right_minor))
            .then_with(|| left_patch.cmp(&right_patch))
            .then_with(|| compare_prerelease(&left_pre, &right_pre)),
        _ => left.cmp(right),
    }
}

fn compare_prerelease(left: &[String], right: &[String]) -> Ordering {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }
    for (left, right) in left.iter().zip(right.iter()) {
        let ordering = match (left.parse::<u64>(), right.parse::<u64>()) {
            (Ok(left), Ok(right)) => left.cmp(&right),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => left.cmp(right),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

/// Install one remote spec into `root` and return the package directory.
///
/// `root` is the BOS-managed extension directory, never a user directory. npm
/// runs with `--ignore-scripts`, so a plugin's lifecycle scripts do not run;
/// pnpm is preferred for a Git spec with a package subdirectory.
pub(crate) fn install_remote(
    root: &Path,
    name: &str,
    spec: &str,
    registry: Option<&str>,
) -> Result<PathBuf, String> {
    let installer = find_installer(spec)?;
    let prefix = root.join(safe_directory_name(name));
    if prefix.exists() {
        std::fs::remove_dir_all(&prefix)
            .map_err(|error| format!("clearing {}: {error}", prefix.display()))?;
    }
    std::fs::create_dir_all(&prefix)
        .map_err(|error| format!("creating {}: {error}", prefix.display()))?;
    let output = installer
        .command(&prefix, spec, registry)
        .output()
        .map_err(|error| format!("running the package installer: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!("package install failed: {tail}"));
    }
    let package = prefix.join("node_modules").join(name);
    if !package.is_dir() {
        return Err(format!(
            "the installer did not create {}",
            package.display()
        ));
    }
    Ok(package)
}

/// Remove the managed directory that holds one installed package.
///
/// A path outside `root` is left alone: a local market install points at the
/// user's own package, and uninstalling must never delete that.
pub(crate) fn remove_managed(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let Some(std::path::Component::Normal(first)) = relative.components().next() else {
        return false;
    };
    std::fs::remove_dir_all(root.join(first)).is_ok()
}

fn safe_directory_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "extension".to_string()
    } else {
        trimmed
    }
}

enum Installer {
    Npm(PathBuf),
    Pnpm(PathBuf),
}

impl Installer {
    fn command(&self, prefix: &Path, spec: &str, registry: Option<&str>) -> Command {
        match self {
            Installer::Npm(program) => {
                let mut command = Command::new(program);
                command
                    .arg("install")
                    .arg("--ignore-scripts")
                    .arg("--no-audit")
                    .arg("--no-fund")
                    .arg("--prefix")
                    .arg(prefix);
                if let Some(registry) = registry {
                    command.arg("--registry").arg(registry);
                }
                command.arg(spec);
                command
            }
            Installer::Pnpm(program) => {
                let mut command = Command::new(program);
                command
                    .arg("--dir")
                    .arg(prefix)
                    .arg("add")
                    .arg("--ignore-scripts");
                if let Some(registry) = registry {
                    command.arg("--registry").arg(registry);
                }
                command.arg(spec);
                command
            }
        }
    }
}

fn find_installer(spec: &str) -> Result<Installer, String> {
    let pnpm = find_program("BOS_PNPM", &["pnpm"]);
    let npm = find_program("BOS_NPM", &["npm"]);
    if spec.contains("&path:") {
        return pnpm
            .map(Installer::Pnpm)
            .ok_or_else(|| "pnpm is required to install a Git package subdirectory".to_string());
    }
    if let Some(npm) = npm {
        return Ok(Installer::Npm(npm));
    }
    pnpm.map(Installer::Pnpm).ok_or_else(|| {
        "neither npm nor pnpm was found; install one to use remote plugins".to_string()
    })
}

fn find_program(env: &str, names: &[&str]) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(env) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut search = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        search.extend(std::env::split_paths(&path));
    }
    if let Some(home) = dirs::home_dir() {
        search.extend([
            home.join(".local/share/mise/shims"),
            home.join("Library/pnpm"),
            home.join(".local/share/pnpm"),
        ]);
        // A Finder-launched app does not inherit the shell's mise/nvm PATH, so
        // look inside the version managers' bin directories as well.
        for versions in [
            home.join(".local/share/mise/installs/node"),
            home.join(".nvm/versions/node"),
        ] {
            for version in directories(&versions) {
                search.push(version.join("bin"));
            }
        }
    }
    search.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    for directory in search {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn codex_candidates(home: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut candidates = Vec::new();
    let cache = home.join(".codex").join("plugins").join("cache");
    for market in directories(&cache) {
        for plugin in directories(&market) {
            if plugin.join(".codex-plugin").join("plugin.json").is_file() {
                candidates.push((plugin, "Codex cache"));
                continue;
            }
            for revision in directories(&plugin) {
                if revision.join(".codex-plugin").join("plugin.json").is_file() {
                    candidates.push((revision, "Codex cache"));
                }
            }
        }
    }
    let bundled = home
        .join(".codex")
        .join(".tmp")
        .join("bundled-marketplaces");
    for market in directories(&bundled) {
        for plugin in directories(&market.join("plugins")) {
            if plugin.join(".codex-plugin").join("plugin.json").is_file() {
                candidates.push((plugin, "Codex marketplace"));
            }
        }
    }
    let curated = home
        .join(".codex")
        .join(".tmp")
        .join("plugins")
        .join("plugins");
    for plugin in directories(&curated) {
        if plugin.join(".codex-plugin").join("plugin.json").is_file() {
            candidates.push((plugin, "Codex marketplace"));
        }
    }
    candidates
}

fn dsh_candidates(home: &Path, errors: &mut Vec<String>) -> Vec<(PathBuf, String)> {
    let profiles = home.join(".dsh").join("profiles");
    let mut candidates = Vec::new();
    for profile in directories(&profiles) {
        let manifest = profile.join("package.json");
        let text = match std::fs::read_to_string(&manifest) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{}: {error}", manifest.display()));
                continue;
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                errors.push(format!("{}: {error}", manifest.display()));
                continue;
            }
        };
        let profile_name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("profile")
            .to_string();
        let mut names = std::collections::BTreeSet::new();
        if let Some(dependencies) = value.get("dependencies").and_then(Value::as_object) {
            names.extend(dependencies.keys().cloned());
        }
        if let Some(bundles) = value
            .get("dsh")
            .and_then(|dsh| dsh.get("profile"))
            .and_then(|profile| profile.get("bundles"))
            .and_then(Value::as_array)
        {
            for bundle in bundles {
                if let Some(name) = bundle.as_str() {
                    names.insert(name.to_string());
                }
            }
        }
        for name in names {
            if name.starts_with("@deepseek-ai/") {
                continue;
            }
            let directory = profile.join("node_modules").join(&name);
            if has_dsh_manifest(&directory) {
                candidates.push((directory, format!("DSH profile {profile_name}")));
            }
        }
    }
    candidates
}

fn has_dsh_manifest(directory: &Path) -> bool {
    let text = match std::fs::read_to_string(directory.join("package.json")) {
        Ok(text) => text,
        Err(_) => return false,
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return false,
    };
    value.get("dsh").map(Value::is_object).unwrap_or(false)
}

fn directories(root: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            }
        }
    }
    directories.sort();
    directories
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SCRATCH: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "bsh-market-{}-{}",
            std::process::id(),
            SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a scratch home");
        directory
    }

    fn copy_directory(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("the target directory");
        for entry in std::fs::read_dir(from).expect("the source directory") {
            let entry = entry.expect("a source entry");
            let source = entry.path();
            let target = to.join(entry.file_name());
            if source.is_dir() {
                copy_directory(&source, &target);
            } else {
                std::fs::copy(&source, &target).expect("the copied file");
            }
        }
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the parent directory");
        }
        std::fs::write(path, text).expect("the written file");
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/extensions")
            .join(name)
    }

    #[test]
    fn the_local_market_discovers_both_standards_and_skips_internal_packages() {
        let home = scratch();
        // Codex: one cache copy and a duplicate marketplace copy. The cache row
        // is the stable install, so discovery keeps that one.
        copy_directory(
            &fixture("codex-deploy"),
            &home.join(".codex/plugins/cache/openai/codex-deploy/0722921d"),
        );
        copy_directory(
            &fixture("codex-deploy"),
            &home.join(".codex/.tmp/bundled-marketplaces/openai/plugins/codex-deploy"),
        );

        // DSH: the profile names a third-party plugin and the runtime's own base
        // package; only the third-party package is a market row.
        write(
            &home.join(".dsh/profiles/desktop/package.json"),
            r#"{
                "name": "desktop",
                "dependencies": {
                    "@local/dsh-echo": "1.0.0",
                    "@deepseek-ai/dsh-base": "0.2.0"
                },
                "dsh": { "profile": { "bundles": ["@local/dsh-echo", "@deepseek-ai/dsh-base"] } }
            }"#,
        );
        copy_directory(
            &fixture("dsh-echo"),
            &home.join(".dsh/profiles/desktop/node_modules/@local/dsh-echo"),
        );
        write(
            &home.join(".dsh/profiles/desktop/node_modules/@deepseek-ai/dsh-base/package.json"),
            r#"{ "name": "@deepseek-ai/dsh-base", "version": "0.2.0", "dsh": { "bundle": { "patch": "./cordis.patch.yml" } } }"#,
        );

        let scan = discover_in(&home);
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        assert_eq!(
            scan.plugins.len(),
            2,
            "one Codex row and one DSH row: {:?}",
            scan.plugins
        );
        let codex = scan
            .plugins
            .iter()
            .find(|plugin| plugin.standard == ExtensionStandard::Codex)
            .expect("the Codex row");
        assert_eq!(codex.id, "codex-deploy");
        assert_eq!(codex.name, "Codex Deploy Fixture");
        let source = match &codex.install {
            MarketInstall::Local(path) => path,
            other => panic!("a local fixture must install from its path, got {other:?}"),
        };
        assert!(
            source.to_string_lossy().contains("plugins/cache"),
            "the cache copy wins the duplicate: {}",
            source.display()
        );
        let dsh = scan
            .plugins
            .iter()
            .find(|plugin| plugin.standard == ExtensionStandard::DeepSeekHarness)
            .expect("the DSH row");
        assert_eq!(dsh.id, "@local/dsh-echo");
        assert_eq!(dsh.name, "Fixture DSH Echo");
        assert!(
            scan.plugins
                .iter()
                .all(|plugin| plugin.id != "@deepseek-ai/dsh-base"),
            "the runtime's own packages are not market rows"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    const REMOTE_INDEX: &str = r#"{
        "format": 1,
        "title": { "en": "Test Market", "zh": "测试市场" },
        "description": "A fixture index.",
        "plugins": [
            {
                "name": "dsh-alpha",
                "title": { "en": "Alpha" },
                "description": { "en": "An alpha plugin." },
                "metadata": { "keywords": ["searchable", "alpha"] },
                "versions": [
                    { "version": "1.0.0", "publishTimestamp": 1, "source": { "type": "npm" } },
                    { "version": "1.1.0", "publishTimestamp": 2, "source": { "type": "npm", "registry": "https://registry.example/" } },
                    { "version": "2.0.0-beta.1", "publishTimestamp": 3, "source": { "type": "npm" } }
                ]
            },
            {
                "name": "dsh-git",
                "versions": [
                    {
                        "version": "3.0.0",
                        "publishTimestamp": 4,
                        "source": {
                            "type": "git",
                            "url": "https://github.com/example/repo",
                            "commit": "0123456789abcdef0123456789abcdef01234567",
                            "path": "packages/plugin"
                        }
                    }
                ]
            }
        ]
    }"#;

    fn remote_index() -> RemoteIndex {
        let value: Value = serde_json::from_str(REMOTE_INDEX).expect("the fixture JSON");
        parse_remote_index(&value, "https://market.example/plugins.json")
            .expect("the fixture index parses")
    }

    #[test]
    fn a_remote_index_builds_market_rows_with_exact_install_targets() {
        let index = remote_index();
        assert_eq!(index.title, "Test Market");
        let rows = remote_market_plugins(&index);
        assert_eq!(rows.len(), 2);
        let alpha = rows
            .iter()
            .find(|plugin| plugin.id == "dsh-alpha")
            .expect("the alpha row");
        assert_eq!(
            alpha.version, "1.1.0",
            "the highest stable version is offered"
        );
        assert_eq!(alpha.name, "Alpha");
        assert_eq!(alpha.source_label, "Remote · Test Market");
        assert!(alpha.is_remote());
        assert_eq!(
            alpha.install,
            MarketInstall::Remote {
                spec: "dsh-alpha@1.1.0".to_string(),
                registry: Some("https://registry.example/".to_string()),
            }
        );
        assert!(alpha.keywords.contains(&"searchable".to_string()));

        let git = rows
            .iter()
            .find(|plugin| plugin.id == "dsh-git")
            .expect("the git row");
        assert_eq!(
            git.install,
            MarketInstall::Remote {
                spec: "git+https://github.com/example/repo.git#0123456789abcdef0123456789abcdef01234567&path:/packages/plugin".to_string(),
                registry: None,
            }
        );
    }

    #[test]
    fn a_remote_index_refuses_a_wrong_format_and_duplicate_names() {
        let wrong: Value =
            serde_json::from_str(r#"{ "format": 2, "title": "Bad", "plugins": [] }"#)
                .expect("JSON");
        let error = parse_remote_index(&wrong, "https://market.example/plugins.json")
            .expect_err("format 2 is refused");
        assert!(error.contains("format"), "{error}");

        let duplicate: Value = serde_json::from_str(
            r#"{
                "format": 1,
                "title": "Bad",
                "plugins": [
                    { "name": "same", "versions": [{ "version": "1.0.0", "source": { "type": "npm" } }] },
                    { "name": "same", "versions": [{ "version": "1.0.0", "source": { "type": "npm" } }] }
                ]
            }"#,
        )
        .expect("JSON");
        let error = parse_remote_index(&duplicate, "https://market.example/plugins.json")
            .expect_err("duplicate names are refused");
        assert!(error.contains("duplicates same"), "{error}");
    }

    #[test]
    fn remote_index_urls_are_validated() {
        assert_eq!(
            validate_index_url("https://market.example/plugins.json").expect("valid"),
            "https://market.example/plugins.json"
        );
        assert!(validate_index_url("ftp://market.example/plugins.json").is_err());
        assert!(validate_index_url("https://user:secret@market.example/plugins.json").is_err());
        assert!(validate_index_url("https://market.example/plugins.json?x=1").is_err());
        assert!(validate_index_url("").is_err());
    }

    #[test]
    fn a_remote_index_is_fetched_over_http() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let body = REMOTE_INDEX.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        let address = listener.local_addr().expect("the bound address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("one request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("the response body");
        });
        let url = format!("http://{address}/plugins.json");
        let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
        let index = runtime
            .block_on(fetch_index(&url))
            .expect("the local index fetches");
        assert_eq!(index.title, "Test Market");
        assert_eq!(index.plugins.len(), 2);
        server.join().expect("the server thread");
    }

    const DSH_CATALOG: &str = r#"{
        "name": "awesome-dsh-plugin",
        "count": 2,
        "plugins": [
            {
                "name": "dsh-alpha",
                "npm": "@scope/dsh-alpha",
                "version": "1.2.3",
                "description": { "en": "An alpha plugin.", "zh": "阿尔法插件" },
                "category": "ui",
                "capabilities": ["fs-read"]
            },
            {
                "name": "dsh-github-only",
                "npm": null,
                "description": { "en": "A GitHub-only entry." }
            }
        ]
    }"#;

    const CODEX_CATALOG: &str = r#"{
        "plugins": [
            {
                "type": "plugin",
                "slug": "linear",
                "displayName": "Linear",
                "version": "0.1.0",
                "description": "Find and reference issues.",
                "repository": "https://github.com/openai/plugins",
                "pluginPath": "plugins/linear",
                "keywords": ["issues", "project"]
            },
            {
                "type": "skill",
                "slug": "not-a-plugin",
                "repository": "https://github.com/openai/plugins",
                "pluginPath": "skills/not-a-plugin"
            }
        ]
    }"#;

    #[test]
    fn the_builtin_dsh_catalog_becomes_installable_rows() {
        let provider = &BUILTIN_PROVIDERS[0];
        assert_eq!(provider.display_url, "https://dshmarket.com/browse/");
        let value: Value = serde_json::from_str(DSH_CATALOG).expect("the catalog JSON");
        let rows = parse_dsh_catalog(&value, provider).expect("the catalog parses");
        assert_eq!(rows.len(), 1, "the GitHub-only entry is not installable");
        let row = &rows[0];
        assert_eq!(row.id, "@scope/dsh-alpha");
        assert_eq!(row.name, "dsh-alpha");
        assert_eq!(row.version, "1.2.3");
        assert_eq!(row.standard, ExtensionStandard::DeepSeekHarness);
        assert_eq!(
            row.install,
            MarketInstall::Remote {
                spec: "@scope/dsh-alpha@1.2.3".to_string(),
                registry: None,
            }
        );
        assert!(row.keywords.contains(&"fs-read".to_string()));
        assert!(row.keywords.contains(&"ui".to_string()));
        assert_eq!(row.source_label, "dshmarket");
    }

    #[test]
    fn the_builtin_codex_catalog_becomes_github_rows() {
        let provider = &BUILTIN_PROVIDERS[1];
        assert_eq!(provider.display_url, "https://www.codex-marketplace.com/");
        let value: Value = serde_json::from_str(CODEX_CATALOG).expect("the catalog JSON");
        let rows = parse_codex_catalog(&value, provider).expect("the catalog parses");
        assert_eq!(rows.len(), 1, "skills are not plugin rows");
        let row = &rows[0];
        assert_eq!(row.id, "linear");
        assert_eq!(row.name, "Linear");
        assert_eq!(row.version, "0.1.0");
        assert_eq!(row.standard, ExtensionStandard::Codex);
        assert_eq!(
            row.install,
            MarketInstall::GitHub {
                repository: "https://github.com/openai/plugins".to_string(),
                path: "plugins/linear".to_string(),
            }
        );
        assert_eq!(row.source_label, "Codex Marketplace");
    }

    #[test]
    fn github_repository_urls_are_normalized_and_refused() {
        assert_eq!(
            validate_github_repository("https://github.com/openai/plugins").expect("valid"),
            "https://github.com/openai/plugins"
        );
        assert_eq!(
            validate_github_repository("https://github.com/openai/plugins.git#readme")
                .expect("normalized"),
            "https://github.com/openai/plugins"
        );
        assert!(validate_github_repository("https://evil.example/openai/plugins").is_err());
        assert!(validate_github_repository("https://github.com/openai").is_err());
    }

    #[test]
    fn a_remote_spec_installs_into_a_managed_directory_and_is_removable() {
        if find_installer("remote-fixture@1.0.0").is_err() {
            eprintln!("skipping: no npm or pnpm on this machine");
            return;
        }
        let scratch = std::env::temp_dir().join(format!(
            "bsh-market-install-{}-{}",
            std::process::id(),
            SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        let source = scratch.join("fixture");
        std::fs::create_dir_all(&source).expect("the fixture package");
        std::fs::write(
            source.join("package.json"),
            r#"{ "name": "remote-fixture", "version": "1.0.0", "dsh": { "bundle": { "patch": "./cordis.patch.yml" } } }"#,
        )
        .expect("the fixture manifest");
        std::fs::write(source.join("cordis.patch.yml"), "[]\n").expect("the fixture patch");

        let root = scratch.join("managed");
        let spec = format!("file:{}", source.display());
        let installed = install_remote(&root, "remote-fixture", &spec, None)
            .expect("the local file spec installs");
        assert!(
            installed.join("package.json").is_file(),
            "the installed package is at {}",
            installed.display()
        );
        assert!(
            installed.starts_with(&root),
            "it lives under the managed root"
        );
        let package = ExtensionRegistry::load(&installed).expect("the installed package loads");
        assert_eq!(package.extension.id, "remote-fixture");

        assert!(remove_managed(&root, &installed), "uninstall clears it");
        assert!(!installed.exists(), "the package directory is gone");
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
