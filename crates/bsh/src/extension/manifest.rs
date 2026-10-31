//! The manifest: the one file every package must ship, parsed strictly and
//! validated semantically before any code runs.
//!
//! The shape is the public extension standard. Unknown fields are refused
//! rather than ignored, because a misspelled mandatory field that silently
//! disappears is how a package ends up asking for nothing and receiving
//! nothing. Optional additions use serde defaults, which is the defined
//! forward-compatibility rule.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};

use super::capability::Capability;
use super::types::{
    extension_api_version, host_version, ContributionId, ContributionKind, ExtensionError,
    ExtensionId, Requirement, RuntimeKind, StartupPolicy, TrustLevel, ValidationReport,
    MANIFEST_SCHEMA_VERSION,
};

/// The manifest file name inside a package.
pub(crate) const MANIFEST_FILE: &str = "extension.toml";

/// The largest package manifest this host will read.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// Hard host ceilings a manifest cannot raise.
pub(crate) const HARD_MAX_MEMORY_MB: u64 = 4096;
/// Hard host ceiling for one extension call.
pub(crate) const HARD_MAX_EXECUTION_MS: u64 = 600_000;
/// Hard host ceiling for concurrent calls.
pub(crate) const HARD_MAX_CONCURRENT_CALLS: u32 = 64;
/// Hard host ceiling for one result or event payload.
pub(crate) const HARD_MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

/// The package identity block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    /// Globally unique reverse-domain id.
    pub(crate) id: ExtensionId,
    /// Human-readable name.
    pub(crate) name: String,
    /// Semantic version.
    pub(crate) version: Version,
    /// One-paragraph description.
    pub(crate) description: String,
    /// Package authors.
    #[serde(default)]
    pub(crate) authors: Vec<String>,
    /// SPDX licence expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) license: Option<String>,
    /// Source repository URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repository: Option<String>,
    /// Project homepage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) homepage: Option<String>,
}

/// The compatibility ranges a package declares.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Compatibility {
    /// BSH versions this package supports.
    pub(crate) bsh: Requirement,
    /// Extension API generations this package uses.
    pub(crate) extension_api: Requirement,
    /// Declarative UI schema generation, when the package contributes UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ui_api: Option<Requirement>,
}

/// The runtime block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeSpec {
    /// Which runtime executes the package.
    pub(crate) kind: RuntimeKind,
    /// Package-relative entry point, or a built-in id. Required by process and
    /// wasm runtimes, forbidden by the declarative runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) entry: Option<String>,
    /// Extra process arguments. Ignored by non-process runtimes.
    #[serde(default)]
    pub(crate) args: Vec<String>,
    /// Extra environment variables. The host never forwards its own.
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    /// When the host activates the package.
    #[serde(default)]
    pub(crate) startup: StartupPolicy,
    /// Wire protocol name. Process packages use "jsonrpc-2.0".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) protocol: Option<String>,
}

/// The capability request block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapabilitiesSpec {
    /// Capabilities the package needs to activate.
    #[serde(default)]
    pub(crate) required: Vec<Capability>,
    /// Capabilities a call may use when the host grants them.
    #[serde(default)]
    pub(crate) optional: Vec<Capability>,
}

/// The contribution declaration block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContributesSpec {
    /// Agent-callable tools.
    #[serde(default)]
    pub(crate) tools: Vec<ContributionId>,
    /// Host commands.
    #[serde(default)]
    pub(crate) commands: Vec<ContributionId>,
    /// Host-managed panels.
    #[serde(default)]
    pub(crate) panels: Vec<ContributionId>,
    /// Prompt fragments.
    #[serde(default)]
    pub(crate) prompts: Vec<ContributionId>,
    /// Scoped context providers.
    #[serde(default)]
    pub(crate) contexts: Vec<ContributionId>,
    /// Reusable workflows.
    #[serde(default)]
    pub(crate) workflows: Vec<ContributionId>,
    /// Model provider descriptors (providers/models/<id>.json).
    #[serde(default)]
    pub(crate) model_providers: Vec<ContributionId>,
    /// Agent role descriptors (providers/agents/<id>.json).
    #[serde(default)]
    pub(crate) agent_providers: Vec<ContributionId>,
    /// Visual themes.
    #[serde(default)]
    pub(crate) themes: Vec<ContributionId>,
    /// Agent/module presets.
    #[serde(default)]
    pub(crate) presets: Vec<ContributionId>,
    /// Event topics the package subscribes to.
    #[serde(default)]
    pub(crate) events: Vec<String>,
    /// Event topics the package publishes.
    #[serde(default)]
    pub(crate) publishes: Vec<String>,
}

