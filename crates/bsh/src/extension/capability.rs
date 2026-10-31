//! Capability vocabulary and the host policy that turns a declaration into a
//! decision.
//!
//! A manifest declaring a capability is a REQUEST. The only grant is either a
//! default rule for a trust level or an explicit record in
//! `extension-state/grants.toml`. Every tool call that needs a sensitive
//! capability asks this module first, and every answer is written to the audit
//! log, so "the extension said it needed this" and "the host allowed this" are
//! two different facts that can always be compared.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::types::{ExtensionError, TrustLevel};

/// The current time as Unix seconds, saturating at the epoch on a clock that
/// cannot answer.
pub(crate) fn unix_now() -> u64 {
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
pub(crate) enum Capability {
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
}

impl Capability {
    /// Every capability, in declaration order.
    pub(crate) const ALL: [Capability; 18] = [
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
        Self::AgentModelsProvide,
        Self::AgentAgentsProvide,
        Self::AgentWorkflowsProvide,
        Self::EventsPublish,
        Self::EventsSubscribe,
        Self::UiPanels,
        Self::UiCommands,
        Self::UiTheme,
    ];

    /// The stable manifest string.
    pub(crate) fn as_str(self) -> &'static str {
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
            Self::AgentModelsProvide => "agent.models.provide",
            Self::AgentAgentsProvide => "agent.agents.provide",
            Self::AgentWorkflowsProvide => "agent.workflows.provide",
            Self::EventsPublish => "events.publish",
            Self::EventsSubscribe => "events.subscribe",
            Self::UiPanels => "ui.panels",
            Self::UiCommands => "ui.commands",
            Self::UiTheme => "ui.theme",
        }
    }

    /// A one-sentence description for the settings row and `bsh ext info`.
    pub(crate) fn description(self) -> &'static str {
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
            Self::AgentModelsProvide => "register model or provider adapters",
            Self::AgentAgentsProvide => "register agent roles or execution strategies",
            Self::AgentWorkflowsProvide => "register reusable workflows",
            Self::EventsPublish => "publish events on the host bus",
            Self::EventsSubscribe => "subscribe to events on the host bus",
            Self::UiPanels => "contribute host-managed panels",
            Self::UiCommands => "contribute host commands and keybindings",
            Self::UiTheme => "contribute visual theme resources",
        }
    }

    /// The risk bucket a default policy rule is written against.
    pub(crate) fn risk(self) -> Risk {
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
            Self::AgentModelsProvide | Self::AgentAgentsProvide | Self::AgentWorkflowsProvide => {
                Risk::Medium
            }
            Self::WorkspaceWrite | Self::NetworkHttp => Risk::High,
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
pub(crate) enum Risk {
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
    pub(crate) fn as_str(self) -> &'static str {
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
pub(crate) enum GrantScope {
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
                write!(formatter, "program:{program}:{}", args.join(","))
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
            return Ok(Self::Path(PathBuf::from(path)));
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
                let args = args
                    .split(',')
                    .filter(|arg| !arg.is_empty())
                    .map(str::to_string)
                    .collect();
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
pub(crate) struct Grant {
    /// What the grant is limited to.
    #[serde(default = "any_scope")]
    pub(crate) scope: GrantScope,
    /// Unix seconds the grant was written.
    #[serde(default)]
    pub(crate) granted_at: u64,
    /// Why the reader granted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

fn any_scope() -> GrantScope {
    GrantScope::Any
}

impl Grant {
    /// Build a grant with the current timestamp.
    pub(crate) fn new(scope: GrantScope, note: Option<String>) -> Self {
        Self {
            scope,
            granted_at: unix_now(),
            note,
        }
    }
}

/// The persisted grant document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GrantFile {
    /// Format generation.
    #[serde(default = "grant_format")]
    pub(crate) format: u32,
    /// extension id -> capability -> grant.
    #[serde(default)]
    pub(crate) grants: BTreeMap<String, BTreeMap<String, Grant>>,
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
pub(crate) struct GrantStore {
    path: PathBuf,
    file: GrantFile,
}

impl GrantStore {
    /// Load the store at `path`, creating an empty one when it is absent.
    pub(crate) fn load(path: PathBuf) -> Result<Self, ExtensionError> {
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
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// All grants for one extension.
    pub(crate) fn grants_for(&self, extension: &str) -> BTreeMap<String, Grant> {
        self.file.grants.get(extension).cloned().unwrap_or_default()
    }

    /// Look up one grant.
    pub(crate) fn get(&self, extension: &str, capability: Capability) -> Option<&Grant> {
        self.file
            .grants
            .get(extension)
            .and_then(|grants| grants.get(capability.as_str()))
    }

    /// Add or replace a grant and persist the document.
    pub(crate) fn grant(
        &mut self,
        extension: &str,
        capability: Capability,
        scope: GrantScope,
        note: Option<String>,
    ) -> Result<(), ExtensionError> {
        self.file
            .grants
            .entry(extension.to_string())
            .or_default()
            .insert(capability.as_str().to_string(), Grant::new(scope, note));
        self.save()
    }

    /// Remove a grant and persist the document. Returns whether it existed.
    pub(crate) fn revoke(
        &mut self,
        extension: &str,
        capability: Capability,
    ) -> Result<bool, ExtensionError> {
        let removed = self
            .file
            .grants
            .get_mut(extension)
            .map(|grants| grants.remove(capability.as_str()).is_some())
            .unwrap_or(false);
        if removed {
            if self
                .file
                .grants
                .get(extension)
                .is_some_and(BTreeMap::is_empty)
            {
                self.file.grants.remove(extension);
            }
            self.save()?;
        }
        Ok(removed)
    }

    /// Serialize the store deterministically.
    pub(crate) fn to_toml(&self) -> Result<String, ExtensionError> {
        toml::to_string_pretty(&self.file).map_err(|error| ExtensionError::Corrupt {
            path: self.path.clone(),
            message: error.to_string(),
        })
    }

    fn save(&self) -> Result<(), ExtensionError> {
        let text = self.to_toml()?;
        super::package::write_atomic(&self.path, text.as_bytes())
    }
}

/// What a host service is being asked to do, for scope matching.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallScope {
    /// The workspace open for this call.
    pub(crate) workspace: Option<PathBuf>,
    /// The resource path being touched.
    pub(crate) path: Option<PathBuf>,
    /// The DNS host being reached.
    pub(crate) domain: Option<String>,
    /// The HTTP method, when the call is a network request.
    pub(crate) method: Option<String>,
    /// The declared executable of a process call.
    pub(crate) program: Option<String>,
    /// The arguments of a process call.
    pub(crate) args: Option<Vec<String>>,
    /// The secret being read.
    pub(crate) secret: Option<String>,
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
    pub(crate) fn covers(&self, call: &CallScope) -> bool {
        match self {
            Self::Any => true,
            Self::Workspace => call.workspace.is_some(),
            Self::Path(prefix) => call
                .path
                .as_ref()
                .or(call.workspace.as_ref())
                .is_some_and(|path| path.starts_with(prefix)),
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
pub(crate) enum Decision {
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
    pub(crate) fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    /// The denial reason, empty when allowed.
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Allow { .. } => "",
            Self::Deny { reason } => reason,
        }
    }
}

/// Whether the policy is enforced or only recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyMode {
    /// Denials stop the operation.
    Enforce,
    /// Denials are audited but the operation proceeds. Development only.
    Advisory,
}

/// The host policy engine.
pub(crate) struct PolicyEngine {
    grants: Mutex<GrantStore>,
    audit: AuditLog,
    mode: PolicyMode,
}

impl PolicyEngine {
    /// Build an engine over a grant file and an audit log.
    pub(crate) fn open(
        grants_path: PathBuf,
        audit_path: PathBuf,
        mode: PolicyMode,
    ) -> Result<Self, ExtensionError> {
        Ok(Self {
            grants: Mutex::new(GrantStore::load(grants_path)?),
            audit: AuditLog::new(audit_path),
            mode,
        })
    }

    /// The grant store path, for diagnostics.
    pub(crate) fn grants_path(&self) -> PathBuf {
        self.grants
            .lock()
            .map(|store| store.path().to_path_buf())
            .unwrap_or_default()
    }

    /// All grants for one extension.
    pub(crate) fn grants_for(&self, extension: &str) -> BTreeMap<String, Grant> {
        self.grants
            .lock()
            .map(|store| store.grants_for(extension))
            .unwrap_or_default()
    }

    /// Add or replace a grant.
    pub(crate) fn grant(
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
    pub(crate) fn revoke(
        &self,
        extension: &str,
        capability: Capability,
    ) -> Result<bool, ExtensionError> {
        self.grants
            .lock()
            .map_err(|_| ExtensionError::Corrupt {
                path: self.grants_path(),
                message: "grant store lock is poisoned".to_string(),
            })?
            .revoke(extension, capability)
    }

    /// Evaluate one capability for one extension.
    pub(crate) fn evaluate(
        &self,
        extension: &str,
        trust: TrustLevel,
        capability: Capability,
        scope: &CallScope,
    ) -> Decision {
        let decision = self.decide(extension, trust, capability, scope);
        self.audit
            .record(extension, capability.as_str(), &decision, trust);
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
    pub(crate) fn preview(
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
    pub(crate) fn authorize_required(
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
    pub(crate) fn authorize_call(
        &self,
        extension: &str,
        trust: TrustLevel,
        capability: Capability,
        scope: &CallScope,
    ) -> Result<GrantScope, ExtensionError> {
        match self.evaluate(extension, trust, capability, scope) {
            Decision::Allow { scope } => Ok(scope),
            Decision::Deny { reason } => Err(ExtensionError::Denied {
                extension: extension.to_string(),
                capability: capability.as_str().to_string(),
                reason,
            }),
        }
    }

    fn decide(
        &self,
        extension: &str,
        trust: TrustLevel,
        capability: Capability,
        scope: &CallScope,
    ) -> Decision {
        if let Ok(store) = self.grants.lock() {
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
        }
        match trust {
            TrustLevel::Builtin => Decision::Allow {
                scope: GrantScope::Any,
            },
            TrustLevel::Verified if capability.risk() <= Risk::Medium => Decision::Allow {
                scope: default_scope(capability, scope),
            },
            TrustLevel::Community | TrustLevel::Local | TrustLevel::Unknown
                if capability.risk() == Risk::Low =>
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
pub(crate) struct AuditLog {
    path: PathBuf,
    max_bytes: u64,
}

impl AuditLog {
    /// Build a log at `path` that rotates at 1 MiB.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            max_bytes: 1024 * 1024,
        }
    }

    /// The log path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn record(&self, extension: &str, capability: &str, decision: &Decision, trust: TrustLevel) {
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
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{line}");
        }
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
}
