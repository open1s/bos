//! Common extension substrate for the DeepSeek Harness and Codex plugin
//! standards.
//!
//! One registry serves both standards. A package is parsed by the shim for its
//! standard (`dsh` or `codex`), lowered onto the one [`Extension`] model,
//! and its contributions are registered once in [`ExtensionRegistry`]. The
//! registry can be queried by standard, capability, or contribution kind, and
//! can install its contributions onto the existing seams:
//!
//! | Contribution | Existing seam | Installed by |
//! |---|---|---|
//! | [`Contribution::Tool`] | [`react::tool::Tool`] / [`react::tool::ToolRegistry`] | [`ExtensionRegistry::apply_to_agent`] -> [`crate::agent::Agent::try_add_tool`] |
//! | [`Contribution::Hook`] | [`crate::agent::plugin::AgentPlugin`] and its wrappers ([`crate::agent::plugin::LlmRequestWrapper`], [`crate::agent::plugin::LlmResponseWrapper`], [`crate::agent::plugin::StreamTokenWrapper`], [`crate::agent::plugin::ToolCallWrapper`], [`crate::agent::plugin::ToolResultWrapper`]) | [`ExtensionRegistry::apply_to_agent`] -> [`crate::agent::Agent::add_plugin`] |
//! | [`Contribution::Command`] | [`crate::plugins::PluginCommand`] | [`ExtensionRegistry::install_host_items`] -> [`crate::plugins::PluginRegistry::register_command`] |
//! | [`Contribution::Panel`] | [`crate::plugins::UiPanel`] | [`ExtensionRegistry::install_host_items`] -> [`crate::plugins::PluginRegistry::register_panel`] |
//! | [`Contribution::Skill`] | [`crate::skills::SkillLoader`] / [`crate::skills::SkillContent`] | [`ExtensionRegistry::apply_to_agent`] -> [`crate::agent::Agent::register_skill_manifest`] |
//! | [`Contribution::SkillDir`] | [`crate::skills::SkillLoader`] | [`ExtensionRegistry::apply_to_agent`] -> [`crate::agent::Agent::register_skills_from_dir`] |
//! | [`Contribution::Prompt`] | host prompt surface; a `commands/*.md` prompt also yields a callable [`Contribution::Tool`] | [`ExtensionRegistry::prompts`] / [`crate::agent::Agent::try_add_tool`] |
//!
//! # What the two standards actually declare
//!
//! The shipped DeepSeek Harness manifest (`dsh-package-manifest`, version
//! `0.2.1-rc.2`) carries identity in `package.json` and a `dsh` object
//! with three public fields: `dsh.manifestVersion` (format `1`),
//! `dsh.bundle.patch` (a `cordis.patch.yml` path or an ordered list), and
//! `dsh.client` (`platform`, `immediately`, `inject`, `external`). Its host
//! entry exports `apply(ctx, config)` and registers capabilities imperatively
//! with `ctx.tools.register(defineTool(...))`, `ctx.commands.register(...)`,
//! `ctx.skills.register(...)`, and Cordis events (`ctx.on`). There is no
//! `dsh.tools`, `dsh.commands`, or `dsh.skills` manifest field. This shim
//! therefore parses the declarative fields and refuses the JavaScript entry
//! with a [`Refusal`]: the workspace has no JavaScript runtime, so executing it
//! would be a lie. A bundle patch is parsed for its row ids and then recorded as
//! a [`RefusedFeature::BundleMount`] refusal: mounting the rows needs the DSH
//! Cordis loader, which this shim does not embed.
//!
//! A Codex plugin carries `.codex-plugin/plugin.json` (`name`, `version`,
//! `description`, `author`, `homepage`, `repository`, `license`, `keywords`,
//! `skills`, `mcpServers`, `apps`, `interface`), `commands/*.md` prompt files
//! with a `description` frontmatter and `$ARGUMENTS` interpolation, and
//! `skills/<name>/SKILL.md`. Its MCP servers and app connectors are external
//! processes and are refused.
//!
//! Because neither standard declares tools as JSON Schema in its manifest, both
//! shims read the same explicitly BOS-owned declarative surface when a package
//! ships it: `tools/*.json` (a JSON Schema plus a pure `echo_arguments`,
//! `constant`, or `template` body), `commands/*.md`, `skills/*/SKILL.md`, and
//! `agents/*.md`. Those files contribute real tools, commands, skills, and
//! prompts without executing code; the imperative path stays refused.

