//! The principal, the action and the target a decision is about.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use regorus::Value;

/// What a tool is about to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Read a path.
    Read,
    /// Create, replace or edit a path.
    Write,
    /// Open a network connection to a resolved address.
    Egress,
    /// Start a process.
    Spawn,
    /// Ask whether a tool may be offered at all.
    ///
    /// This is the registration-time question: the tool name is the subject and
    /// the target is informational. A capability decision narrows what a turn is
    /// built with; the actions above are the call-time questions a tool asks
    /// before it touches anything.
    Capability,
}

impl Action {
    /// The stable lower-case name a policy matches on (`input.action`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Read => "read",
            Action::Write => "write",
            Action::Egress => "egress",
            Action::Spawn => "spawn",
            Action::Capability => "capability",
        }
    }

    /// Index used by the allow counters.
    pub(crate) fn index(&self) -> usize {
        match self {
            Action::Read => 0,
            Action::Write => 1,
            Action::Egress => 2,
            Action::Spawn => 3,
            Action::Capability => 4,
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The concrete thing an action touches.
///
/// A path is resolved and absolute by the time it reaches a policy. A host
/// carries the resolved IP as the enforcing unit and the hostname only as a
/// hint for a reader; that is what makes a DNS rebind visible to the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An absolute path.
    Path(PathBuf),
    /// A network endpoint: the name the request used, the address it actually
    /// resolves to, and the port it asked for.
    Host {
        /// The hostname as written by the caller, normalised to lower case.
        host: String,
        /// The resolved address the policy decides on, as text. Empty when the
        /// name could not be resolved (a host-rule-only question).
        ip: String,
        /// The destination port, when the caller knows it. A policy that names
        /// ports refuses a request whose port it cannot see.
        port: Option<u16>,
    },
}

impl Target {
    /// A path target, made absolute and lexically normalised.
    ///
    /// Lexical normalisation removes `.` and `..` so a policy that compares
    /// prefixes cannot be walked around. Symbolic links are the enforcer's job:
    /// a caller that touches the filesystem must pass its canonical path.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Target::Path(normalize_path(path.into()))
    }

    /// A host target. `ip` is the address the policy decides on.
    ///
    /// The port is left unknown, which a policy that names ports refuses. Use
    /// [Target::host_on] whenever the port is known.
    pub fn host(host: impl Into<String>, ip: impl Into<String>) -> Self {
        Target::Host {
            host: normalize_host(&host.into()),
            ip: ip.into(),
            port: None,
        }
    }

    /// A host target for a request with a known destination port.
    pub fn host_on(host: impl Into<String>, ip: impl Into<String>, port: u16) -> Self {
        Target::Host {
            host: normalize_host(&host.into()),
            ip: ip.into(),
            port: Some(port),
        }
    }

    /// The path, when this target is a path.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Target::Path(path) => Some(path),
            Target::Host { .. } => None,
        }
    }

    /// The hostname, when this target is a host.
    pub fn host_name(&self) -> Option<&str> {
        match self {
            Target::Host { host, .. } => Some(host),
            Target::Path(_) => None,
        }
    }

    /// The resolved IP, when this target is a host.
    pub fn resolved_ip(&self) -> Option<&str> {
        match self {
            Target::Host { ip, .. } => Some(ip),
            Target::Path(_) => None,
        }
    }

    /// The destination port, when this target is a host and the caller knew it.
    pub fn port(&self) -> Option<u16> {
        match self {
            Target::Host { port, .. } => *port,
            Target::Path(_) => None,
        }
    }

    /// One-line rendering for an audit row.
    pub fn display(&self) -> String {
        match self {
            Target::Path(path) => path.display().to_string(),
            Target::Host { host, ip, port } => match port {
                Some(port) => format!("{host}:{port} ({ip})"),
                None => format!("{host} ({ip})"),
            },
        }
    }

    fn to_rego_value(&self) -> Value {
        let mut fields: BTreeMap<Value, Value> = BTreeMap::new();
        match self {
            Target::Path(path) => {
                fields.insert(Value::from("kind"), Value::from("path"));
                fields.insert(
                    Value::from("path"),
                    Value::from(path.to_string_lossy().as_ref()),
                );
            }
            Target::Host { host, ip, port } => {
                fields.insert(Value::from("kind"), Value::from("host"));
                fields.insert(Value::from("host"), Value::from(host.as_str()));
                fields.insert(Value::from("ip"), Value::from(ip.as_str()));
                // Null when unknown: a policy that names ports refuses it, so an
                // unknown port can never ride a port list it was not given.
                fields.insert(
                    Value::from("port"),
                    match port {
                        Some(port) => Value::from(*port as u64),
                        None => Value::Null,
                    },
                );
            }
        }
        Value::from(fields)
    }
}

/// One policy question: who is asking, what it wants, and against what.
///
/// The principal is the session id, the active agent preset id and the tool
/// name. The target is the fourth identifying fact. All four travel to Rego as
/// `input.session`, `input.preset`, `input.tool`, `input.action` and
/// `input.target`.
#[derive(Debug, Clone)]
pub struct PolicyInput {
    /// The turn's session id. Empty when the caller has none.
    pub session: String,
    /// The preset id in effect. Empty when no preset is selected.
    pub preset: String,
    /// The tool that is asking, for example `write_file`.
    pub tool: String,
    /// What the tool wants to do.
    pub action: Action,
    /// What it wants to do it to.
    pub target: Target,
}

