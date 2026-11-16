//! The installed-extension view: the inventory record joined with the package
//! metadata read from its own directory.
//!
//! The settings record is a cache of what the market row knew at install time;
//! the package directory is the truth. This module resolves the two into one
//! `InstalledPlugin` for the list, and keeps the parse behind a
//! `(path, mtime)` cache so a render pass never re-reads a manifest it has
//! already read. The cache is cleared on a settings save and on every
//! install/uninstall, which is exactly when a record or its directory can
//! change.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use agent::extensions::{ExtensionStandard, RefusedFeature};

use crate::settings::global::{InstalledExtension, InstalledStandard};

/// How a package standard maps onto the stored inventory standard.
impl From<ExtensionStandard> for InstalledStandard {
    fn from(standard: ExtensionStandard) -> Self {
        match standard {
            ExtensionStandard::Codex => InstalledStandard::Codex,
            ExtensionStandard::DeepSeekHarness => InstalledStandard::Dsh,
        }
    }
}

/// One part the shim refused inside an installed package.
///
/// The reason is the loader's own sentence and never carries configuration
/// values, environment maps or package file contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledRefusal {
    /// The refused feature, in the shim's own words ("external app connector").
    pub(crate) feature: String,
    /// Why the feature did not load.
    pub(crate) reason: String,
}

/// One installed plugin as the list draws it: the inventory record joined with
/// the metadata read from the package directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledPlugin {
    /// The settings id, and the key the workspace disabled list uses.
    pub(crate) id: String,
    /// The package name from the manifest, else the record's cached name.
    pub(crate) name: String,
    /// The display name from the manifest, else the record's cached title.
    pub(crate) display_name: String,
    /// The manifest description, else the record's cached description.
    pub(crate) description: String,
    /// The manifest version, else the record's cached version.
    pub(crate) version: String,
    /// The standard the package was read as, else the stored standard.
    pub(crate) standard: InstalledStandard,
    /// The market source the row came from.
    pub(crate) source_label: String,
    /// The npm spec or `repository#path` the installer was given.
    pub(crate) install_target: String,
    /// The package directory, as stored.
    pub(crate) path: Option<String>,
    /// Whether the directory is BOS's managed copy.
    pub(crate) managed: bool,
    /// Unix seconds the record was written.
    pub(crate) installed_at: u64,
    /// Whether this workspace leaves the package enabled.
    pub(crate) enabled: bool,
    /// Whether the reader approved the package's declared commands at the
    /// directory it is installed at.
    pub(crate) trusted: bool,
    /// The MCP servers the package declares, in the only shape the trust UI may
    /// draw: exact command lines, working directories and env KEYS. Empty for a
    /// package that declares none.
    pub(crate) mcp_launches: Vec<crate::mcp::McpLaunch>,
    /// The capabilities the package DECLARED, whether or not they loaded.
    ///
    /// A package whose script was refused still declares what it would have
    /// done, and the refusal line next to it says why it did not happen. This
    /// list is the package's own claim.
    pub(crate) capabilities: Vec<String>,
    /// The capabilities that actually LOADED, which is what the row's chip
    /// line draws.
    ///
    /// The two lists differ whenever a package's payload was refused, and
    /// drawing the declaration alone would tell the reader a package can
    /// select a model it cannot.
    pub(crate) loaded_capabilities: Vec<String>,
    /// The parts the shim refused.
    pub(crate) refusals: Vec<InstalledRefusal>,
    /// Why the package could not be read, when it could not.
    pub(crate) load_error: Option<String>,
}

impl InstalledPlugin {
    /// The row title: the display name when the manifest has one, else the id.
    pub(crate) fn title(&self) -> &str {
        if self.display_name.trim().is_empty() {
            &self.id
        } else {
            &self.display_name
        }
    }