mod codex;
mod declarative;
mod dsh;
mod registry;

pub use registry::{AppliedExtensions, ExtensionRegistry, HostInstall, LoadedPackage};

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::agent::plugin::{AgentPlugin, LlmStage, ToolStage};
use crate::plugins::{PluginCommand, UiPanel};
use crate::tools::Tool;

/// The package standard an extension was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtensionStandard {
    /// A DeepSeek Harness package: `package.json` with a `dsh` object.
    DeepSeekHarness,
    /// A Codex package: `.codex-plugin/plugin.json`.
    Codex,
}

impl ExtensionStandard {
    /// The short, stable name used in errors and inventories.
    pub fn as_str(self) -> &'static str {
        match self {
            ExtensionStandard::DeepSeekHarness => "dsh",
            ExtensionStandard::Codex => "codex",
        }
    }

    /// Detect the standard of the package at `root`.
    ///
    /// A directory carrying both markers is ambiguous and is refused rather
    /// than guessed: reading a Codex plugin as DSH (or the reverse) would
    /// silently load the wrong contribution set.
    pub fn detect(root: &Path) -> Result<Self, ExtensionError> {
        let codex = root.join(".codex-plugin").join("plugin.json").is_file();
        let dsh = match package_json_has_dsh(root) {
            Ok(found) => found,
            Err(_) if codex => false,
            Err(error) => return Err(error),
        };
        match (dsh, codex) {
            (true, true) => Err(ExtensionError::AmbiguousStandard {
                path: root.to_path_buf(),
            }),
            (true, false) => Ok(ExtensionStandard::DeepSeekHarness),
            (false, true) => Ok(ExtensionStandard::Codex),
            (false, false) => Err(ExtensionError::UnknownStandard {
                path: root.to_path_buf(),
            }),
        }
    }
}

impl fmt::Display for ExtensionStandard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ExtensionStandard::DeepSeekHarness => "DeepSeek Harness",
            ExtensionStandard::Codex => "Codex",
        })
    }
}

/// A contribution kind, used for capability queries and name collision checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContributionKind {
    /// A model-facing tool.
    Tool,
    /// A plugin hook (the `AgentPlugin` seam).
    Hook,
    /// A host command.
    Command,
    /// One `SKILL.md` skill.
    Skill,
    /// A directory of skills.
    SkillDir,
    /// A UI panel.
    Panel,
    /// A prompt template.
    Prompt,
}

impl fmt::Display for ContributionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ContributionKind {
    /// The short, stable name used in inventories and errors.
    pub fn as_str(self) -> &'static str {
        match self {
            ContributionKind::Tool => "tool",
            ContributionKind::Hook => "hook",
            ContributionKind::Command => "command",
            ContributionKind::Skill => "skill",
            ContributionKind::SkillDir => "skill directory",
            ContributionKind::Panel => "panel",
            ContributionKind::Prompt => "prompt",
        }
    }
}

/// A capability an extension declares, whether or not the contribution loaded.
///
/// A package can declare a capability it could not deliver (an imperative DSH
/// host entry, a Codex MCP server). The capability stays listed so a host can
/// show what the package asked for next to the [`Refusal`] explaining why it is
/// unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// Model-facing tools.
    Tools,
    /// Plugin hooks (`AgentPlugin`).
    Hooks,
    /// Host commands.
    Commands,
    /// Skills.
    Skills,
    /// Prompt templates.
    Prompts,
    /// UI panels.
    Panels,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Capability {
    /// The short, stable name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Tools => "tools",
            Capability::Hooks => "hooks",
            Capability::Commands => "commands",
            Capability::Skills => "skills",
            Capability::Prompts => "prompts",
            Capability::Panels => "panels",
        }
    }
}

