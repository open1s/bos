//! Capability vocabulary and the host policy that turns a declaration into a
//! decision.
//!
//! A manifest declaring a capability is a REQUEST. The only grant is either a
//! default rule for a trust level or an explicit record in
//! `extension-state/grants.toml`. Every tool call that needs a sensitive
//! capability asks this module first, and every answer is written to the audit
//! log, so "the extension said it needed this" and "the host allowed this" are
//! two different facts that can always be compared.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Outbound requests one extension may authorize in a one-second window.
const MAX_NETWORK_CALLS_PER_SECOND: usize = 20;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::types::{ExtensionError, TrustLevel};

/// The current time as Unix seconds, saturating at the epoch on a clock that
/// cannot answer.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// A capability an extension can request.
///
/// The strings are the manifest vocabulary and the audit vocabulary; adding a
/// variant is a change to the public extension standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// Read files inside the workspace.
    WorkspaceRead,
    /// Write files inside the workspace.
    WorkspaceWrite,
    /// Make outbound HTTP requests.
    NetworkHttp,
    /// Start an external process.
    ProcessExecute,
    /// Read the screen or drive keyboard and pointer input.
    ComputerUse,
    /// Read a named secret from the vault.
    SecretsRead,
    /// Use the extension's own storage namespace.
    ExtensionStorage,
    /// Register tools with the agent.
    AgentToolsRegister,
    /// Supply context to the agent.
    AgentContextProvide,
    /// Supply prompt fragments to the agent.
    AgentPromptsProvide,
    /// Supply SKILL.md skills to the agent.
    AgentSkillsProvide,
    /// Register model or provider adapters.
    AgentModelsProvide,
    /// Register agent roles or execution strategies.
    AgentAgentsProvide,
    /// Register workflows.
    AgentWorkflowsProvide,
    /// Publish events on the host bus.
    EventsPublish,
    /// Subscribe to events on the host bus.
    EventsSubscribe,
    /// Contribute host-managed panels.
    UiPanels,
    /// Contribute host commands.
    UiCommands,
    /// Contribute visual theme resources.
    UiTheme,
    /// Receive observe-hook batches from host seams.
    HooksObserve,
    /// Register an event interceptor that may rewrite or short-circuit a host seam.
    AgentIntercept,
}

impl Capability {
    /// Every capability, in declaration order.
    pub const ALL: [Capability; 21] = [
        Self::WorkspaceRead,
        Self::WorkspaceWrite,
        Self::NetworkHttp,
        Self::ProcessExecute,
        Self::ComputerUse,
        Self::SecretsRead,
        Self::ExtensionStorage,
        Self::AgentToolsRegister,
        Self::AgentContextProvide,
        Self::AgentPromptsProvide,
        Self::AgentSkillsProvide,
        Self::AgentModelsProvide,
        Self::AgentAgentsProvide,
        Self::AgentWorkflowsProvide,
        Self::EventsPublish,
        Self::EventsSubscribe,
        Self::UiPanels,
        Self::UiCommands,
        Self::UiTheme,
        Self::HooksObserve,
        Self::AgentIntercept,
    ];

    /// The stable manifest string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceRead => "workspace.read",
            Self::WorkspaceWrite => "workspace.write",
            Self::NetworkHttp => "network.http",
            Self::ProcessExecute => "process.execute",
            Self::ComputerUse => "computer.use",
            Self::SecretsRead => "secrets.read",
            Self::ExtensionStorage => "extension.storage",
            Self::AgentToolsRegister => "agent.tools.register",
            Self::AgentContextProvide => "agent.context.provide",
            Self::AgentPromptsProvide => "agent.prompts.provide",
            Self::AgentSkillsProvide => "agent.skills.provide",
            Self::AgentModelsProvide => "agent.models.provide",
            Self::AgentAgentsProvide => "agent.agents.provide",
            Self::AgentWorkflowsProvide => "agent.workflows.provide",
            Self::EventsPublish => "events.publish",
            Self::EventsSubscribe => "events.subscribe",
            Self::UiPanels => "ui.panels",
            Self::UiCommands => "ui.commands",
            Self::UiTheme => "ui.theme",
            Self::HooksObserve => "hooks.observe",
            Self::AgentIntercept => "agent.intercept",
        }
    }

    /// A one-sentence description for the settings row and `bsh ext info`.
    pub fn description(self) -> &'static str {
        match self {
            Self::WorkspaceRead => "read files inside the current workspace",
            Self::WorkspaceWrite => "write, create or remove files inside the current workspace",
            Self::NetworkHttp => "make outbound HTTP requests",
            Self::ProcessExecute => "start and manage an external process",
            Self::ComputerUse => "read the screen or control the keyboard and pointer",
            Self::SecretsRead => "read a named secret from the vault",
            Self::ExtensionStorage => "use this extension's private storage namespace",
            Self::AgentToolsRegister => "register tools the agent can call",
            Self::AgentContextProvide => "supply scoped context to the agent",
            Self::AgentPromptsProvide => "supply prompt fragments to the agent",
            Self::AgentSkillsProvide => "supply SKILL.md skills to the agent",
            Self::AgentModelsProvide => "register model or provider adapters",
            Self::AgentAgentsProvide => "register agent roles or execution strategies",
            Self::AgentWorkflowsProvide => "register reusable workflows",
            Self::EventsPublish => "publish events on the host bus",
            Self::EventsSubscribe => "subscribe to events on the host bus",
            Self::UiPanels => "contribute host-managed panels",
            Self::UiCommands => "contribute host commands and keybindings",
            Self::UiTheme => "contribute visual theme resources",
            Self::HooksObserve => "receive observe-hook batches from host seams",
            Self::AgentIntercept => "intercept and rewrite host events and requests",
        }
    }

    /// The risk bucket a default policy rule is written against.
    pub fn risk(self) -> Risk {
        match self {
            Self::WorkspaceRead
            | Self::ExtensionStorage
            | Self::AgentToolsRegister
            | Self::AgentContextProvide
            | Self::AgentPromptsProvide
            | Self::EventsPublish
            | Self::EventsSubscribe
            | Self::UiPanels
            | Self::UiCommands
            | Self::UiTheme => Risk::Low,
            Self::AgentModelsProvide
            | Self::AgentAgentsProvide
            | Self::AgentWorkflowsProvide
            | Self::AgentSkillsProvide => Risk::Medium,
            Self::WorkspaceWrite
            | Self::NetworkHttp
            | Self::HooksObserve
            | Self::AgentIntercept => Risk::High,
            Self::ProcessExecute | Self::ComputerUse | Self::SecretsRead => Risk::Critical,
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = ExtensionError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw == "terminal.execute" {
            return Ok(Capability::ProcessExecute);
        }
        Capability::ALL
            .iter()
            .copied()
            .find(|capability| capability.as_str() == raw)
            .ok_or_else(|| ExtensionError::InvalidId {
                id: raw.to_string(),
                reason: "unknown capability; run 'bsh ext capabilities' for the vocabulary"
                    .to_string(),
            })
    }
}

