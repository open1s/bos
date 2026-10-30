//! Core domain types for the BSH extension platform: identity, versions,
//! compatibility, lifecycle vocabulary and the one error type every layer
//! reports through.
//!
//! Nothing here knows about GPUI, Wasmtime or a concrete process. A manifest,
//! a registry record, a runtime and a tool all speak these types, which is what
//! keeps the platform independent of the host that embeds it.

use std::fmt;
use std::path::PathBuf;

use semver::{Version, VersionReq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// The extension API generation this build implements.
///
/// A manifest's `[compatibility] extension_api` range is checked against this
/// version before a package is allowed to activate.
pub(crate) const EXTENSION_API_VERSION: &str = "1.0.0";

/// The manifest schema version this build understands.
pub(crate) const MANIFEST_SCHEMA_VERSION: &str = "1.0";

/// The BSH host version, used for `[compatibility] bsh` checks.
pub(crate) const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The host version as a parsed [`Version`].
pub(crate) fn host_version() -> Version {
    Version::parse(HOST_VERSION).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// The extension API version as a parsed [`Version`].
pub(crate) fn extension_api_version() -> Version {
    Version::parse(EXTENSION_API_VERSION).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// A globally unique extension identifier in reverse-domain form.
///
/// At least two dot-separated segments, each 1..=63 lowercase ASCII
/// alphanumeric, `-` or `_`, starting with a letter or digit and ending with a
/// letter or digit. The validation is deliberately narrower than DNS: it is
/// also the on-disk directory name, a TOML key and the prefix of every tool the
/// extension contributes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ExtensionId(String);

impl ExtensionId {
    /// Parse and validate an identifier.
    pub(crate) fn parse(raw: impl Into<String>) -> Result<Self, ExtensionError> {
        let raw = raw.into();
        validate_extension_id(&raw).map_err(|reason| ExtensionError::InvalidId {
            id: raw.clone(),
            reason,
        })?;
        Ok(Self(raw))
    }

    /// The identifier as a string.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment, which namespaces contributed tool names.
    pub(crate) fn namespace(&self) -> &str {
        self.0.rsplit('.').next().unwrap_or(&self.0)
    }

    /// The model-facing name of one contribution owned by this extension.
    ///
    /// Tool names are a flat global namespace in every LLM API, so a dotted
    /// contribution id is flattened and prefixed with the extension's last
    /// segment: `com.example.git-assistant` plus `git.status` becomes
    /// `git-assistant__git_status`. The result is capped at 64 characters,
    /// the common provider limit.
    pub(crate) fn qualify(&self, contribution: &ContributionId) -> String {
        let namespace = sanitize_name(self.namespace());
        let local = sanitize_name(&contribution.as_str().replace('.', "_"));
        let mut qualified = format!("{namespace}__{local}");
        if qualified.len() > 64 {
            qualified.truncate(64);
            while qualified.ends_with('_') {
                qualified.pop();
            }
        }
        qualified
    }
}

impl fmt::Display for ExtensionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ExtensionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ExtensionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        ExtensionId::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// Check the reverse-domain identifier grammar.
pub(crate) fn validate_extension_id(raw: &str) -> Result<(), String> {
    if raw.is_empty() || raw.len() > 128 {
        return Err("an extension id must be 1..=128 bytes".to_string());
    }
    let segments: Vec<&str> = raw.split('.').collect();
    if segments.len() < 2 {
        return Err("an extension id must have at least two dot-separated segments".to_string());
    }
    for segment in segments {
        if segment.is_empty() || segment.len() > 63 {
            return Err("each id segment must be 1..=63 bytes".to_string());
        }
        for (index, character) in segment.chars().enumerate() {
            let allowed = character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || ((character == '-' || character == '_') && index > 0);
            if !allowed {
                return Err(format!(
                    "id segment {segment:?} may only contain lowercase ASCII letters, digits, '-' and '_', and must start with a letter or digit"
                ));
            }
        }
        if segment.ends_with('-') || segment.ends_with('_') {
            return Err(format!(
                "id segment {segment:?} must end with a letter or digit"
            ));
        }
    }
    Ok(())
}

/// Replace every character that is not ASCII alphanumeric with `_` and trim
/// the result. Used only for model-facing names, never for identity.
fn sanitize_name(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized
        .trim_matches(|character| character == '_' || character == '-')
        .to_string();
    if trimmed.is_empty() {
        "extension".to_string()
    } else {
        trimmed
    }
}

/// A contribution identifier, unique within one extension.
///
/// Dotted, lowercase, 1..=96 bytes. The platform qualifies it with the owning
/// extension before registering it in a host namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ContributionId(String);

impl ContributionId {
    /// Parse and validate a contribution id.
    pub(crate) fn parse(raw: impl Into<String>) -> Result<Self, ExtensionError> {
        let raw = raw.into();
        validate_contribution_id(&raw).map_err(|reason| ExtensionError::InvalidContribution {
            id: raw.clone(),
            reason,
        })?;
        Ok(Self(raw))
    }

    /// The identifier as a string.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment, which is the local file name and tool suffix.
    pub(crate) fn local_name(&self) -> &str {
        self.0.rsplit('.').next().unwrap_or(&self.0)
    }
}

impl fmt::Display for ContributionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ContributionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ContributionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        ContributionId::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// Check the dotted contribution identifier grammar.
pub(crate) fn validate_contribution_id(raw: &str) -> Result<(), String> {
    if raw.is_empty() || raw.len() > 96 {
        return Err("a contribution id must be 1..=96 bytes".to_string());
    }
    for segment in raw.split('.') {
        if segment.is_empty() || segment.len() > 63 {
            return Err("each contribution segment must be 1..=63 bytes".to_string());
        }
        if !segment
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        {
            return Err(format!(
                "contribution segment {segment:?} must start with a lowercase letter or digit"
            ));
        }
        if !segment.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        }) {
            return Err(format!(
                "contribution segment {segment:?} may only contain lowercase ASCII letters, digits, '-' and '_'"
            ));
        }
    }
    Ok(())
}

/// The execution runtime a package asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RuntimeKind {
    /// First-party Rust code linked into BSH and registered by id.
    Builtin,
    /// Pure declarative contributions read from the package directory.
    Declarative,
    /// An external process speaking the platform's JSON-RPC protocol.
    Process,
    /// An external Model Context Protocol server over stdio.
    Mcp,
    /// A WebAssembly Component. The contract is published in `wit/`; this
    /// build refuses to activate one until the Wasmtime runtime lands.
    WasmComponent,
}