/// The configuration block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationSpec {
    /// Package-relative JSON Schema for the package's configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) schema: Option<String>,
    /// Default configuration values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) defaults: Option<toml::Value>,
}

/// The resource request block, subject to host ceilings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResourcesSpec {
    /// Memory ceiling in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_memory_mb: Option<u64>,
    /// One-call wall-clock ceiling in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_execution_ms: Option<u64>,
    /// Concurrent-call ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_concurrent_calls: Option<u32>,
    /// One-result output ceiling in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_bytes: Option<u64>,
}

impl ResourcesSpec {
    /// Effective memory ceiling.
    pub(crate) fn memory_mb(&self) -> u64 {
        self.max_memory_mb.unwrap_or(256)
    }

    /// Effective one-call wall-clock ceiling.
    pub(crate) fn execution_ms(&self) -> u64 {
        self.max_execution_ms.unwrap_or(30_000)
    }

    /// Effective concurrency ceiling.
    pub(crate) fn concurrent_calls(&self) -> u32 {
        self.max_concurrent_calls.unwrap_or(4)
    }

    /// Effective result ceiling.
    pub(crate) fn output_bytes(&self) -> u64 {
        self.max_output_bytes.unwrap_or(1024 * 1024)
    }
}

/// A complete package manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    /// Manifest schema generation. This host reads "1.0".
    pub(crate) schema_version: String,
    /// Identity block.
    pub(crate) extension: Identity,
    /// Compatibility block.
    pub(crate) compatibility: Compatibility,
    /// Runtime block.
    pub(crate) runtime: RuntimeSpec,
    /// Capability request block.
    #[serde(default)]
    pub(crate) capabilities: CapabilitiesSpec,
    /// Contribution declaration block.
    #[serde(default)]
    pub(crate) contributes: ContributesSpec,
    /// Configuration block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) configuration: Option<ConfigurationSpec>,
    /// Resource block.
    #[serde(default)]
    pub(crate) resources: ResourcesSpec,
    /// Dependency map: extension id -> version range.
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<ExtensionId, Requirement>,
}

impl Manifest {
    /// Parse a manifest from text. Syntax errors name the origin path.
    pub(crate) fn parse(text: &str, origin: &Path) -> Result<Self, ExtensionError> {
        toml::from_str(text).map_err(|error| ExtensionError::Toml {
            path: origin.to_path_buf(),
            message: error.to_string(),
        })
    }