    /// The first visible character of the title, for the tinted square.
    pub(crate) fn initial(&self) -> String {
        self.title()
            .chars()
            .find(|character| character.is_alphanumeric())
            .map(|character| character.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    }

    /// Whether the row carries the support pill.
    ///
    /// A refused part is `Partial`; a package that cannot be read at all is
    /// `Problem`. Both are states the reader can act on by opening the row.
    /// MCP servers are filtered out at resolve time, because this crate starts
    /// them itself and the shim's refusal of them is not a user-visible gap -
    /// an MCP package that is not trusted gets its own Needs trust state.
    pub(crate) fn problem(&self) -> bool {
        self.load_error.is_some() || self.refusal_count() > 0
    }

    /// The pill's locale key: `Problem` only when the package cannot be read.
    ///
    /// A readable package that declares one unsupported part is `Partial`: the
    /// rest of it loads, and a Canva-style app connector does not make its MCP
    /// server a broken plugin.
    pub(crate) fn problem_label_key(&self) -> &'static str {
        if self.load_error.is_some() {
            "settings.pluginsProblem"
        } else {
            "settings.pluginsPartial"
        }
    }

    /// Whether the package declares MCP servers the reader has not approved.
    ///
    /// This is the untrusted-but-MCP-bearing state the gate deliberately does
    /// not hide: the row shows a Needs trust pill and the disclosure names the
    /// commands, but nothing runs until the reader approves them.
    pub(crate) fn needs_trust(&self) -> bool {
        !self.trusted && !self.mcp_launches.is_empty()
    }

    /// How many parts the shim refused.
    pub(crate) fn refusal_count(&self) -> usize {
        self.refusals.len()
    }