impl RuntimeKind {
    /// Stable lowercase name used in logs and lockfiles.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Declarative => "declarative",
            Self::Process => "process",
            Self::Mcp => "mcp",
            Self::WasmComponent => "wasm-component",
        }
    }
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// When the host should activate a package.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StartupPolicy {
    /// Activate as the host starts.
    OnStartup,
    /// Activate when a contribution is first used. The default for
    /// third-party packages.
    #[default]
    OnDemand,
    /// Activate when a workspace opens.
    OnWorkspaceOpen,
    /// Never activate automatically; the reader or a command does.
    Manual,
}

/// How much a package is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TrustLevel {
    /// Shipped with BSH and linked in.
    Builtin,
    /// A package from a verified publisher. This build records the level but
    /// does not yet verify signatures; `doctor` reports that honestly.
    Verified,
    /// A package from a public source.
    Community,
    /// The reader's own development directory.
    Local,
    /// Source could not be established.
    Unknown,
}

impl TrustLevel {
    /// Stable name used in status rows.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Verified => "verified",
            Self::Community => "community",
            Self::Local => "local",
            Self::Unknown => "unknown",
        }
    }
}

/// Where an installed package came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ExtensionSource {
    /// Linked into the host.
    Builtin,
    /// Copied into the managed `installed/` tree.
    #[default]
    Installed,
    /// Read in place from a development directory.
    Development,
    /// An official or private registry.
    Registry {
        /// Registry base URL.
        url: String,
    },
    /// A Git checkout.
    Git {
        /// Repository URL.
        repository: String,
        /// Revision.
        revision: String,
    },
}

impl ExtensionSource {
    /// A short label for status output.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Builtin => "builtin".to_string(),
            Self::Installed => "installed".to_string(),
            Self::Development => "development".to_string(),
            Self::Registry { url } => format!("registry {url}"),
            Self::Git {
                repository,
                revision,
            } => format!("git {repository}@{revision}"),
        }
    }
}