impl Serialize for Capability {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Capability {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// The coarse risk bucket used by default policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// Registration and read-only metadata.
    Low,
    /// Can influence the model or another extension but cannot touch the
    /// machine.
    Medium,
    /// Can modify workspace data or reach the network.
    High,
    /// Can run code, read secrets or drive the machine.
    Critical,
}

impl Risk {
    /// A label for diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// The scope a grant is limited to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantScope {
    /// No restriction.
    Any,
    /// The workspace that was open when the call was made.
    Workspace,
    /// A path prefix.
    Path(PathBuf),
    /// One DNS name and its subdomains.
    Domain(String),
    /// One DNS name (and its subdomains) restricted to one HTTP method.
    DomainMethod(String, String),
    /// One executable, by the name the manifest declares.
    Program(String),
    /// One executable and the exact argument vector.
    ProgramArgs(String, Vec<String>),
    /// One named secret.
    Secret(String),
}

impl fmt::Display for GrantScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => formatter.write_str("any"),
            Self::Workspace => formatter.write_str("workspace"),
            Self::Path(path) => write!(formatter, "path:{}", path.display()),
            Self::Domain(domain) => write!(formatter, "domain:{domain}"),
            Self::DomainMethod(domain, method) => write!(formatter, "domain:{domain}:{method}"),
            Self::Program(program) => write!(formatter, "program:{program}"),
            Self::ProgramArgs(program, args) => {
                // A comma inside one argument would be re-split on reload and
                // silently change what the grant allows; the JSON form is
                // unambiguous and the parser still reads the legacy join.
                if args
                    .iter()
                    .any(|argument| argument.contains(',') || argument.starts_with('['))
                {
                    write!(
                        formatter,
                        "program:{program}:{}",
                        serde_json::to_string(args).unwrap_or_default()
                    )
                } else {
                    write!(formatter, "program:{program}:{}", args.join(","))
                }
            }
            Self::Secret(secret) => write!(formatter, "secret:{secret}"),
        }
    }
}

impl FromStr for GrantScope {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw == "any" {
            return Ok(Self::Any);
        }
        if raw == "workspace" {
            return Ok(Self::Workspace);
        }
        if let Some(path) = raw.strip_prefix("path:") {
            if path.trim().is_empty() {
                return Err("path scope needs a path".to_string());
            }
            return Ok(Self::Path(normalize_scope_path(Path::new(path))));
        }
        if let Some(domain) = raw.strip_prefix("domain:") {
            if domain.trim().is_empty() {
                return Err("domain scope needs a host".to_string());
            }
            if let Some((host, method)) = domain.split_once(':') {
                if host.trim().is_empty() || method.trim().is_empty() {
                    return Err("method scope needs a host and a method".to_string());
                }
                return Ok(Self::DomainMethod(
                    host.to_string(),
                    method.to_ascii_uppercase(),
                ));
            }
            return Ok(Self::Domain(domain.to_string()));
        }
        if let Some(program) = raw.strip_prefix("program:") {
            if program.trim().is_empty() {
                return Err("program scope needs an executable".to_string());
            }
            if let Some((program, args)) = program.split_once(':') {
                if program.trim().is_empty() {
                    return Err("program scope needs an executable".to_string());
                }
                let args = if args.trim_start().starts_with('[') {
                    serde_json::from_str::<Vec<String>>(args)
                        .map_err(|error| format!("program argument JSON is invalid: {error}"))?
                } else {
                    args.split(',')
                        .filter(|arg| !arg.is_empty())
                        .map(str::to_string)
                        .collect()
                };
                return Ok(Self::ProgramArgs(program.to_string(), args));
            }
            return Ok(Self::Program(program.to_string()));
        }
        if let Some(secret) = raw.strip_prefix("secret:") {
            if secret.trim().is_empty() {
                return Err("secret scope needs a name".to_string());
            }
            return Ok(Self::Secret(secret.to_string()));
        }
        Err(format!(
            "unknown scope {raw:?}; use any, workspace, path:..., domain:..., program:... or secret:..."
        ))
    }
}

impl Serialize for GrantScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for GrantScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// One explicit grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// What the grant is limited to.
    #[serde(default = "any_scope")]
    pub scope: GrantScope,
    /// Unix seconds the grant was written.
    #[serde(default)]
    pub granted_at: u64,
    /// Why the reader granted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn any_scope() -> GrantScope {
    GrantScope::Any
}

impl Grant {
    /// Build a grant with the current timestamp.
    pub fn new(scope: GrantScope, note: Option<String>) -> Self {
        Self {
            scope,
            granted_at: unix_now(),
            note,
        }
    }
}