    /// Read and parse the manifest in `root`.
    pub(crate) fn load(root: &Path) -> Result<Self, ExtensionError> {
        let path = root.join(MANIFEST_FILE);
        let metadata = std::fs::metadata(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_MANIFEST_BYTES {
            return Err(ExtensionError::Limit {
                extension: root.display().to_string(),
                message: format!("manifest is larger than {MAX_MANIFEST_BYTES} bytes"),
            });
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        Self::parse(&text, &path)
    }

    /// The extension id.
    pub(crate) fn id(&self) -> &ExtensionId {
        &self.extension.id
    }

    /// The package version.
    pub(crate) fn version(&self) -> &Version {
        &self.extension.version
    }

    /// Whether this package contributes any declarative file of `kind`.
    pub(crate) fn declares(&self, kind: ContributionKind) -> bool {
        !self.contribution_ids(kind).is_empty()
    }

    /// The contribution ids of one kind.
    pub(crate) fn contribution_ids(&self, kind: ContributionKind) -> &[ContributionId] {
        match kind {
            ContributionKind::Tool => &self.contributes.tools,
            ContributionKind::Command => &self.contributes.commands,
            ContributionKind::Panel => &self.contributes.panels,
            ContributionKind::Prompt => &self.contributes.prompts,
            ContributionKind::Context => &self.contributes.contexts,
            ContributionKind::Workflow => &self.contributes.workflows,
            ContributionKind::Theme => &self.contributes.themes,
        }
    }

    /// Every declared contribution id with its kind.
    pub(crate) fn all_contributions(&self) -> Vec<(ContributionKind, &ContributionId)> {
        let kinds = [
            ContributionKind::Tool,
            ContributionKind::Command,
            ContributionKind::Panel,
            ContributionKind::Prompt,
            ContributionKind::Context,
            ContributionKind::Workflow,
            ContributionKind::Theme,
        ];
        let mut all = Vec::new();
        for kind in kinds {
            for id in self.contribution_ids(kind) {
                all.push((kind, id));
            }
        }
        all
    }

    /// Validate the manifest against this host. Collects every problem rather
    /// than stopping at the first, because a package author needs the whole
    /// list in one run.
    pub(crate) fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            report.error(
                "schema_version",
                format!(
                    "unsupported schema version {:?}; this host reads {:?}",
                    self.schema_version, MANIFEST_SCHEMA_VERSION
                ),
            );
        }
        self.validate_identity(&mut report);
        self.validate_compatibility(&mut report);
        self.validate_runtime(&mut report);
        self.validate_capabilities(&mut report);
        self.validate_contributions(&mut report);
        self.validate_configuration(&mut report);
        self.validate_resources(&mut report);
        self.validate_dependencies(&mut report);
        report
    }

    fn validate_identity(&self, report: &mut ValidationReport) {
        let name = self.extension.name.trim();
        if name.is_empty() {
            report.error("extension.name", "must not be empty");
        } else if name.chars().count() > 128 {
            report.error("extension.name", "must be at most 128 characters");
        }
        let description = self.extension.description.trim();
        if description.is_empty() {
            report.error("extension.description", "must not be empty");
        } else if description.len() > 4096 {
            report.error("extension.description", "must be at most 4096 bytes");
        }
        for (field, value) in [
            ("extension.license", self.extension.license.as_deref()),
            ("extension.repository", self.extension.repository.as_deref()),
            ("extension.homepage", self.extension.homepage.as_deref()),
        ] {
            if value.is_some_and(|value| value.len() > 2048) {
                report.error(field, "must be at most 2048 bytes");
            }
        }
        if self.extension.authors.len() > 32 {
            report.error("extension.authors", "must contain at most 32 entries");
        }
    }

    fn validate_compatibility(&self, report: &mut ValidationReport) {
        let host = host_version();
        if !self.compatibility.bsh.matches(&host) {
            report.error(
                "compatibility.bsh",
                format!(
                    "requires bsh {}, this host is {host}",
                    self.compatibility.bsh
                ),
            );
        }
        let api = extension_api_version();
        if !self.compatibility.extension_api.matches(&api) {
            report.error(
                "compatibility.extension_api",
                format!(
                    "requires extension API {}, this host implements {api}",
                    self.compatibility.extension_api
                ),
            );
        }
        if let Some(ui) = &self.compatibility.ui_api {
            if !ui.matches(&api) {
                report.warn(
                    "compatibility.ui_api",
                    format!("declares UI API {ui}; this host validates UI documents at {api}"),
                );
            }
        }
    }