/// The lifecycle states a package moves through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleState {
    /// Found on disk, not read yet.
    Discovered,
    /// The manifest is being read and validated.
    Validating,
    /// The manifest is valid and its dependencies are resolved.
    Resolved,
    /// Being copied into the managed tree.
    Installing,
    /// Copied and hashed, not loaded.
    Installed,
    /// The runtime is being constructed.
    Loading,
    /// The runtime is starting.
    Starting,
    /// Ready to accept calls.
    Active,
    /// Being suspended.
    Suspending,
    /// Suspended; in-flight calls were cancelled.
    Suspended,
    /// Being stopped.
    Stopping,
    /// Stopped; runtime resources released.
    Unloaded,
    /// Failed; see the record's error.
    Failed,
}

impl LifecycleState {
    /// Stable name used in state files and diagnostics.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Validating => "validating",
            Self::Resolved => "resolved",
            Self::Installing => "installing",
            Self::Installed => "installed",
            Self::Loading => "loading",
            Self::Starting => "starting",
            Self::Active => "active",
            Self::Suspending => "suspending",
            Self::Suspended => "suspended",
            Self::Stopping => "stopping",
            Self::Unloaded => "unloaded",
            Self::Failed => "failed",
        }
    }

    /// Whether this state can accept a tool call.
    pub(crate) fn is_ready(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Whether this state represents a terminal failure.
    pub(crate) fn is_failed(self) -> bool {
        matches!(self, Self::Failed)
    }
}

impl fmt::Display for LifecycleState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One contribution kind a package can declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ContributionKind {
    /// An agent-callable tool.
    Tool,
    /// A host command.
    Command,
    /// A host-managed UI panel.
    Panel,
    /// A prompt fragment.
    Prompt,
    /// A scoped context provider.
    Context,
    /// A workflow.
    Workflow,
    /// A theme.
    Theme,
}

impl ContributionKind {
    /// Stable plural name used in diagnostics.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tools",
            Self::Command => "commands",
            Self::Panel => "panels",
            Self::Prompt => "prompts",
            Self::Context => "contexts",
            Self::Workflow => "workflows",
            Self::Theme => "themes",
        }
    }
}

/// One field-level validation problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidationIssue {
    /// Dotted manifest path, for example `runtime.entry`.
    pub(crate) field: String,
    /// What is wrong and what the accepted shape is.
    pub(crate) message: String,
}

impl ValidationIssue {
    /// Build an issue.
    pub(crate) fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

/// The full result of validating one manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ValidationReport {
    /// Problems that prevent loading.
    pub(crate) errors: Vec<ValidationIssue>,
    /// Problems the host can live with.
    pub(crate) warnings: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// Whether no error was recorded.
    pub(crate) fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Record an error.
    pub(crate) fn error(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.errors.push(ValidationIssue::new(field, message));
    }

    /// Record a warning.
    pub(crate) fn warn(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(ValidationIssue::new(field, message));
    }

    /// Append another report.
    pub(crate) fn merge(&mut self, other: ValidationReport) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }
}