/// The persisted grant document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantFile {
    /// Format generation.
    #[serde(default = "grant_format")]
    pub format: u32,
    /// extension id -> capability -> grant.
    #[serde(default)]
    pub grants: BTreeMap<String, BTreeMap<String, Grant>>,
}

fn grant_format() -> u32 {
    1
}

impl Default for GrantFile {
    fn default() -> Self {
        Self {
            format: grant_format(),
            grants: BTreeMap::new(),
        }
    }
}

/// The grant store, loaded from and written to one TOML document.
pub struct GrantStore {
    path: PathBuf,
    file: GrantFile,
}

impl GrantStore {
    /// Load the store at `path`, creating an empty one when it is absent.
    pub fn load(path: PathBuf) -> Result<Self, ExtensionError> {
        let file = if path.is_file() {
            let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
                path: path.clone(),
                source,
            })?;
            toml::from_str(&text).map_err(|error| ExtensionError::Corrupt {
                path: path.clone(),
                message: error.to_string(),
            })?
        } else {
            GrantFile::default()
        };
        Ok(Self { path, file })
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All grants for one extension.
    pub fn grants_for(&self, extension: &str) -> BTreeMap<String, Grant> {
        self.file.grants.get(extension).cloned().unwrap_or_default()
    }

    /// Look up one grant.
    pub fn get(&self, extension: &str, capability: Capability) -> Option<&Grant> {
        self.file
            .grants
            .get(extension)
            .and_then(|grants| grants.get(capability.as_str()))
    }

    /// Add or replace a grant and persist the document.
    pub fn grant(
        &mut self,
        extension: &str,
        capability: Capability,
        scope: GrantScope,
        note: Option<String>,
    ) -> Result<(), ExtensionError> {
        // Mutate a candidate and persist it first: the in-memory store must
        // never describe a grant the durable file does not contain.
        let mut candidate = self.file.clone();
        candidate
            .grants
            .entry(extension.to_string())
            .or_default()
            .insert(capability.as_str().to_string(), Grant::new(scope, note));
        self.write(&candidate)?;
        self.file = candidate;
        Ok(())
    }

    /// Remove a grant and persist the document. Returns whether it existed.
    pub fn revoke(
        &mut self,
        extension: &str,
        capability: Capability,
    ) -> Result<bool, ExtensionError> {
        let mut candidate = self.file.clone();
        let removed = candidate
            .grants
            .get_mut(extension)
            .map(|grants| grants.remove(capability.as_str()).is_some())
            .unwrap_or(false);
        if removed {
            if candidate
                .grants
                .get(extension)
                .is_some_and(BTreeMap::is_empty)
            {
                candidate.grants.remove(extension);
            }
            self.write(&candidate)?;
            self.file = candidate;
        }
        Ok(removed)
    }

    /// Serialize the store deterministically.
    pub fn to_toml(&self) -> Result<String, ExtensionError> {
        toml::to_string_pretty(&self.file).map_err(|error| ExtensionError::Corrupt {
            path: self.path.clone(),
            message: error.to_string(),
        })
    }

    fn write(&self, file: &GrantFile) -> Result<(), ExtensionError> {
        let text = toml::to_string_pretty(file).map_err(|error| ExtensionError::Corrupt {
            path: self.path.clone(),
            message: error.to_string(),
        })?;
        write_grant_atomic(&self.path, text.as_bytes())
    }
}

/// What a host service is being asked to do, for scope matching.
#[derive(Debug, Clone, Default)]
pub struct CallScope {
    /// The workspace open for this call.
    pub workspace: Option<PathBuf>,
    /// The resource path being touched.
    pub path: Option<PathBuf>,
    /// The DNS host being reached.
    pub domain: Option<String>,
    /// The HTTP method, when the call is a network request.
    pub method: Option<String>,
    /// The declared executable of a process call.
    pub program: Option<String>,
    /// The arguments of a process call.
    pub args: Option<Vec<String>>,
    /// The secret being read.
    pub secret: Option<String>,
}