    fn validate_runtime(&self, report: &mut ValidationReport) {
        match self.runtime.kind {
            RuntimeKind::Builtin => {
                if let Some(entry) = &self.runtime.entry {
                    if let Err(reason) = validate_builtin_entry(entry) {
                        report.error("runtime.entry", reason);
                    }
                }
            }
            RuntimeKind::Declarative => {
                if self.runtime.entry.is_some() {
                    report.error(
                        "runtime.entry",
                        "the declarative runtime reads contribution files and has no entry point",
                    );
                }
            }
            RuntimeKind::Process | RuntimeKind::Mcp => {
                match &self.runtime.entry {
                    None => report.error(
                        "runtime.entry",
                        if self.runtime.kind == RuntimeKind::Mcp {
                            "the MCP runtime needs a server command or a package-relative executable"
                        } else {
                            "the process runtime needs a package-relative executable or an allowlisted command"
                        },
                    ),
                    Some(entry) => {
                        if let Err(reason) = validate_process_entry(entry) {
                            report.error("runtime.entry", reason);
                        }
                    }
                }
                if let Some(protocol) = &self.runtime.protocol {
                    let expected = if self.runtime.kind == RuntimeKind::Mcp {
                        "mcp"
                    } else {
                        "jsonrpc-2.0"
                    };
                    if protocol != expected {
                        report.error(
                            "runtime.protocol",
                            format!("the supported protocol for this runtime is \"{expected}\""),
                        );
                    }
                }
            }
            RuntimeKind::WasmComponent | RuntimeKind::Extism => match &self.runtime.entry {
                None => report.error(
                    "runtime.entry",
                    if self.runtime.kind == RuntimeKind::Extism {
                        "the Extism runtime needs a package-relative .wasm plugin"
                    } else {
                        "the wasm-component runtime needs a package-relative .wasm file"
                    },
                ),
                Some(entry) => match validate_relative_path(entry) {
                    Ok(path) => {
                        if path.extension().and_then(|value| value.to_str()) != Some("wasm") {
                            report.error("runtime.entry", "must name a .wasm file");
                        }
                    }
                    Err(reason) => report.error("runtime.entry", reason),
                },
            },
        }
        if self.runtime.args.len() > 64 {
            report.error("runtime.args", "must contain at most 64 arguments");
        }
        for (index, argument) in self.runtime.args.iter().enumerate() {
            if argument.len() > 4096 || argument.contains('\0') {
                report.error(
                    format!("runtime.args[{index}]"),
                    "must be at most 4096 bytes and contain no NUL",
                );
            }
        }
        if self.runtime.env.len() > 64 {
            report.error("runtime.env", "must contain at most 64 variables");
        }
        for (key, value) in &self.runtime.env {
            if !valid_env_name(key) {
                report.error(
                    format!("runtime.env.{key}"),
                    "variable names use ASCII letters, digits and underscores and cannot start with a digit",
                );
            }
            if is_dangerous_env(key) {
                report.error(
                    format!("runtime.env.{key}"),
                    "this variable can change how the process loads code and is not settable by a package",
                );
            }
            if value.len() > 4096 || value.contains('\0') {
                report.error(
                    format!("runtime.env.{key}"),
                    "must be at most 4096 bytes and contain no NUL",
                );
            }
        }
    }

    fn validate_capabilities(&self, report: &mut ValidationReport) {
        let mut required = BTreeSet::new();
        for capability in &self.capabilities.required {
            if !required.insert(*capability) {
                report.error(
                    "capabilities.required",
                    format!("capability {capability} is listed twice"),
                );
            }
        }
        let mut optional = BTreeSet::new();
        for capability in &self.capabilities.optional {
            if !optional.insert(*capability) {
                report.error(
                    "capabilities.optional",
                    format!("capability {capability} is listed twice"),
                );
            }
            if required.contains(capability) {
                report.warn(
                    "capabilities",
                    format!("{capability} is both required and optional; required wins"),
                );
            }
        }
    }