/// One extension package, lowered onto the common model.
#[derive(Clone)]
pub struct Extension {
    /// Stable package id (the npm name for DSH, the `name` field for Codex).
    pub id: String,
    /// Display name (DSH locale `meta.title`, Codex `interface.displayName`).
    pub name: String,
    /// Package version as declared by the manifest.
    pub version: String,
    /// The standard the package was read as.
    pub standard: ExtensionStandard,
    /// Capabilities the package declared or delivered.
    pub capabilities: Vec<Capability>,
    /// Package root directory.
    pub source: PathBuf,
    /// The manifest file the package was read from.
    pub manifest: PathBuf,
    /// Display description, when the package declares one.
    pub description: Option<String>,
    /// Standard-specific facts that are safe to expose (never config values).
    pub details: ExtensionDetails,
    /// The contributions loaded from the package.
    pub contributions: Vec<Contribution>,
}

impl fmt::Debug for Extension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extension")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("version", &self.version)
            .field("standard", &self.standard)
            .field("capabilities", &self.capabilities)
            .field("contributions", &self.contributions)
            .finish_non_exhaustive()
    }
}

impl Extension {
    /// Create an extension with no contributions yet.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        standard: ExtensionStandard,
        source: impl Into<PathBuf>,
    ) -> Self {
        let source = source.into();
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            standard,
            capabilities: Vec::new(),
            manifest: source.clone(),
            source,
            description: None,
            details: ExtensionDetails::default(),
            contributions: Vec::new(),
        }
    }

    /// Set the manifest path.
    pub fn with_manifest(mut self, manifest: impl Into<PathBuf>) -> Self {
        self.manifest = manifest.into();
        self
    }

    /// Set the display description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Declare one capability.
    pub fn with_capability(mut self, capability: Capability) -> Self {
        if !self.capabilities.contains(&capability) {
            self.capabilities.push(capability);
            self.capabilities.sort();
        }
        self
    }

    /// Declare several capabilities.
    pub fn with_capabilities(mut self, capabilities: impl IntoIterator<Item = Capability>) -> Self {
        for capability in capabilities {
            if !self.capabilities.contains(&capability) {
                self.capabilities.push(capability);
            }
        }
        self.capabilities.sort();
        self
    }

    /// Add one contribution.
    pub fn with_contribution(mut self, contribution: Contribution) -> Self {
        self.contributions.push(contribution);
        self
    }

    /// Add several contributions.
    pub fn with_contributions(
        mut self,
        contributions: impl IntoIterator<Item = Contribution>,
    ) -> Self {
        self.contributions.extend(contributions);
        self
    }

    /// Set the standard-specific details.
    pub fn with_details(mut self, details: ExtensionDetails) -> Self {
        self.details = details;
        self
    }

    /// Whether the package declared `capability`.
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Standard-specific facts about an extension.
///
/// Everything here is declarative metadata. Configuration values, environment
/// maps, and prompt bodies are deliberately absent: a diagnostics list must
/// never become a path for a plugin secret to reach a log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionDetails {
    /// The DSH `dsh.manifestVersion` when declared.
    pub manifest_version: Option<u32>,
    /// The first DSH bundle patch, when declared.
    pub bundle_patch: Option<PathBuf>,
    /// Ids inserted by the DSH bundle patch(es), in order.
    pub bundle_rows: Vec<String>,
    /// The DSH `dsh.client` declaration.
    pub client: Option<ClientManifest>,
    /// A package icon path that exists inside the package.
    pub icon: Option<PathBuf>,
    /// The DSH `engines.dsh` compatibility range, when declared.
    pub engines: Option<String>,
    /// A declared JavaScript host entry (DSH), whether or not it was executed.
    pub host_entry: Option<PathBuf>,
    /// A declared JavaScript client entry (DSH), whether or not it was executed.
    pub client_entry: Option<PathBuf>,
}