/// Lexically normalize a path without touching the filesystem.
///
/// A scope is a lease on a prefix, so the comparison has to happen on a path
/// that no `..` component can lengthen: `/ws/sub/../../etc` must not look like
/// it still sits under `/ws/sub`. Canonicalization would also resolve symlinks,
/// which a grant deliberately does not do (a lease names a location, not the
/// inode behind it), so this walks components instead.
pub fn write_grant_atomic(path: &Path, bytes: &[u8]) -> Result<(), ExtensionError> {
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
    let _ = std::fs::remove_file(path);
    std::fs::rename(&temporary, path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn normalize_scope_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let rooted = normalized.has_root();
                if !normalized.pop() && !rooted {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Derive the scope of one tool call from its arguments.
///
/// The standard fixes three argument names the policy reads: `path`, `url` and
/// `secret`. A relative path resolves against the open workspace, and every
/// path is normalized first, so a scoped grant cannot be escaped with `..`.
pub fn call_scope(arguments: &Value, workspace: Option<&Path>) -> CallScope {
    let mut scope = CallScope {
        workspace: workspace.map(Path::to_path_buf),
        path: workspace.map(Path::to_path_buf),
        ..CallScope::default()
    };
    if let Some(raw) = arguments.get("path").and_then(Value::as_str) {
        let path = PathBuf::from(raw);
        let resolved = if path.is_absolute() {
            path
        } else if let Some(workspace) = workspace {
            workspace.join(path)
        } else {
            path
        };
        scope.path = Some(normalize_scope_path(&resolved));
    }
    if let Some(raw) = arguments.get("url").and_then(Value::as_str) {
        // Parse with the transport's own URL parser. Manual `://`/`@`/`:`
        // splitting disagrees with reqwest on backslashes, userinfo and IPv6,
        // so the policy could authorize a different host than the one the
        // request actually reached.
        if let Ok(parsed) = reqwest::Url::parse(raw) {
            if matches!(parsed.scheme(), "http" | "https") {
                if let Some(host) = parsed.host_str() {
                    scope.domain = Some(host.to_ascii_lowercase());
                }
            }
        }
    }
    if let Some(raw) = arguments.get("method").and_then(Value::as_str) {
        scope.method = Some(raw.to_ascii_uppercase());
    }
    scope.secret = arguments
        .get("secret")
        .and_then(Value::as_str)
        .map(str::to_string);
    scope
}

/// Authorize every capability a tool descriptor declares, in order.
///
/// One shared seam for the agent tool wrapper, host-initiated calls (language
/// diagnostics, integrations) and panel actions, so a surface cannot dispatch a
/// tool while skipping the per-call check the standard requires.
pub fn authorize_each(
    policy: &PolicyEngine,
    extension: &str,
    trust: TrustLevel,
    capabilities: &[Capability],
    scope: &CallScope,
) -> Result<(), ExtensionError> {
    for capability in capabilities {
        policy.authorize_call(extension, trust, *capability, scope)?;
    }
    Ok(())
}

/// Whether a DNS host equals a domain or is one of its subdomains.
fn host_matches(host: &str, domain: &str) -> bool {
    host.eq_ignore_ascii_case(domain)
        || host
            .to_ascii_lowercase()
            .ends_with(&format!(".{}", domain.to_ascii_lowercase()))
}

impl GrantScope {
    /// Whether a grant of this scope covers a call.
    pub fn covers(&self, call: &CallScope) -> bool {
        match self {
            Self::Any => true,
            // A workspace grant covers paths inside the active workspace, not
            // any path merely because a workspace happens to be open.
            Self::Workspace => match (&call.path, &call.workspace) {
                (Some(path), Some(workspace)) => {
                    normalize_scope_path(path).starts_with(normalize_scope_path(workspace))
                }
                (None, workspace) => workspace.is_some(),
                (Some(_), None) => false,
            },
            // Compare normalized paths: a lexical prefix check on the raw
            // argument would let `..` walk out of the granted prefix.
            Self::Path(prefix) => {
                let prefix = normalize_scope_path(prefix);
                call.path
                    .as_ref()
                    .or(call.workspace.as_ref())
                    .is_some_and(|path| normalize_scope_path(path).starts_with(&prefix))
            }
            Self::Domain(domain) => call
                .domain
                .as_ref()
                .is_some_and(|host| host_matches(host, domain)),
            Self::DomainMethod(domain, method) => {
                call.domain
                    .as_ref()
                    .is_some_and(|host| host_matches(host, domain))
                    && call
                        .method
                        .as_ref()
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(method))
            }
            Self::Program(program) => call
                .program
                .as_ref()
                .is_some_and(|actual| actual == program),
            Self::ProgramArgs(program, args) => {
                call.program
                    .as_ref()
                    .is_some_and(|actual| actual == program)
                    && call.args.as_ref().is_some_and(|actual| actual == args)
            }
            Self::Secret(secret) => call.secret.as_ref().is_some_and(|name| name == secret),
        }
    }
}

/// The result of one policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Allowed, under this scope.
    Allow {
        /// The scope the call was allowed under.
        scope: GrantScope,
    },
    /// Denied, with a reason the reader can act on.
    Deny {
        /// Why the call was denied.
        reason: String,
    },
}

impl Decision {
    /// Whether the call is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    /// The denial reason, empty when allowed.
    pub fn reason(&self) -> &str {
        match self {
            Self::Allow { .. } => "",
            Self::Deny { reason } => reason,
        }
    }
}

/// Whether the policy is enforced or only recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyMode {
    /// Denials stop the operation.
    Enforce,
    /// Denials are audited but the operation proceeds. Development only.
    Advisory,
}

/// The host policy engine.
pub struct PolicyEngine {
    grants: Mutex<GrantStore>,
    audit: AuditLog,
    mode: PolicyMode,
    network: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// The first audit write that failed, if any. A policy that cannot record
    /// its decisions is degraded, and the host must be able to say so.
    audit_failure: Mutex<Option<String>>,
}

impl PolicyEngine {
    /// Build an engine over a grant file and an audit log.
    pub fn open(
        grants_path: PathBuf,
        audit_path: PathBuf,
        mode: PolicyMode,
    ) -> Result<Self, ExtensionError> {
        Ok(Self {
            grants: Mutex::new(GrantStore::load(grants_path)?),
            audit: AuditLog::new(audit_path),
            mode,
            network: Mutex::new(HashMap::new()),
            audit_failure: Mutex::new(None),
        })
    }

    /// The first audit persistence failure, if any.
    pub fn audit_degraded(&self) -> Option<String> {
        self.audit_failure
            .lock()
            .ok()
            .and_then(|failure| failure.clone())
    }

    /// The grant store path, for diagnostics.
    pub fn grants_path(&self) -> PathBuf {
        self.grants
            .lock()
            .map(|store| store.path().to_path_buf())
            .unwrap_or_default()
    }

    /// All grants for one extension.
    pub fn grants_for(&self, extension: &str) -> BTreeMap<String, Grant> {
        self.grants
            .lock()
            .map(|store| store.grants_for(extension))
            .unwrap_or_default()
    }

    /// Add or replace a grant.
    pub fn grant(
        &self,
        extension: &str,
        capability: Capability,
        scope: GrantScope,
        note: Option<String>,
    ) -> Result<(), ExtensionError> {
        self.grants
            .lock()
            .map_err(|_| ExtensionError::Corrupt {
                path: self.grants_path(),
                message: "grant store lock is poisoned".to_string(),
            })?
            .grant(extension, capability, scope, note)
    }