    fn validate_contributions(&self, report: &mut ValidationReport) {
        let mut seen: BTreeMap<&ContributionId, ContributionKind> = BTreeMap::new();
        for (kind, id) in self.all_contributions() {
            if let Some(previous) = seen.insert(id, kind) {
                report.error(
                    "contributes",
                    format!(
                        "contribution id {id:?} is declared as both {} and {}",
                        previous.as_str(),
                        kind.as_str()
                    ),
                );
            }
        }
        if self.all_contributions().is_empty()
            && self.contributes.events.is_empty()
            && self.contributes.publishes.is_empty()
        {
            report.warn(
                "contributes",
                "the package declares no tools, commands, panels, prompts, contexts, workflows, themes or events",
            );
        }
        for (field, topics) in [
            ("contributes.events", &self.contributes.events),
            ("contributes.publishes", &self.contributes.publishes),
        ] {
            if topics.len() > 256 {
                report.error(field, "must contain at most 256 topics");
            }
            for topic in topics {
                if let Err(reason) = validate_topic(topic, field.ends_with("events")) {
                    report.error(field, format!("{topic:?}: {reason}"));
                }
            }
        }
        for kind in [
            ContributionKind::Tool,
            ContributionKind::Command,
            ContributionKind::Panel,
            ContributionKind::Prompt,
            ContributionKind::Context,
            ContributionKind::Workflow,
            ContributionKind::Theme,
        ] {
            if self.contribution_ids(kind).len() > 256 {
                report.error(
                    format!("contributes.{}", kind.as_str()),
                    "must contain at most 256 entries",
                );
            }
        }
    }

    fn validate_configuration(&self, report: &mut ValidationReport) {
        let Some(configuration) = &self.configuration else {
            return;
        };
        if let Some(schema) = &configuration.schema {
            match validate_relative_path(schema) {
                Ok(path) => {
                    if path.extension().and_then(|value| value.to_str()) != Some("json") {
                        report.error("configuration.schema", "must name a .json schema");
                    }
                }
                Err(reason) => report.error("configuration.schema", reason),
            }
        }
        if let Some(defaults) = &configuration.defaults {
            match toml::to_string(defaults) {
                Ok(text) if text.len() > 64 * 1024 => {
                    report.error("configuration.defaults", "must be at most 64 KiB");
                }
                Err(error) => report.error(
                    "configuration.defaults",
                    format!("cannot be serialized: {error}"),
                ),
                Ok(_) => {}
            }
        }
    }

    fn validate_resources(&self, report: &mut ValidationReport) {
        let resources = &self.resources;
        if let Some(value) = resources.max_memory_mb {
            if value == 0 || value > HARD_MAX_MEMORY_MB {
                report.error(
                    "resources.max_memory_mb",
                    format!("must be 1..={HARD_MAX_MEMORY_MB}"),
                );
            }
        }
        if let Some(value) = resources.max_execution_ms {
            if value == 0 || value > HARD_MAX_EXECUTION_MS {
                report.error(
                    "resources.max_execution_ms",
                    format!("must be 1..={HARD_MAX_EXECUTION_MS}"),
                );
            }
        }
        if let Some(value) = resources.max_concurrent_calls {
            if value == 0 || value > HARD_MAX_CONCURRENT_CALLS {
                report.error(
                    "resources.max_concurrent_calls",
                    format!("must be 1..={HARD_MAX_CONCURRENT_CALLS}"),
                );
            }
        }
        if let Some(value) = resources.max_output_bytes {
            if value == 0 || value > HARD_MAX_OUTPUT_BYTES {
                report.error(
                    "resources.max_output_bytes",
                    format!("must be 1..={HARD_MAX_OUTPUT_BYTES}"),
                );
            }
        }
    }

    fn validate_dependencies(&self, report: &mut ValidationReport) {
        if self.dependencies.len() > 64 {
            report.error("dependencies", "must contain at most 64 entries");
        }
        for id in self.dependencies.keys() {
            if id == &self.extension.id {
                report.error("dependencies", "a package cannot depend on itself");
            }
        }
    }

    /// The trust level this package is loaded at. Built-in manifests are
    /// linked in; everything read from disk is local until a publisher
    /// verification path exists.
    pub(crate) fn default_trust(&self) -> TrustLevel {
        if self.runtime.kind == RuntimeKind::Builtin {
            TrustLevel::Builtin
        } else {
            TrustLevel::Local
        }
    }
}