/// The declarative DSH `dsh.client` section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientManifest {
    /// The client platform; the reference ships `web`.
    pub platform: String,
    /// Whether the client module loads immediately.
    pub immediately: bool,
    /// Module ids the client declares it injects.
    pub inject: Vec<String>,
    /// Non-baseline runtime imports the client declares.
    pub external: Vec<String>,
}

/// A contribution an extension delivers.
#[derive(Clone)]
pub enum Contribution {
    /// A model-facing tool.
    Tool(Arc<dyn Tool>),
    /// A plugin hook.
    Hook(HookContribution),
    /// A host command.
    Command(PluginCommand),
    /// One skill.
    Skill(SkillContribution),
    /// A directory of skills.
    SkillDir(SkillDirContribution),
    /// A UI panel.
    Panel(UiPanel),
    /// A prompt template.
    Prompt(PromptContribution),
}

impl Contribution {
    /// The contribution kind.
    pub fn kind(&self) -> ContributionKind {
        match self {
            Contribution::Tool(_) => ContributionKind::Tool,
            Contribution::Hook(_) => ContributionKind::Hook,
            Contribution::Command(_) => ContributionKind::Command,
            Contribution::Skill(_) => ContributionKind::Skill,
            Contribution::SkillDir(_) => ContributionKind::SkillDir,
            Contribution::Panel(_) => ContributionKind::Panel,
            Contribution::Prompt(_) => ContributionKind::Prompt,
        }
    }

    /// The name the registry uniqueness check uses.
    pub fn name(&self) -> String {
        match self {
            Contribution::Tool(tool) => tool.name().to_string(),
            Contribution::Hook(hook) => hook.plugin.name().to_string(),
            Contribution::Command(command) => command.name.clone(),
            Contribution::Skill(skill) => skill.name.clone(),
            Contribution::SkillDir(dir) => dir.path.display().to_string(),
            Contribution::Panel(panel) => panel.id.clone(),
            Contribution::Prompt(prompt) => prompt.name.clone(),
        }
    }
}

impl fmt::Debug for Contribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(self.kind().as_str())
            .field(&self.name())
            .finish()
    }
}

/// A hook contribution: the existing [`AgentPlugin`] seam plus the stages it
/// declares.
#[derive(Clone)]
pub struct HookContribution {
    /// The plugin whose overrides run at the declared stages.
    pub plugin: Arc<dyn AgentPlugin>,
    /// The stages the package declares. The plugin itself decides which
    /// overrides it implements; this list is the host-visible declaration.
    pub stages: Vec<HookStage>,
}

impl HookContribution {
    /// Create a hook contribution.
    pub fn new(plugin: Arc<dyn AgentPlugin>, stages: Vec<HookStage>) -> Self {
        Self { plugin, stages }
    }
}

impl fmt::Debug for HookContribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookContribution")
            .field("plugin", &self.plugin.name())
            .field("stages", &self.stages)
            .finish()
    }
}

/// A hook stage, expressed with the existing stage enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookStage {
    /// An LLM request/response stage.
    Llm(LlmStage),
    /// A tool call/result stage.
    Tool(ToolStage),
    /// Per-token stream rewriting.
    StreamToken,
}

/// One skill contributed by an extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillContribution {
    /// The skill name from the `SKILL.md` frontmatter.
    pub name: String,
    /// The skill description from the frontmatter.
    pub description: String,
    /// The `SKILL.md` manifest path.
    pub manifest: PathBuf,
}

/// A directory of skills contributed by an extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDirContribution {
    /// The directory the skill loader walks.
    pub path: PathBuf,
}

/// A prompt template contributed by an extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContribution {
    /// The prompt name.
    pub name: String,
    /// The prompt description.
    pub description: String,
    /// The prompt body, with frontmatter removed.
    pub body: String,
    /// The file the prompt was read from.
    pub source: PathBuf,
    /// The argument placeholder the body uses, when it declares one.
    pub arguments_hint: Option<String>,
}