    /// Revoke a grant. Returns whether it existed.
    pub fn revoke(&self, extension: &str, capability: Capability) -> Result<bool, ExtensionError> {
        self.grants
            .lock()
            .map_err(|_| ExtensionError::Corrupt {
                path: self.grants_path(),
                message: "grant store lock is poisoned".to_string(),
            })?
            .revoke(extension, capability)
    }

    /// Evaluate one capability for one extension.
    pub fn evaluate(
        &self,
        extension: &str,
        trust: TrustLevel,
        capability: Capability,
        scope: &CallScope,
    ) -> Decision {
        let decision = self.decide(extension, trust, capability, scope);
        if let Err(error) = self
            .audit
            .record(extension, capability.as_str(), &decision, trust)
        {
            if let Ok(mut failure) = self.audit_failure.lock() {
                failure.get_or_insert_with(|| error.to_string());
            }
        }
        if self.mode == PolicyMode::Advisory {
            if let Decision::Deny { .. } = decision {
                return Decision::Allow {
                    scope: GrantScope::Any,
                };
            }
        }
        decision
    }

    /// Evaluate without writing an audit entry.
    ///
    /// Status rows ask this question for every declared capability on every
    /// rebuild; recording those as decisions would fill the audit log with rows
    /// for operations that never happened. A real authorization still uses
    /// evaluate().
    pub fn preview(
        &self,
        extension: &str,
        trust: TrustLevel,
        capability: Capability,
        scope: &CallScope,
    ) -> Decision {
        self.decide(extension, trust, capability, scope)
    }

    /// Evaluate every required capability at activation. The first denial is
    /// returned as an error naming the capability and the reason.
    pub fn authorize_required(
        &self,
        extension: &str,
        trust: TrustLevel,
        required: &[Capability],
        scope: &CallScope,
    ) -> Result<(), ExtensionError> {
        for capability in required {
            // A process grant can name the executable or its argument vector;
            // the load-time scope has neither, so the per-spawn check is the
            // one that enforces it. Requiring the grant to exist here keeps a
            // missing grant a load error.
            if *capability == Capability::ProcessExecute
                && scope.program.is_none()
                && self.grants_for(extension).contains_key(capability.as_str())
            {
                continue;
            }
            if let Decision::Deny { reason } = self.evaluate(extension, trust, *capability, scope) {
                return Err(ExtensionError::MissingCapability {
                    extension: extension.to_string(),
                    capability: capability.as_str().to_string(),
                    reason,
                });
            }
        }
        Ok(())
    }

    /// Evaluate one capability for a call, returning the scope it was allowed
    /// under or the denial as an error.
    pub fn authorize_call(
        &self,
        extension: &str,
        trust: TrustLevel,
        capability: Capability,
        scope: &CallScope,
    ) -> Result<GrantScope, ExtensionError> {
        match self.evaluate(extension, trust, capability, scope) {
            Decision::Allow { scope } => {
                if capability == Capability::NetworkHttp && !self.meter_network(extension) {
                    let reason = format!(
                        "more than {MAX_NETWORK_CALLS_PER_SECOND} network calls in one second"
                    );
                    // The policy decision was "allow"; the meter then refused.
                    // Record the final outcome, so the audit cannot claim a
                    // request went out when rate limiting stopped it.
                    let denial = Decision::Deny {
                        reason: reason.clone(),
                    };
                    if let Err(error) =
                        self.audit
                            .record(extension, capability.as_str(), &denial, trust)
                    {
                        if let Ok(mut failure) = self.audit_failure.lock() {
                            failure.get_or_insert_with(|| error.to_string());
                        }
                    }
                    return Err(ExtensionError::Denied {
                        extension: extension.to_string(),
                        capability: capability.as_str().to_string(),
                        reason,
                    });
                }
                Ok(scope)
            }
            Decision::Deny { reason } => Err(ExtensionError::Denied {
                extension: extension.to_string(),
                capability: capability.as_str().to_string(),
                reason,
            }),
        }
    }

    /// A per-extension fixed window for outbound requests.
    fn meter_network(&self, extension: &str) -> bool {
        let Ok(mut calls) = self.network.lock() else {
            return false;
        };
        let now = Instant::now();
        let window = calls.entry(extension.to_string()).or_default();
        while window
            .front()
            .is_some_and(|started| now.duration_since(*started) >= Duration::from_secs(1))
        {
            let _ = window.pop_front();
        }
        if window.len() >= MAX_NETWORK_CALLS_PER_SECOND {
            return false;
        }
        window.push_back(now);
        true
    }

    fn decide(
        &self,
        extension: &str,
        trust: TrustLevel,
        capability: Capability,
        scope: &CallScope,
    ) -> Decision {
        let store = match self.grants.lock() {
            Ok(store) => store,
            // A grant store we cannot read is not "no grant": falling through
            // to the trust defaults would silently widen a call after a panic
            // in another thread.
            Err(_) => {
                return Decision::Deny {
                    reason: "grant store is unavailable (lock poisoned); refusing a                              trust-default decision"
                        .to_string(),
                };
            }
        };
        if let Some(grant) = store.get(extension, capability) {
            if grant.scope.covers(scope) {
                return Decision::Allow {
                    scope: grant.scope.clone(),
                };
            }
            return Decision::Deny {
                reason: format!(
                    "grant is scoped to {}, which does not cover this call",
                    grant.scope
                ),
            };
        }
        drop(store);
        match trust {
            TrustLevel::Builtin => Decision::Allow {
                scope: GrantScope::Any,
            },
            TrustLevel::Verified if capability.risk() <= Risk::Medium => Decision::Allow {
                scope: default_scope(capability, scope),
            },
            // The standard's table allows agent.skills.provide at every trust
            // level even though it is a medium-risk capability; it is the one
            // documented medium exception to the low-risk default.
            TrustLevel::Community | TrustLevel::Local | TrustLevel::Unknown
                if capability.risk() == Risk::Low
                    || capability == Capability::AgentSkillsProvide =>
            {
                Decision::Allow {
                    scope: default_scope(capability, scope),
                }
            }
            _ => Decision::Deny {
                reason: format!(
                    "{} requires an explicit grant for a {} extension (run 'bsh ext grant {} {}')",
                    capability,
                    trust.as_str(),
                    extension,
                    capability
                ),
            },
        }
    }
}