/// Validate a package-relative path: no absolute paths, no parent traversal,
/// no Windows drive or backslash forms.
pub(crate) fn validate_relative_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("path must not be empty".to_string());
    }
    if trimmed.contains('\\') {
        return Err("path must use '/' separators".to_string());
    }
    if trimmed.contains('\0') {
        return Err("path must contain no NUL".to_string());
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err("path must be package-relative, not absolute".to_string());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {
                return Err("path must not contain '.' components".to_string());
            }
            Component::ParentDir => {
                return Err("path must not contain '..' components".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("path must be package-relative".to_string());
            }
        }
    }
    Ok(path.to_path_buf())
}

fn validate_builtin_entry(entry: &str) -> Result<(), String> {
    if entry.trim().is_empty() {
        return Err("a built-in entry must not be empty".to_string());
    }
    if entry.contains('/') || entry.contains('\\') || entry.contains("..") {
        return Err("a built-in entry is an id, not a path".to_string());
    }
    Ok(())
}

fn validate_process_entry(entry: &str) -> Result<(), String> {
    if entry.trim().is_empty() {
        return Err("a process entry must not be empty".to_string());
    }
    if is_allowlisted_command(entry) {
        return Ok(());
    }
    let path = validate_relative_path(entry)?;
    if path.components().count() == 1 && entry.ends_with(".exe") {
        return Err("a bare .exe name is not an allowlisted command".to_string());
    }
    Ok(())
}

/// Bare commands the host will start without a package-relative path. Every
/// one still needs an explicit process.execute grant; this list only decides
/// whether the host can find it at all.
pub(crate) fn is_allowlisted_command(entry: &str) -> bool {
    matches!(
        entry,
        "python3" | "python" | "node" | "bun" | "deno" | "sh" | "bash"
    )
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|character: char| character.is_ascii_digit())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn is_dangerous_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "PATH" | "LD_PRELOAD" | "LD_LIBRARY_PATH" | "DYLD_INSERT_LIBRARIES" | "DYLD_LIBRARY_PATH"
    ) || upper.starts_with("DYLD_")
        || upper == "PYTHONPATH"
        || upper == "NODE_OPTIONS"
}