/// Why a contribution was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefusedFeature {
    /// A DSH host entry (`apply(ctx, config)` in JavaScript).
    ImperativeHostEntry,
    /// A DSH client entry (browser JavaScript).
    ImperativeClientEntry,
    /// An MCP server that needs an external process.
    McpServer,
    /// An external app connector.
    ExternalApp,
    /// A DSH bundle patch that was parsed but not mounted.
    BundleMount,
    /// A contribution kind this shim has no declarative carrier for.
    UnsupportedContribution,
}

impl fmt::Display for RefusedFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RefusedFeature::ImperativeHostEntry => "imperative host entry",
            RefusedFeature::ImperativeClientEntry => "imperative client entry",
            RefusedFeature::McpServer => "MCP server",
            RefusedFeature::ExternalApp => "external app connector",
            RefusedFeature::BundleMount => "bundle patch",
            RefusedFeature::UnsupportedContribution => "unsupported contribution",
        })
    }
}

/// An explicit, localized refusal: which extension, which feature, and why.
///
/// The reason never carries plugin configuration values, environment maps, or
/// file contents. It names the package, the feature, and the path the host can
/// show the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The extension id the refusal belongs to.
    pub extension: String,
    /// The refused feature.
    pub feature: RefusedFeature,
    /// The file or directory the feature was declared in.
    pub path: Option<PathBuf>,
    /// Why the feature was not loaded.
    pub reason: String,
}

impl Refusal {
    /// Create a refusal.
    pub fn new(
        extension: impl Into<String>,
        feature: RefusedFeature,
        path: Option<PathBuf>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            extension: extension.into(),
            feature,
            path,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "extension '{}' refused {}", self.extension, self.feature)?;
        if let Some(path) = &self.path {
            write!(f, " at {}", path.display())?;
        }
        write!(f, ": {}", self.reason)
    }
}

impl std::error::Error for Refusal {}

/// Errors raised while detecting, parsing, registering, or installing an
/// extension.
#[derive(Debug, Error)]
pub enum ExtensionError {
    /// A file could not be read.
    #[error("I/O error reading {path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A manifest was present but could not be validated.
    #[error("{standard} package at {path} is invalid: {message}")]
    Manifest {
        /// The standard being read.
        standard: ExtensionStandard,
        /// The manifest or offending file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// Neither standard marker was found.
    #[error(
        "no supported extension manifest at {path}: expected a DeepSeek Harness package.json with a `dsh` field or a Codex .codex-plugin/plugin.json"
    )]
    UnknownStandard {
        /// The directory that was inspected.
        path: PathBuf,
    },
    /// Both standard markers were found.
    #[error(
        "ambiguous extension manifest at {path}: both a `dsh` package.json and .codex-plugin/plugin.json are present"
    )]
    AmbiguousStandard {
        /// The directory that was inspected.
        path: PathBuf,
    },
    /// The caller asked for one standard and found the other.
    #[error("extension at {path} is a {found} package, not {expected}")]
    WrongStandard {
        /// The directory that was inspected.
        path: PathBuf,
        /// The standard the caller asked for.
        expected: ExtensionStandard,
        /// The standard that was detected.
        found: ExtensionStandard,
    },
    /// A feature was explicitly refused.
    #[error("{0}")]
    Refused(#[from] Refusal),
    /// An extension with this id is already registered.
    #[error("extension '{id}' is already registered")]
    DuplicateExtension {
        /// The duplicated id.
        id: String,
    },
    /// No extension with this id is registered.
    #[error("extension '{id}' is not registered")]
    UnknownExtension {
        /// The missing id.
        id: String,
    },
    /// A contribution name is already claimed by another extension.
    #[error(
        "duplicate {kind} '{name}' from extension '{incoming}': already contributed by '{existing}'"
    )]
    DuplicateContribution {
        /// The duplicated kind.
        kind: ContributionKind,
        /// The duplicated name.
        name: String,
        /// The existing owner.
        existing: String,
        /// The incoming owner.
        incoming: String,
    },
    /// A tool conflicts with a tool already on the agent.
    #[error(
        "tool '{name}' from extension '{extension}' conflicts with a tool already on the agent"
    )]
    ToolConflict {
        /// The conflicting tool name.
        name: String,
        /// The extension that contributed it.
        extension: String,
    },
    /// A tool could not be registered.
    #[error("tool '{name}' from extension '{extension}' could not be installed: {message}")]
    Tool {
        /// The tool name.
        name: String,
        /// The contributing extension.
        extension: String,
        /// The registration error.
        message: String,
    },
    /// A skill could not be registered.
    #[error("skill registration for extension '{extension}' failed: {message}")]
    Skill {
        /// The contributing extension.
        extension: String,
        /// The registration error.
        message: String,
    },
    /// The host plugin registry rejected a command or panel.
    #[error(transparent)]
    Plugin(#[from] crate::plugins::PluginError),
}