    /// The refused kinds, comma separated, for the pill's tooltip.
    pub(crate) fn refused_kinds(&self) -> String {
        self.refusals
            .iter()
            .map(|refusal| refusal.feature.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The capabilities as one line for the disclosure.
    pub(crate) fn capability_line(&self) -> String {
        self.loaded_capabilities.join(", ")
    }
}

/// The metadata one package directory declares.
#[derive(Debug, Clone, Default)]
struct PackageMeta {
    name: String,
    display_name: String,
    description: String,
    version: String,
    standard: Option<InstalledStandard>,
    capabilities: Vec<String>,
    loaded_capabilities: Vec<String>,
    refusals: Vec<InstalledRefusal>,
    /// Declared MCP servers, parsed for the trust disclosure. Values are absent
    /// by construction - [`crate::mcp::McpLaunch`] keeps env KEYS only.
    mcp_launches: Vec<crate::mcp::McpLaunch>,
}

impl PackageMeta {
    /// Read one package through the shared extension loader.
    ///
    /// The MCP refusal is dropped here for the same reason the market drops it:
    /// `mcp::extension_servers` starts the declared servers itself, so calling
    /// it a problem would be a false alarm.
    ///
    /// `trust` is the reader's own decision, read from the inventory record. It
    /// is the ONLY thing that decides whether a DSH package's JavaScript host
    /// entry runs: an untrusted package is parsed declaratively (and its entry
    /// refused by name), a trusted one is executed in the bridge. The path is
    /// part of the decision upstream - a record whose directory moved is
    /// untrusted again - so this function takes the resolved verdict rather than
    /// re-deriving it from a flag.
    fn read(root: &Path, trust: agent::extensions::Trust) -> (Option<Self>, Option<String>) {
        match agent::extensions::ExtensionRegistry::load_with_trust(root, trust) {
            Ok(package) => {
                let extension = package.extension;
                let refusals = package
                    .refusals
                    .iter()
                    .filter(|refusal| refusal.feature != RefusedFeature::McpServer)
                    .map(|refusal| InstalledRefusal {
                        feature: refusal.feature.to_string(),
                        reason: refusal.reason.clone(),
                    })
                    .collect();
                // The same declaration the MCP gate reads, but trusted or not:
                // this is what the row must show before anything may start.
                let mcp_launches: Vec<crate::mcp::McpLaunch> =
                    crate::mcp::declared_servers(root, &extension.id)
                        .iter()
                        .map(crate::mcp::ExtensionMcp::launch)
                        .collect();
                // Read before the struct literal takes the fields by move.
                // What this shell can put in front of the reader: the
                // contributions that loaded, minus the kinds bsh has no
                // surface for.
                let loaded_capabilities: Vec<String> =
                    crate::caps::surfaced_capabilities(&extension.loaded_capabilities())
                        .into_iter()
                        .map(|capability| capability.as_str().to_string())
                        .collect();
                (
                    Some(Self {
                        name: extension.id,
                        display_name: extension.name,
                        description: extension.description.unwrap_or_default(),
                        version: extension.version,
                        standard: Some(InstalledStandard::from(extension.standard)),
                        capabilities: extension
                            .capabilities
                            .iter()
                            .map(|capability| capability.as_str().to_string())
                            .collect(),
                        // What the host actually loaded, which is the claim the
                        // row's chip line makes. The declared list above stays
                        // for the refusal disclosure beside it.
                        loaded_capabilities,
                        refusals,
                        mcp_launches,
                    }),
                    None,
                )
            }
            Err(error) => (None, Some(error.to_string())),
        }
    }
}

/// One cached parse, keyed by the package directory and stamped with the newest
/// manifest/locale mtime under it.
#[derive(Debug, Clone)]
struct CachedPackage {
    /// The file stamp AND whether the parse executed a trusted entry. Both
    /// belong in the key: a trust change must invalidate the cache, because the
    /// parse is a different parse once the entry runs.
    stamp: Option<(SystemTime, bool)>,
    meta: Option<PackageMeta>,
    error: Option<String>,
}

/// The list's resolved view, with its `(path, mtime)` cache.
///
/// A render pass resolves every record it draws; the first pass for a package
/// reads its manifest, and later passes only compare mtimes. The cache is small
/// (one entry per installed package) and is cleared wholesale on save and on
/// install/uninstall, so there is no per-record invalidation to forget.
#[derive(Debug, Default)]
pub(crate) struct InstalledCache {
    packages: HashMap<PathBuf, CachedPackage>,
}

impl InstalledCache {
    /// Forget every parsed package. Called on a settings save and on
    /// install/uninstall, the two events that can change a directory.
    pub(crate) fn clear(&mut self) {
        self.packages.clear();
    }

    /// Join one inventory record with its package metadata.
    pub(crate) fn resolve(
        &mut self,
        record: &InstalledExtension,
        enabled: bool,
    ) -> InstalledPlugin {
        let root = record
            .path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(crate::caps::expand_path);
        let trust = if record.is_trusted() {
            agent::extensions::Trust::Trusted
        } else {
            agent::extensions::Trust::Untrusted
        };
        let (meta, load_error) = match &root {
            Some(path) => self.read_cached(path, trust),
            None => (None, None),
        };
        let standard = meta
            .as_ref()
            .and_then(|meta| meta.standard)
            .unwrap_or(record.standard);
        InstalledPlugin {
            id: record.id.clone(),
            name: meta
                .as_ref()
                .map(|meta| meta.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| record.name.clone()),
            display_name: meta
                .as_ref()
                .map(|meta| meta.display_name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| record.display_name.clone()),
            description: meta
                .as_ref()
                .map(|meta| meta.description.clone())
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| record.description.clone()),
            version: meta
                .as_ref()
                .map(|meta| meta.version.clone())
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| record.version.clone()),
            standard,
            source_label: record.source_label.clone(),
            install_target: record.install_target.clone(),
            path: record.path.clone(),
            managed: record.managed,
            installed_at: record.installed_at,
            enabled,
            trusted: record.is_trusted(),
            mcp_launches: meta
                .as_ref()
                .map(|meta| meta.mcp_launches.clone())
                .unwrap_or_default(),
            capabilities: meta
                .as_ref()
                .map(|meta| meta.capabilities.clone())
                .unwrap_or_default(),
            loaded_capabilities: meta
                .as_ref()
                .map(|meta| meta.loaded_capabilities.clone())
                .unwrap_or_default(),
            refusals: meta
                .as_ref()
                .map(|meta| meta.refusals.clone())
                .unwrap_or_default(),
            load_error,
        }
    }

    /// The cached parse for one directory, re-read only when its stamp moved.
    fn read_cached(
        &mut self,
        root: &Path,
        trust: agent::extensions::Trust,
    ) -> (Option<PackageMeta>, Option<String>) {
        // The cache key carries trust as well as the file stamp: a package that
        // was JUST trusted must be re-read, because its parse now includes an
        // execution. Without this the row would keep showing the untrusted parse
        // - and the tools it refused - until some unrelated file changed.
        let stamp = package_stamp(root).map(|stamp| (stamp, trust.permits_execution()));
        let stale = match self.packages.get(root) {
            Some(cached) => cached.stamp != stamp,
            None => true,
        };
        if stale {
            let (meta, error) = PackageMeta::read(root, trust);
            self.packages
                .insert(root.to_path_buf(), CachedPackage { stamp, meta, error });
        }
        let cached = self
            .packages
            .get(root)
            .expect("the entry was inserted above");
        (cached.meta.clone(), cached.error.clone())
    }
}