fn validate_topic(topic: &str, allow_wildcard: bool) -> Result<(), String> {
    if topic.is_empty() || topic.len() > 128 {
        return Err("a topic must be 1..=128 bytes".to_string());
    }
    let segments: Vec<&str> = topic.split('.').collect();
    if segments.iter().all(|segment| *segment == "*") {
        return Err("a topic needs at least one non-wildcard segment".to_string());
    }
    for (index, segment) in segments.iter().enumerate() {
        if *segment == "*" {
            if !allow_wildcard || index != segments.len() - 1 {
                return Err(
                    "'*' is only allowed as the final segment of a subscription topic".to_string(),
                );
            }
            continue;
        }
        if segment.is_empty() {
            return Err("topic segments must not be empty".to_string());
        }
        if !segment.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        }) {
            return Err(format!(
                "topic segment {segment:?} may only contain lowercase letters, digits, '-' and '_'"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        r#"
schema_version = "1.0"

[extension]
id = "com.example.git-assistant"
name = "Git Assistant"
version = "1.0.0"
description = "Git tools for the BSH agent"
authors = ["Example Developer"]
license = "MIT"
repository = "https://example.com/git-assistant"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"
startup = "on-demand"

[capabilities]
required = ["agent.tools.register", "workspace.read"]
optional = ["workspace.write"]

[contributes]
tools = ["git.status", "git.diff"]
commands = ["git.refresh"]
panels = ["git.panel"]
events = ["workspace.opened"]
publishes = ["git.changed"]

[resources]
max_memory_mb = 128
max_execution_ms = 5000
max_concurrent_calls = 4
"#
        .to_string()
    }

    fn parse(text: &str) -> Manifest {
        Manifest::parse(text, Path::new("extension.toml")).expect("manifest parses")
    }

    #[test]
    fn a_complete_manifest_parses_and_validates() {
        let manifest = parse(&sample());
        assert_eq!(manifest.extension.id.as_str(), "com.example.git-assistant");
        assert_eq!(manifest.extension.version, Version::new(1, 0, 0));
        assert_eq!(manifest.contributes.tools.len(), 2);
        assert_eq!(manifest.resources.memory_mb(), 128);
        let report = manifest.validate();
        assert!(report.is_valid(), "{report:?}");
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    #[test]
    fn unknown_fields_are_refused() {
        let text = sample().replace(
            "schema_version = \"1.0\"",
            "schema_version = \"1.0\"\nsurprise = true",
        );
        assert!(Manifest::parse(&text, Path::new("extension.toml")).is_err());
    }

    #[test]
    fn an_unsupported_schema_version_is_an_error() {
        let text = sample().replace("schema_version = \"1.0\"", "schema_version = \"2.0\"");
        let report = parse(&text).validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "schema_version"));
    }

    #[test]
    fn an_incompatible_host_range_is_an_error() {
        let text = sample().replace(">=3.0.0, <4.0.0", ">=999.0.0");
        let report = parse(&text).validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "compatibility.bsh"));
    }

    #[test]
    fn process_entry_rejects_traversal_and_absolute_paths() {
        for entry in ["../escape", "/bin/sh", "tools/../../escape", "a\\\\b"] {
            assert!(
                validate_relative_path(entry).is_err(),
                "{entry:?} should be refused"
            );
        }
        assert!(validate_relative_path("bin/server").is_ok());
        assert!(validate_process_entry("python3").is_ok());
        assert!(validate_process_entry("/usr/bin/python3").is_err());
    }

    #[test]
    fn a_declarative_package_cannot_declare_an_entry() {
        let text = sample().replace(
            "kind = \"declarative\"",
            "kind = \"declarative\"\nentry = \"bin/server\"",
        );
        let report = parse(&text).validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "runtime.entry"));
    }

    #[test]
    fn resource_requests_are_capped() {
        let text = sample().replace("max_memory_mb = 128", "max_memory_mb = 100_000");
        let report = parse(&text).validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "resources.max_memory_mb"));
    }

    #[test]
    fn dangerous_environment_variables_are_refused() {
        let text = sample().replace(
            "[resources]",
            "[runtime.env]\nLD_PRELOAD = \"/tmp/evil.so\"\n\n[resources]",
        );
        let report = parse(&text).validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "runtime.env.LD_PRELOAD"));
    }

    #[test]
    fn one_contribution_id_cannot_have_two_kinds() {
        let text = sample().replace(
            "commands = [\"git.refresh\"]",
            "commands = [\"git.status\"]",
        );
        let report = parse(&text).validate();
        assert!(!report.is_valid());
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.field == "contributes"));
    }

    #[test]
    fn a_capability_in_both_lists_is_a_warning_not_an_error() {
        let text = sample().replace(
            "optional = [\"workspace.write\"]",
            "optional = [\"workspace.read\"]",
        );
        let report = parse(&text).validate();
        assert!(report.is_valid(), "{report:?}");
        assert!(report
            .warnings
            .iter()
            .any(|issue| issue.field == "capabilities"));
    }

    #[test]
    fn event_topics_are_validated() {
        for topic in ["workspace.opened", "tool.*", "agent.completed"] {
            assert!(
                validate_topic(topic, topic.ends_with('*')).is_ok(),
                "{topic}"
            );
        }
        for topic in ["Workspace.Opened", "workspace..opened", "*", "a.*.b"] {
            assert!(validate_topic(topic, true).is_err(), "{topic}");
        }
    }

    #[test]
    fn manifests_round_trip_through_toml() {
        let manifest = parse(&sample());
        let text = toml::to_string_pretty(&manifest).expect("serialize");
        let read = parse(&text);
        assert_eq!(read.extension.id, manifest.extension.id);
        assert_eq!(read.contributes.tools, manifest.contributes.tools);
    }
}