pub(crate) fn package_json_has_dsh(root: &Path) -> Result<bool, ExtensionError> {
    let path = root.join("package.json");
    if !path.is_file() {
        return Ok(false);
    }
    let value = read_json(&path, ExtensionStandard::DeepSeekHarness)?;
    Ok(value
        .get("dsh")
        .map(serde_json::Value::is_object)
        .unwrap_or(false))
}

pub(crate) fn read_text(path: &Path) -> Result<String, ExtensionError> {
    std::fs::read_to_string(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn read_json(
    path: &Path,
    standard: ExtensionStandard,
) -> Result<serde_json::Value, ExtensionError> {
    let text = read_text(path)?;
    serde_json::from_str(&text).map_err(|error| ExtensionError::Manifest {
        standard,
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

pub(crate) fn manifest_error(
    standard: ExtensionStandard,
    path: &Path,
    message: impl Into<String>,
) -> ExtensionError {
    ExtensionError::Manifest {
        standard,
        path: path.to_path_buf(),
        message: message.into(),
    }
}

pub(crate) fn manifest_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    standard: ExtensionStandard,
    path: &Path,
) -> Result<String, ExtensionError> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            manifest_error(
                standard,
                path,
                format!("`{key}` must be a string in the package manifest"),
            )
        })
}

/// Read at most `max` bytes of a file, decoded lossily.
///
/// Used only to look for registration API names in JavaScript entry files; the
/// content is never logged or stored.
pub(crate) fn read_prefix(path: &Path, max: usize) -> Result<String, ExtensionError> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buffer = Vec::new();
    file.take(max as u64)
        .read_to_end(&mut buffer)
        .map_err(|source| ExtensionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// Resolve a manifest-declared relative path inside the package.
///
/// Absolute paths and `..` components are refused. When the target exists its
/// canonical form must stay under the canonical package root, so a symlink
/// cannot smuggle a file in from outside.
pub(crate) fn resolve_package_path(
    root: &Path,
    raw: &str,
    standard: ExtensionStandard,
    manifest: &Path,
) -> Result<PathBuf, ExtensionError> {
    let relative = Path::new(raw);
    if relative.is_absolute() {
        return Err(manifest_error(
            standard,
            manifest,
            format!("path '{raw}' must be relative to the package"),
        ));
    }
    if relative
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(manifest_error(
            standard,
            manifest,
            format!("path '{raw}' must not leave the package"),
        ));
    }
    let joined = root.join(relative);
    if !joined.exists() {
        return Ok(joined);
    }
    let canonical_root = root.canonicalize().map_err(|source| ExtensionError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let canonical = joined.canonicalize().map_err(|source| ExtensionError::Io {
        path: joined.clone(),
        source,
    })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(manifest_error(
            standard,
            manifest,
            format!("path '{raw}' leaves the package"),
        ));
    }
    Ok(canonical)
}

pub(crate) fn dedup_capabilities(capabilities: BTreeSet<Capability>) -> Vec<Capability> {
    capabilities.into_iter().collect()
}