/// Everything that can go wrong while discovering, loading, authorizing,
/// running or unloading an extension.
#[derive(Debug, Error)]
pub(crate) enum ExtensionError {
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path being read or written.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A manifest is not valid TOML.
    #[error("{path}: invalid TOML: {message}")]
    Toml {
        /// Manifest path.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// A JSON contribution file is malformed.
    #[error("{path}: invalid JSON: {message}")]
    Json {
        /// File path.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// A manifest is syntactically valid but semantically wrong.
    #[error("{path}: {message}")]
    Manifest {
        /// Manifest path.
        path: PathBuf,
        /// Human-readable problem, usually a joined [`ValidationReport`].
        message: String,
    },
    /// An extension id does not match the standard.
    #[error("invalid extension id {id:?}: {reason}")]
    InvalidId {
        /// The offending id.
        id: String,
        /// Why it is invalid.
        reason: String,
    },
    /// A contribution id does not match the standard.
    #[error("invalid contribution id {id:?}: {reason}")]
    InvalidContribution {
        /// The offending id.
        id: String,
        /// Why it is invalid.
        reason: String,
    },
    /// A semantic version or range could not be parsed.
    #[error("invalid version {value:?}: {reason}")]
    InvalidVersion {
        /// The offending value.
        value: String,
        /// Why it is invalid.
        reason: String,
    },
    /// The package does not support this host or API generation.
    #[error(
        "extension {extension} is incompatible: it requires bsh {requirement}, this host is {host}"
    )]
    Incompatible {
        /// Extension id.
        extension: String,
        /// The requested range.
        requirement: String,
        /// The host version.
        host: String,
    },
    /// A lifecycle transition is not in the state machine.
    #[error("extension {extension} cannot move from {from} to {to}")]
    Lifecycle {
        /// Extension id.
        extension: String,
        /// Current state.
        from: LifecycleState,
        /// Requested state.
        to: LifecycleState,
    },
    /// The runtime kind has no implementation in this build.
    #[error("runtime {kind} is not implemented in this build")]
    UnsupportedRuntime {
        /// Runtime kind.
        kind: String,
    },
    /// A required capability was denied by policy.
    #[error(
        "extension {extension} requires capability {capability}, which policy denied: {reason}"
    )]
    MissingCapability {
        /// Extension id.
        extension: String,
        /// Capability id.
        capability: String,
        /// Policy reason.
        reason: String,
    },
    /// A call asked for a capability the extension does not hold.
    #[error("extension {extension} is not authorized for {capability}: {reason}")]
    Denied {
        /// Extension id.
        extension: String,
        /// Capability id.
        capability: String,
        /// Policy reason.
        reason: String,
    },
    /// A dependency is missing, conflicting or cyclic.
    #[error("dependency error for {extension}: {message}")]
    Dependency {
        /// Extension id at which the problem surfaced.
        extension: String,
        /// Details.
        message: String,
    },
    /// The dependency graph contains a cycle.
    #[error("extension dependency cycle: {chain}")]
    Cycle {
        /// The cycle, in visiting order.
        chain: String,
    },
    /// The runtime reported an error.
    #[error("extension {extension} runtime error: {message}")]
    Runtime {
        /// Extension id.
        extension: String,
        /// Runtime message.
        message: String,
    },
    /// The runtime violated the wire protocol.
    #[error("extension {extension} protocol error: {message}")]
    Protocol {
        /// Extension id.
        extension: String,
        /// Protocol message.
        message: String,
    },
    /// An operation exceeded its wall-clock bound.
    #[error("extension {extension} {operation} timed out after {millis} ms")]
    Timeout {
        /// Extension id.
        extension: String,
        /// Operation name.
        operation: String,
        /// Bound in milliseconds.
        millis: u64,
    },
    /// A declared limit was exceeded.
    #[error("extension {extension} exceeded a limit: {message}")]
    Limit {
        /// Extension id.
        extension: String,
        /// What was exceeded.
        message: String,
    },
    /// A package is not installed.
    #[error("extension {id} is not installed")]
    NotInstalled {
        /// Extension id.
        id: String,
    },
    /// A tool or contribution does not exist in the loaded extension.
    #[error("{id} was not found")]
    NotFound {
        /// The looked-up name.
        id: String,
    },
    /// A package version is already installed.
    #[error("extension {id} {version} is already installed")]
    AlreadyInstalled {
        /// Extension id.
        id: String,
        /// Version.
        version: String,
    },
    /// A package could not be found in any configured source.
    #[error("no package provides {id} matching {requirement}")]
    NoMatchingVersion {
        /// Extension id.
        id: String,
        /// Requested range.
        requirement: String,
    },
    /// A stored document is malformed.
    #[error("{path}: {message}")]
    Corrupt {
        /// File path.
        path: PathBuf,
        /// The problem.
        message: String,
    },
    /// A name is already claimed in a shared namespace.
    #[error("contribution name {name:?} is already claimed by {owner}")]
    NameConflict {
        /// The name.
        name: String,
        /// The current owner.
        owner: String,
    },
}