impl PolicyInput {
    /// Start a question for `tool` about `target`.
    pub fn new(tool: impl Into<String>, action: Action, target: Target) -> Self {
        Self {
            session: String::new(),
            preset: String::new(),
            tool: tool.into(),
            action,
            target,
        }
    }

    /// Name the session this question belongs to.
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = session.into();
        self
    }

    /// Name the agent preset in effect.
    pub fn with_preset(mut self, preset: impl Into<String>) -> Self {
        self.preset = preset.into();
        self
    }

    pub(crate) fn to_rego_value(&self, workspace_root: &str) -> Value {
        let mut fields: BTreeMap<Value, Value> = BTreeMap::new();
        fields.insert(Value::from("session"), Value::from(self.session.as_str()));
        fields.insert(Value::from("preset"), Value::from(self.preset.as_str()));
        fields.insert(Value::from("tool"), Value::from(self.tool.as_str()));
        fields.insert(Value::from("action"), Value::from(self.action.as_str()));
        fields.insert(Value::from("target"), self.target.to_rego_value());
        fields.insert(Value::from("workspace_root"), Value::from(workspace_root));
        Value::from(fields)
    }
}

/// One policy answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Whether the action may proceed.
    pub allow: bool,
    /// The id of the rule that decided it, when the policy named one.
    pub rule: Option<String>,
    /// The policy's own explanation, when it gave one.
    pub reason: Option<String>,
}

impl Decision {
    /// An unconditional allow.
    pub fn allow() -> Self {
        Self {
            allow: true,
            rule: None,
            reason: None,
        }
    }

    /// A denial naming `rule` and `reason`.
    pub fn deny(rule: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            rule: Some(rule.into()),
            reason: Some(reason.into()),
        }
    }

    /// Whether the action may proceed.
    pub fn is_allowed(&self) -> bool {
        self.allow
    }

    /// Whether the action is refused.
    pub fn is_denied(&self) -> bool {
        !self.allow
    }

    /// The refusal text a tool returns: structured, names the rule, and gives
    /// no advice about how to get around it.
    ///
    /// English and Chinese are built in because tool errors are back-end text
    /// that both locales must be able to read; `lang` follows the app's own
    /// locale tag (`zh…` selects Chinese).
    pub fn refusal(&self, lang: &str) -> String {
        let zh = lang.to_ascii_lowercase().starts_with("zh");
        let rule = self.rule.as_deref();
        let reason = rule
            .and_then(|id| reason_for(id, zh))
            .or_else(|| self.reason.as_deref().map(str::to_string))
            .unwrap_or_else(|| {
                if zh {
                    "未给出原因".to_string()
                } else {
                    "no reason given".to_string()
                }
            });
        match (zh, rule) {
            (true, Some(id)) => format!("策略拒绝：{reason}（规则 {id}）。"),
            (false, Some(id)) => format!("Refused by policy: {reason} (rule {id})."),
            (true, None) => format!("策略拒绝：{reason}。"),
            (false, None) => format!("Refused by policy: {reason}."),
        }
    }
}

/// A translated reason for the rule ids this workspace ships.
///
/// A policy the user writes may use any id; an unknown id falls back to the
/// reason the policy itself wrote, so a custom rule never loses its voice.
fn reason_for(rule: &str, zh: bool) -> Option<String> {
    let (en, cn) = match rule {
        "read-inside-workspace" => ("the path is inside the workspace", "路径在工作区内"),
        "read-outside-workspace" => ("reads are confined to the workspace", "读取仅限工作区内"),
        "write-inside-workspace" => ("the path is inside the workspace", "路径在工作区内"),
        "write-outside-workspace" => ("writes are confined to the workspace", "写入仅限工作区内"),
        "write-forbidden-directory" => (
            "writes into .bsh or .git internals are forbidden",
            "不能写入 .bsh 或 .git 内部",
        ),
        "egress-allowlisted" => ("the address is allowlisted", "地址在白名单内"),
        "egress-host-allowlisted" => ("the hostname is allowlisted", "主机名在白名单内"),
        "egress-port-not-allowed" => (
            "the destination port is not on the egress port list",
            "目标端口不在出站端口白名单内",
        ),
        "egress-not-allowlisted" => (
            "the resolved address is not on the egress allowlist",
            "解析后的地址不在出站白名单内",
        ),
        "egress-unresolved" => ("the host could not be resolved", "无法解析目标主机"),
        "spawn-shell-in-workspace" => (
            "the shell may start in the workspace",
            "允许在工作区启动 shell",
        ),
        "spawn-not-allowed" => ("this process may not be spawned", "不允许启动该进程"),
        "default-deny" => (
            "no policy rule permits this action",
            "没有策略规则允许该操作",
        ),
        "policy-evaluator-failure" => ("the policy could not be evaluated", "策略无法评估"),
        "policy-evaluator-failure-read" => (
            "the policy could not be evaluated; an in-workspace read is allowed",
            "策略无法评估；允许工作区内的读取",
        ),
        _ => return None,
    };
    Some(if zh { cn } else { en }.to_string())
}

/// Lower-case a host and drop the root label's trailing dot.
///
/// DNS names are case-insensitive and `example.com.` is the same name, so a
/// policy that matched the raw text would be trivially walked around.
fn normalize_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Make a path absolute and remove `.` and `..` components.
pub(crate) fn normalize_path(path: PathBuf) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path,
        }
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
        }
    }
    out
}

/// Whether `path` is `root` or below it, compared by path components.
pub(crate) fn inside_root(path: &Path, root: &Path) -> bool {
    if root.as_os_str().is_empty() {
        return false;
    }
    path.starts_with(root)
}