/// The scope a default allow is limited to.
fn default_scope(capability: Capability, call: &CallScope) -> GrantScope {
    match capability {
        Capability::WorkspaceRead | Capability::WorkspaceWrite => match &call.workspace {
            Some(workspace) => GrantScope::Path(workspace.clone()),
            None => GrantScope::Workspace,
        },
        Capability::NetworkHttp => match &call.domain {
            Some(domain) => GrantScope::Domain(domain.clone()),
            None => GrantScope::Any,
        },
        Capability::SecretsRead => match &call.secret {
            Some(secret) => GrantScope::Secret(secret.clone()),
            None => GrantScope::Any,
        },
        _ => GrantScope::Any,
    }
}

/// Append-only JSON-lines audit log with one rotation.
pub struct AuditLog {
    path: PathBuf,
    max_bytes: u64,
}

impl AuditLog {
    /// Build a log at `path` that rotates at 1 MiB.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            max_bytes: 1024 * 1024,
        }
    }

    /// The log path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn record(
        &self,
        extension: &str,
        capability: &str,
        decision: &Decision,
        trust: TrustLevel,
    ) -> Result<(), ExtensionError> {
        let (allowed, scope, reason) = match decision {
            Decision::Allow { scope } => (true, Some(scope.to_string()), None),
            Decision::Deny { reason } => (false, None, Some(reason.as_str())),
        };
        let line = serde_json::json!({
            "timestamp": unix_now(),
            "extension": extension,
            "capability": capability,
            "allowed": allowed,
            "scope": scope,
            "trust": trust.as_str(),
            "reason": reason,
        });
        self.rotate_if_needed();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ExtensionError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| ExtensionError::Io {
                path: self.path.clone(),
                source,
            })?;
        use std::io::Write;
        writeln!(file, "{line}").map_err(|source| ExtensionError::Io {
            path: self.path.clone(),
            source,
        })
    }

    fn rotate_if_needed(&self) {
        let too_large = std::fs::metadata(&self.path)
            .map(|metadata| metadata.len() >= self.max_bytes)
            .unwrap_or(false);
        if too_large {
            let rotated = self.path.with_extension("jsonl.1");
            let _ = std::fs::rename(&self.path, rotated);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let id = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bsh-ext-policy-{label}-{}-{id}",
                std::process::id()
            ));
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

    fn engine(scratch: &Scratch) -> PolicyEngine {
        PolicyEngine::open(
            scratch.0.join("grants.toml"),
            scratch.0.join("audit.jsonl"),
            PolicyMode::Enforce,
        )
        .expect("policy engine")
    }

    #[test]
    fn network_calls_are_rate_limited_per_extension() {
        let scratch = Scratch::new("network-rate");
        let policy = engine(&scratch);
        policy
            .grant(
                "com.example.flood",
                Capability::NetworkHttp,
                GrantScope::Any,
                None,
            )
            .expect("grant");
        let scope = CallScope {
            domain: Some("example.com".to_string()),
            method: Some("GET".to_string()),
            ..CallScope::default()
        };
        for _ in 0..MAX_NETWORK_CALLS_PER_SECOND {
            policy
                .authorize_call(
                    "com.example.flood",
                    TrustLevel::Community,
                    Capability::NetworkHttp,
                    &scope,
                )
                .expect("within the window");
        }
        let error = policy
            .authorize_call(
                "com.example.flood",
                TrustLevel::Community,
                Capability::NetworkHttp,
                &scope,
            )
            .expect_err("over the window");
        assert!(error.to_string().contains("network calls"), "{error}");
    }
    #[test]
    fn a_domain_scope_can_restrict_the_http_method() {
        let scope: GrantScope = "domain:api.example.com:POST".parse().expect("scope");
        assert_eq!(
            scope,
            GrantScope::DomainMethod("api.example.com".to_string(), "POST".to_string())
        );
        assert!(scope.covers(&CallScope {
            domain: Some("api.example.com".to_string()),
            method: Some("POST".to_string()),
            ..CallScope::default()
        }));
        assert!(!scope.covers(&CallScope {
            domain: Some("api.example.com".to_string()),
            method: Some("GET".to_string()),
            ..CallScope::default()
        }));
        assert_eq!(scope.to_string(), "domain:api.example.com:POST");
    }

    #[test]
    fn a_process_scope_can_restrict_the_executable_and_arguments() {
        let scope: GrantScope = "program:python3:--version".parse().expect("scope");
        assert_eq!(
            scope,
            GrantScope::ProgramArgs("python3".to_string(), vec!["--version".to_string()])
        );
        assert!(scope.covers(&CallScope {
            program: Some("python3".to_string()),
            args: Some(vec!["--version".to_string()]),
            ..CallScope::default()
        }));
        assert!(!scope.covers(&CallScope {
            program: Some("python3".to_string()),
            args: Some(vec!["-c".to_string()]),
            ..CallScope::default()
        }));
        let program: GrantScope = "program:sh".parse().expect("scope");
        assert_eq!(program, GrantScope::Program("sh".to_string()));
    }

    #[test]
    fn a_parent_directory_cannot_escape_a_path_grant() {
        let grant: GrantScope = "path:/ws/sub".parse().expect("scope");
        let inside = call_scope(
            &serde_json::json!({"path": "notes/readme.md"}),
            Some(Path::new("/ws/sub")),
        );
        assert!(grant.covers(&inside));
        let outside = call_scope(
            &serde_json::json!({"path": "../outside.txt"}),
            Some(Path::new("/ws/sub")),
        );
        assert!(
            !grant.covers(&outside),
            "{}",
            outside.path.expect("path").display()
        );
        let absolute = call_scope(
            &serde_json::json!({"path": "/etc/passwd"}),
            Some(Path::new("/ws/sub")),
        );
        assert!(!grant.covers(&absolute));
    }

    #[test]
    fn a_program_argument_with_a_comma_round_trips() {
        let scope =
            GrantScope::ProgramArgs("python3".to_string(), vec!["--exclude=a,b".to_string()]);
        let parsed: GrantScope = scope.to_string().parse().expect("scope");
        assert_eq!(parsed, scope);
    }

    #[test]
    fn community_packages_may_use_skills() {
        let scratch = Scratch::new("skills");
        let policy = engine(&scratch);
        // §6's table allows agent.skills.provide at every trust level.
        assert!(policy
            .evaluate(
                "com.example.skills",
                TrustLevel::Community,
                Capability::AgentSkillsProvide,
                &CallScope::default(),
            )
            .is_allowed());
        // The other medium capabilities still need a grant at community trust.
        assert!(!policy
            .evaluate(
                "com.example.skills",
                TrustLevel::Community,
                Capability::AgentModelsProvide,
                &CallScope::default(),
            )
            .is_allowed());
    }

    #[test]
    fn every_declared_capability_is_checked() {
        let scratch = Scratch::new("each");
        let policy = engine(&scratch);
        let error = authorize_each(
            &policy,
            "com.example.tool",
            TrustLevel::Local,
            &[Capability::WorkspaceRead, Capability::WorkspaceWrite],
            &CallScope::default(),
        )
        .expect_err("the write is denied");
        assert!(matches!(error, ExtensionError::Denied { .. }), "{error}");
    }

    #[test]
    fn capability_strings_round_trip() {
        for capability in Capability::ALL {
            let parsed: Capability = capability.as_str().parse().expect("capability");
            assert_eq!(parsed, capability);
        }
        assert!("workspace.teleport".parse::<Capability>().is_err());
        assert_eq!(
            "terminal.execute".parse::<Capability>().expect("alias"),
            Capability::ProcessExecute
        );
    }

    #[test]
    fn a_local_package_cannot_write_without_a_grant() {
        let scratch = Scratch::new("deny");
        let policy = engine(&scratch);
        let decision = policy.evaluate(
            "com.example.tool",
            TrustLevel::Local,
            Capability::WorkspaceWrite,
            &CallScope::default(),
        );
        assert!(!decision.is_allowed());
        assert!(decision.reason().contains("explicit grant"));

        policy
            .grant(
                "com.example.tool",
                Capability::WorkspaceWrite,
                GrantScope::Workspace,
                Some("audited".to_string()),
            )
            .expect("grant");
        let decision = policy.evaluate(
            "com.example.tool",
            TrustLevel::Local,
            Capability::WorkspaceWrite,
            &CallScope {
                workspace: Some(PathBuf::from("/tmp/ws")),
                ..CallScope::default()
            },
        );
        assert!(decision.is_allowed());
    }

    #[test]
    fn a_scoped_grant_does_not_cover_another_path() {
        let scratch = Scratch::new("scope");
        let policy = engine(&scratch);
        policy
            .grant(
                "com.example.tool",
                Capability::WorkspaceWrite,
                GrantScope::Path(PathBuf::from("/tmp/allowed")),
                None,
            )
            .expect("grant");
        let denied = policy.evaluate(
            "com.example.tool",
            TrustLevel::Local,
            Capability::WorkspaceWrite,
            &CallScope {
                workspace: Some(PathBuf::from("/tmp/other")),
                path: Some(PathBuf::from("/tmp/other/file")),
                ..CallScope::default()
            },
        );
        assert!(!denied.is_allowed());
        let allowed = policy.evaluate(
            "com.example.tool",
            TrustLevel::Local,
            Capability::WorkspaceWrite,
            &CallScope {
                workspace: Some(PathBuf::from("/tmp/allowed")),
                path: Some(PathBuf::from("/tmp/allowed/file")),
                ..CallScope::default()
            },
        );
        assert!(allowed.is_allowed());
    }

    #[test]
    fn builtins_are_allowed_by_default_and_denials_are_audited() {
        let scratch = Scratch::new("audit");
        let policy = engine(&scratch);
        assert!(policy
            .evaluate(
                "com.bos.bsh.builtin",
                TrustLevel::Builtin,
                Capability::ProcessExecute,
                &CallScope::default(),
            )
            .is_allowed());
        let _ = policy.evaluate(
            "com.example.no",
            TrustLevel::Community,
            Capability::ProcessExecute,
            &CallScope::default(),
        );
        let text = std::fs::read_to_string(scratch.0.join("audit.jsonl")).expect("audit file");
        assert!(text.contains("process.execute"), "{text}");
        assert!(text.contains("\"allowed\":false"), "{text}");
    }

    #[test]
    fn grants_persist() {
        let scratch = Scratch::new("persist");
        {
            let policy = engine(&scratch);
            policy
                .grant(
                    "com.example.tool",
                    Capability::NetworkHttp,
                    GrantScope::Domain("example.com".to_string()),
                    Some("needed for the API".to_string()),
                )
                .expect("grant");
        }
        let policy = engine(&scratch);
        let grant = policy
            .grants_for("com.example.tool")
            .get("network.http")
            .cloned()
            .expect("persisted grant");
        assert_eq!(grant.scope, GrantScope::Domain("example.com".to_string()));
        assert!(policy
            .evaluate(
                "com.example.tool",
                TrustLevel::Community,
                Capability::NetworkHttp,
                &CallScope {
                    domain: Some("api.example.com".to_string()),
                    ..CallScope::default()
                },
            )
            .is_allowed());
    }

    #[test]
    fn a_poisoned_grant_store_denies_instead_of_trusting() {
        let scratch = Scratch::new("poisoned-grants");
        let policy = Arc::new(engine(&scratch));
        let poison = Arc::clone(&policy);
        let _ = std::thread::spawn(move || {
            let _guard = poison.grants.lock().expect("grants");
            panic!("poison the grant store");
        })
        .join();
        let decision = policy.decide(
            "com.example.builtin",
            TrustLevel::Builtin,
            Capability::WorkspaceRead,
            &CallScope::default(),
        );
        assert!(
            matches!(decision, Decision::Deny { .. }),
            "a poisoned grant store must not fall through to trust defaults: {decision:?}"
        );
    }

    #[test]
    fn a_failed_grant_persist_does_not_change_the_in_memory_store() {
        let scratch = Scratch::new("grant-persist");
        let path = scratch.0.join("grants.toml");
        std::fs::create_dir_all(&path).expect("a directory where the grant file goes");
        let policy = PolicyEngine::open(
            path.clone(),
            scratch.0.join("audit.jsonl"),
            PolicyMode::Enforce,
        )
        .expect("policy");
        assert!(policy
            .grant(
                "com.example",
                Capability::WorkspaceRead,
                GrantScope::Any,
                None,
            )
            .is_err());
        assert!(
            policy.grants_for("com.example").is_empty(),
            "a grant that could not be written must not be applied in memory"
        );
        assert!(policy.audit_degraded().is_none());
    }

    #[test]
    fn a_failed_revoke_persist_keeps_the_grant() {
        let scratch = Scratch::new("revoke-persist");
        let path = scratch.0.join("grants.toml");
        let policy = engine(&scratch);
        policy
            .grant(
                "com.example",
                Capability::WorkspaceRead,
                GrantScope::Any,
                None,
            )
            .expect("grant");
        std::fs::remove_file(&path).expect("remove grant file");
        std::fs::create_dir_all(&path).expect("a directory where the grant file was");
        assert!(policy
            .revoke("com.example", Capability::WorkspaceRead)
            .is_err());
        assert!(
            policy
                .grants_for("com.example")
                .contains_key("workspace.read"),
            "a revoke that could not be written must not remove the in-memory grant"
        );
    }

    #[test]
    fn a_workspace_grant_covers_only_paths_inside_the_workspace() {
        let scope = GrantScope::Workspace;
        let workspace = PathBuf::from("/work/project");
        let call = |path: Option<&str>| CallScope {
            workspace: Some(workspace.clone()),
            path: path.map(PathBuf::from),
            ..CallScope::default()
        };
        assert!(scope.covers(&call(Some("/work/project/src/main.rs"))));
        assert!(!scope.covers(&call(Some("/etc/passwd"))));
        assert!(!scope.covers(&call(Some("/work/project/../secrets"))));
        assert!(scope.covers(&call(None)));
        assert!(!scope.covers(&CallScope::default()));
    }

    #[test]
    fn url_scope_is_derived_with_the_transport_parser() {
        let scope =
            |raw: &str| call_scope(&serde_json::json!({"url": raw}), Some(Path::new("/w"))).domain;
        assert_eq!(
            scope("https://api.example.com/v1"),
            Some("api.example.com".to_string())
        );
        assert_eq!(
            scope("https://user:pass@evil.example/x"),
            Some("evil.example".to_string())
        );
        assert_eq!(
            scope("https://API.EXAMPLE.COM/x"),
            Some("api.example.com".to_string())
        );
        assert_eq!(
            scope("http://trusted.example\\@evil.example/"),
            Some("trusted.example".to_string())
        );
        assert_eq!(scope("ftp://example.com/x"), None);
        assert_eq!(scope("not a url"), None);
    }

    #[test]
    fn a_rate_limited_request_is_audited_as_a_denial() {
        let scratch = Scratch::new("network-audit");
        let policy = engine(&scratch);
        policy
            .grant(
                "com.example.flood",
                Capability::NetworkHttp,
                GrantScope::Any,
                None,
            )
            .expect("grant");
        let call = CallScope {
            domain: Some("example.com".to_string()),
            ..CallScope::default()
        };
        for _ in 0..MAX_NETWORK_CALLS_PER_SECOND {
            policy
                .authorize_call(
                    "com.example.flood",
                    TrustLevel::Community,
                    Capability::NetworkHttp,
                    &call,
                )
                .expect("within the window");
        }
        policy
            .authorize_call(
                "com.example.flood",
                TrustLevel::Community,
                Capability::NetworkHttp,
                &call,
            )
            .expect_err("over the window");
        let audit = std::fs::read_to_string(scratch.0.join("audit.jsonl")).expect("audit");
        let rows: Vec<serde_json::Value> = audit
            .lines()
            .map(|line| serde_json::from_str(line).expect("audit row"))
            .collect();
        assert!(
            rows.iter().any(|row| {
                row["capability"] == "network.http"
                    && row["allowed"] == false
                    && row["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("more than"))
            }),
            "the final metering denial must be audited: {rows:?}"
        );
    }

    #[test]
    fn an_unwritable_audit_log_is_surfaced() {
        let scratch = Scratch::new("audit-degraded");
        let audit_path = scratch.0.join("audit.jsonl");
        std::fs::create_dir_all(&audit_path).expect("a directory where the audit log goes");
        let policy = PolicyEngine::open(
            scratch.0.join("grants.toml"),
            audit_path,
            PolicyMode::Enforce,
        )
        .expect("policy");
        let decision = policy.evaluate(
            "com.example",
            TrustLevel::Builtin,
            Capability::WorkspaceRead,
            &CallScope::default(),
        );
        assert!(decision.is_allowed(), "the policy itself still decided");
        assert!(
            policy.audit_degraded().is_some(),
            "a dropped audit row must be visible to the host"
        );
    }
}