impl ExtensionError {
    /// A stable category for tool errors and UI rows.
    pub(crate) fn category(&self) -> &'static str {
        match self {
            Self::Io { .. } | Self::Toml { .. } | Self::Json { .. } | Self::Corrupt { .. } => "io",
            Self::Manifest { .. }
            | Self::InvalidId { .. }
            | Self::InvalidContribution { .. }
            | Self::InvalidVersion { .. }
            | Self::Incompatible { .. } => "manifest",
            Self::Lifecycle { .. } => "lifecycle",
            Self::UnsupportedRuntime { .. } => "unsupported",
            Self::MissingCapability { .. } | Self::Denied { .. } => "permission",
            Self::Dependency { .. } | Self::Cycle { .. } => "dependency",
            Self::Runtime { .. } | Self::Protocol { .. } => "runtime",
            Self::Timeout { .. } => "timeout",
            Self::Limit { .. } => "limit",
            Self::NotInstalled { .. }
            | Self::AlreadyInstalled { .. }
            | Self::NoMatchingVersion { .. } => "install",
            Self::NotFound { .. } => "not_found",
            Self::NameConflict { .. } => "conflict",
        }
    }

    /// Map to the agent tool error the model sees.
    pub(crate) fn to_tool_error(&self) -> agent::tools::ToolError {
        match self {
            Self::Denied { .. } | Self::MissingCapability { .. } => {
                agent::tools::ToolError::Failed(format!("permission denied: {self}"))
            }
            Self::Timeout { .. } => agent::tools::ToolError::Timeout(self.to_string()),
            Self::NotFound { id } => agent::tools::ToolError::NotFound(id.clone()),
            Self::Limit { .. } => agent::tools::ToolError::Failed(self.to_string()),
            Self::InvalidId { .. }
            | Self::InvalidContribution { .. }
            | Self::InvalidVersion { .. }
            | Self::Manifest { .. } => agent::tools::ToolError::InvalidInput(self.to_string()),
            _ => agent::tools::ToolError::Failed(self.to_string()),
        }
    }
}

/// A parsed semantic version requirement with the original text kept for
/// diagnostics and lockfile output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Requirement {
    /// The original range text.
    pub(crate) raw: String,
    /// The parsed range.
    pub(crate) parsed: VersionReq,
}

impl Requirement {
    /// Parse a requirement.
    pub(crate) fn parse(raw: impl Into<String>) -> Result<Self, ExtensionError> {
        let raw = raw.into();
        let parsed = VersionReq::parse(&raw).map_err(|error| ExtensionError::InvalidVersion {
            value: raw.clone(),
            reason: error.to_string(),
        })?;
        Ok(Self { raw, parsed })
    }

    /// Whether `version` satisfies the range.
    pub(crate) fn matches(&self, version: &Version) -> bool {
        self.parsed.matches(version)
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.raw)
    }
}

impl Serialize for Requirement {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for Requirement {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Requirement::parse(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_ids_follow_the_reverse_domain_grammar() {
        for valid in [
            "com.example.git-assistant",
            "com.bos.bsh.hello",
            "io.2code.tool",
            "a.b_c-d",
        ] {
            assert!(ExtensionId::parse(valid).is_ok(), "{valid} should parse");
        }
        for invalid in [
            "",
            "one-segment",
            "Com.Example.Tool",
            "com..example",
            "com.example..tool",
            "com.example.tool.",
            "com.example.-tool",
            "com/example/tool",
            "com.example.tool ",
        ] {
            assert!(
                ExtensionId::parse(invalid).is_err(),
                "{invalid:?} should fail"
            );
        }
        assert!(ExtensionId::parse("x".repeat(129)).is_err());
    }

    #[test]
    fn contribution_ids_are_dotted_and_unambiguous() {
        assert!(ContributionId::parse("git.status").is_ok());
        assert!(ContributionId::parse("refresh").is_ok());
        for invalid in [
            "",
            ".git",
            "git.",
            "git..status",
            "Git.Status",
            "git/status",
        ] {
            assert!(ContributionId::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn qualification_flattens_dots_and_caps_length() {
        let owner = ExtensionId::parse("com.example.git-assistant").expect("id");
        let contribution = ContributionId::parse("git.status").expect("contribution");
        assert_eq!(
            owner.qualify(&contribution),
            "git-assistant__git_status".to_string()
        );
        assert_eq!(owner.namespace(), "git-assistant");

        let owner =
            ExtensionId::parse("com.example.a-very-long-namespace-name-for-tools").expect("id");
        let contribution = ContributionId::parse("x".repeat(60)).expect("contribution");
        assert!(owner.qualify(&contribution).len() <= 64);
    }

    #[test]
    fn requirements_keep_their_text_and_match_semver() {
        let requirement = Requirement::parse("^1.2").expect("range");
        assert_eq!(requirement.to_string(), "^1.2");
        assert!(requirement.matches(&Version::new(1, 5, 0)));
        assert!(!requirement.matches(&Version::new(2, 0, 0)));
        assert!(Requirement::parse("not a range").is_err());
    }

    #[test]
    fn lifecycle_states_have_stable_names() {
        assert_eq!(LifecycleState::Active.as_str(), "active");
        assert!(LifecycleState::Active.is_ready());
        assert!(LifecycleState::Failed.is_failed());
        assert!(!LifecycleState::Suspended.is_ready());
    }
}