/// The newest mtime of the files a package's metadata comes from.
///
/// A directory's own mtime can stay still while its manifest is rewritten, so
/// the manifest (and the DSH locale file, which carries the display title and
/// description) are stamped individually. All absent means the directory is
/// gone, which is cached the same way and retried only when it reappears.
fn package_stamp(root: &Path) -> Option<SystemTime> {
    [
        "",
        "package.json",
        ".codex-plugin/plugin.json",
        ".mcp.json",
        "locale/en.json",
    ]
    .iter()
    .filter_map(|relative| fs::metadata(root.join(relative)).ok())
    .filter_map(|metadata| metadata.modified().ok())
    .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/extensions")
            .join(name)
    }

    fn record(id: &str, path: &Path) -> InstalledExtension {
        InstalledExtension {
            id: id.to_string(),
            name: "stale-name".to_string(),
            display_name: "Stale Title".to_string(),
            description: "stale description".to_string(),
            version: "0.0.1".to_string(),
            standard: InstalledStandard::Local,
            source_label: "dshmarket".to_string(),
            install_target: "npm install".to_string(),
            path: Some(path.to_string_lossy().to_string()),
            managed: true,
            installed_at: 7,
            trusted: false,
            trusted_path: None,
        }
    }

    #[test]
    fn the_resolved_view_reads_the_manifest_over_stale_record_metadata() {
        let mut cache = InstalledCache::default();
        let plugin = cache.resolve(&record("echo", &fixture("dsh-echo")), true);
        assert_eq!(
            plugin.name, "@local/dsh-echo",
            "the manifest package name wins"
        );
        assert_eq!(plugin.display_name, "Fixture DSH Echo");
        assert_eq!(
            plugin.description,
            "A fixture DeepSeek Harness package for the extension substrate."
        );
        assert_eq!(plugin.version, "1.2.3");
        assert_eq!(plugin.standard, InstalledStandard::Dsh);
        assert!(plugin.enabled);
        assert!(plugin.capabilities.contains(&"tools".to_string()));
        assert_eq!(plugin.title(), "Fixture DSH Echo");
        assert_eq!(plugin.initial(), "F");
    }

    #[test]
    fn a_readable_package_with_a_refusal_is_partial_and_names_the_kind() {
        let mut cache = InstalledCache::default();
        let plugin = cache.resolve(&record("deploy", &fixture("codex-deploy")), true);
        assert!(plugin.problem(), "a refused part is a state the row shows");
        assert_eq!(
            plugin.problem_label_key(),
            "settings.pluginsPartial",
            "a readable package with one refused part is Partial, not Problem"
        );
        assert!(plugin.refusal_count() > 0);
        assert!(
            plugin
                .refusals
                .iter()
                .any(|refusal| refusal.feature == "external app connector"),
            "the codex app connector is the refused kind: {:?}",
            plugin.refusals
        );
        assert!(
            !plugin
                .refusals
                .iter()
                .any(|refusal| refusal.feature == "MCP server"),
            "MCP is started by this crate, so it is not a user-visible problem"
        );
        assert!(
            plugin.refused_kinds().contains("external app connector"),
            "the pill's tooltip names the kind"
        );
        assert_eq!(plugin.standard, InstalledStandard::Codex);
    }

    #[test]
    fn a_missing_directory_is_a_load_error_not_a_silent_row() {
        let mut cache = InstalledCache::default();
        let plugin = cache.resolve(&record("gone", Path::new("/tmp/bsh-no-such-package")), true);
        assert!(plugin.problem());
        assert_eq!(
            plugin.problem_label_key(),
            "settings.pluginsProblem",
            "a package that cannot be read is a real Problem"
        );
        assert!(plugin.load_error.is_some());
        assert_eq!(plugin.version, "0.0.1", "the record's cache still shows");
        assert_eq!(plugin.refusals, Vec::new());
    }

    #[test]
    fn a_bare_record_without_a_path_is_not_a_problem() {
        let mut cache = InstalledCache::default();
        let bare = InstalledExtension {
            id: "builtin-bundle".to_string(),
            display_name: "Built-in bundle".to_string(),
            ..InstalledExtension::default()
        };
        let plugin = cache.resolve(&bare, false);
        assert_eq!(plugin.title(), "Built-in bundle");
        assert!(
            !plugin.problem(),
            "nothing was attempted, so nothing failed"
        );
        assert!(!plugin.enabled);
    }

    /// An MCP-bearing package that nobody approved is a state the pane must
    /// show, and the disclosure it can draw never carries an env VALUE.
    #[test]
    fn an_untrusted_mcp_package_shows_what_it_would_run_without_env_values() {
        let mut cache = InstalledCache::default();
        let plugin = cache.resolve(&record("codex-deploy", &fixture("codex-deploy")), true);
        assert!(plugin.enabled, "the premise: installed and enabled");
        assert!(!plugin.trusted, "a record with no trust field is untrusted");
        assert!(
            plugin.needs_trust(),
            "an untrusted package with MCP servers is the Needs trust state"
        );
        assert_eq!(plugin.mcp_launches.len(), 1, "one declared server");
        let launch = &plugin.mcp_launches[0];
        assert_eq!(launch.server, "fixture-server");
        assert!(
            launch.command.contains("node") && launch.command.contains("./server.js"),
            "the exact command line is shown: {}",
            launch.command
        );
        assert_eq!(launch.env_keys, ["FIXTURE_TOKEN"], "env KEYS only");
        // The disclosure draws exactly these three fields; joining them is the
        // text the reader sees, so a value can only appear here by appearing
        // in one of them.
        let shown = plugin
            .mcp_launches
            .iter()
            .map(|launch| {
                format!(
                    "{} {} {}",
                    launch.command,
                    launch.cwd.clone().unwrap_or_default(),
                    launch.env_keys.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            shown.contains("FIXTURE_TOKEN"),
            "the disclosure names the environment key: {shown}"
        );
        assert!(
            !shown.contains("sk-fixture-secret"),
            "the fixture's env VALUE never reaches the disclosure: {shown}"
        );
    }

    #[test]
    fn the_cache_reuses_a_parse_until_the_manifest_moves() {
        let mut cache = InstalledCache::default();
        let first = cache.resolve(&record("echo", &fixture("dsh-echo")), true);
        let again = cache.resolve(&record("echo", &fixture("dsh-echo")), true);
        assert_eq!(first, again);
        assert_eq!(cache.packages.len(), 1, "one directory, one parse");
        cache.clear();
        assert!(cache.packages.is_empty(), "save/install clears the view");
    }

    /// The trust verdict is what decides whether a DSH package's host entry is
    /// EXECUTED, and the parse cache must not serve an untrusted parse to a
    /// package that has just been trusted.
    ///
    /// The fixture writes a marker file the moment it is imported, so the marker
    /// is direct evidence of execution rather than of a successful `apply`. Two
    /// facts are asserted: an untrusted record leaves no marker, and trusting the
    /// SAME record - through the same cache - runs it. Without the trust half of
    /// the cache key the second resolve would return the cached untrusted parse
    /// and the marker would still be absent.
    #[test]
    fn trusting_a_package_runs_its_entry_and_the_cache_follows() {
        if agent::extensions::bridge::find_interpreter().is_none() {
            eprintln!("skipped: no Node interpreter");
            return;
        }
        let source = fixture("dsh-untrusted");
        let root = std::env::temp_dir().join(format!("bsh-trust-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch");
        for entry in fs::read_dir(&source).expect("fixture") {
            let entry = entry.expect("entry");
            fs::copy(entry.path(), root.join(entry.file_name())).expect("copy");
        }
        let marker = root.join("executed.marker");
        assert!(!marker.exists(), "the scratch copy starts clean");

        let mut cache = InstalledCache::default();
        let untrusted = record("bridge-trust", &root);
        let view = cache.resolve(&untrusted, true);
        assert!(!view.trusted, "a bare record is untrusted");
        assert!(!marker.exists(), "an untrusted package's entry never ran");
        assert!(
            !view
                .loaded_capabilities
                .iter()
                .any(|capability| capability == "tools"),
            "nothing the refused entry would register is reported: {:?}",
            view.loaded_capabilities
        );

        let mut trusted = record("bridge-trust", &root);
        trusted.trusted = true;
        trusted.trusted_path = trusted.path.clone();
        let view = cache.resolve(&trusted, true);
        assert!(view.trusted, "the trusted record resolves as trusted");
        assert!(
            marker.exists(),
            "trusting the package ran its entry, through the same cache"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
