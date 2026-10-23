//! Compiling policy sources into one engine and evaluating a decision.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use regorus::languages::rego::compiler::Compiler as RvmCompiler;
use regorus::rvm::vm::RegoVM;
use regorus::{Engine, Rc, Value};

use super::audit::{PolicyAudit, PolicyAuditEntry};
use super::baseline_source;
use super::egress::EgressRuleSet;
use super::target::{inside_root, normalize_path, Action, Decision, PolicyInput, Target};

/// Where one policy module came from.
///
/// The origin decides the DEFAULT trust and the id namespace: a built-in module
/// ships in the binary and is trusted by construction, while a workspace file
/// or an extension contribution is the reader's own material and must pass the
/// host's trust gate before it is compiled. The pane lists the origin beside
/// every module id, because "which policy refused me" is only answerable when
/// the reader can tell their own rule from the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolicyOrigin {
    /// Shipped in the binary under a `builtin:` id, trusted by default.
    Builtin,
    /// Read from the workspace's own `.bsh/policy.rego`.
    Workspace,
    /// Supplied by a preset declaration or an installed extension.
    Extension,
}

impl PolicyOrigin {
    /// The stable lower-case name the pane and the audit show.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "built-in",
            Self::Workspace => "workspace",
            Self::Extension => "extension",
        }
    }

    /// Whether a module from this origin is trusted without a separate gate.
    pub fn trusted_by_default(self) -> bool {
        matches!(self, Self::Builtin | Self::Workspace)
    }
}

/// One Rego module the host supplies to [`PolicySet::compile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySource {
    /// Stable label, used as the module's file name and in audit text.
    pub id: String,
    /// The Rego source.
    pub rego: String,
    /// Where the module came from.
    pub origin: PolicyOrigin,
    /// Whether the host's trust gate admitted this module.
    ///
    /// A built-in and a workspace module are trusted by location; an extension
    /// module is trusted only after the reader's explicit approval, which the
    /// host records on the preset declaration. The flag is metadata at this
    /// layer - an untrusted extension module never reaches the set - and it is
    /// what the policy pane lists so the reader can see the gate's answer.
    pub trusted: bool,
    /// Whether this module's refusal is ADVISORY.
    ///
    /// The host sets it on the modules a preset contributes when the preset's
    /// `policy_mode = advisory`. Such a refusal is recorded in the audit and
    /// returned as a warning, and the call proceeds - the preset author asked
    /// to be told rather than obeyed. The baseline, the session tier and the
    /// workspace policy are never advisory: the floor still refuses, which is
    /// what keeps advisory from widening the session.
    pub advisory: bool,
}

impl PolicySource {
    /// Name an extension-supplied source. The default constructor keeps the
    /// historical signature, and the origin makes the trust question explicit
    /// to a caller that does not name one: an unnamed contribution is an
    /// extension's, not the binary's.
    pub fn new(id: impl Into<String>, rego: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            rego: rego.into(),
            origin: PolicyOrigin::Extension,
            trusted: false,
            advisory: false,
        }
    }

    /// Name a module shipped in the binary. It is trusted by default and its id
    /// must live under `builtin:`.
    pub fn builtin(id: impl Into<String>, rego: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            rego: rego.into(),
            origin: PolicyOrigin::Builtin,
            trusted: true,
            advisory: false,
        }
    }

    /// Name the workspace's own policy file. It is the reader's material,
    /// written inside a directory the tool boundary refuses to write, so it is
    /// trusted by location.
    pub fn workspace(id: impl Into<String>, rego: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            rego: rego.into(),
            origin: PolicyOrigin::Workspace,
            trusted: true,
            advisory: false,
        }
    }

    /// Name an extension-supplied module, with the host's trust answer.
    pub fn extension(id: impl Into<String>, rego: impl Into<String>, trusted: bool) -> Self {
        Self {
            id: id.into(),
            rego: rego.into(),
            origin: PolicyOrigin::Extension,
            trusted,
            advisory: false,
        }
    }

    /// Mark this module's refusals as advisory.
    ///
    /// Only the modules a preset itself contributes may be advisory; the floor
    /// (baseline, session tier, workspace) is never marked, so advisory can
    /// relax the preset's own narrowing but cannot widen the session.
    pub fn advisory(mut self, advisory: bool) -> Self {
        self.advisory = advisory;
        self
    }

    /// Return this source with a different id.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Stamp a binary-shipped module's stable `builtin:` id and origin.
    ///
    /// The wrapper constructors in [`super`] use it so the in-tree Rego text
    /// keeps living beside its module while its identity is the reserved one.
    pub fn as_builtin(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self.origin = PolicyOrigin::Builtin;
        self.trusted = true;
        self
    }
}

/// The data document every policy module reads.
///
/// It is installed once, at compile time, and never mutated per decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyData {
    /// The workspace root the baseline confines paths to.
    pub workspace_root: PathBuf,
    /// The agent preset id in effect, mirrored into Rego as `data.preset`.
    pub preset: String,
    /// Resolved addresses egress may reach. An exact address or a CIDR range.
    ///
    /// Empty is the documented default and denies all egress.
    pub egress_allowlist: Vec<String>,
    /// Hostnames egress may reach, lower-cased. A leading "*." matches any
    /// subdomain ("*.example.com" but not "example.com").
    ///
    /// This is the WEAKER door and it is named as such: a hostname rule survives
    /// a DNS rebind, because the name is what the policy saw. An address rule is
    /// what the kernel-level tools use; a host rule exists for a policy that
    /// names a service rather than an address.
    pub egress_hosts: Vec<String>,
    /// Destination ports egress may use. Empty means any port; a non-empty list
    /// refuses a request whose port the caller did not state.
    pub egress_ports: Vec<u16>,
    /// Whether the workspace declared NO egress allowlist file at all.
    ///
    /// The three states of `.bsh/egress-allowlist` are not the two an empty and
    /// a non-empty rule list can express. An ABSENT file declares nothing and
    /// leaves egress UNRESTRICTED, so a fresh workspace can reach the network
    /// without its reader writing a file first. A file that is PRESENT and
    /// parses to no rule is a deliberate deny-all. The lists are empty in both
    /// cases, so the distinction is this flag, and the baseline's
    /// `egress-unrestricted` arm is the only reader of it.
    ///
    /// Defaults to false, which keeps the documented deny-all floor for a
    /// caller that builds the data directly.
    pub egress_unrestricted: bool,
    /// Whether the preset restricts reads to `preset_read_roots`.
    pub preset_read_restricted: bool,
    /// Canonical absolute read roots the preset allows.
    pub preset_read_roots: Vec<String>,
    /// Whether the preset restricts writes to `preset_write_roots`.
    pub preset_write_restricted: bool,
    /// Canonical absolute write roots the preset allows.
    pub preset_write_roots: Vec<String>,
    /// Canonical absolute paths the preset always denies.
    pub preset_deny_roots: Vec<String>,
    /// Absolute GLOB patterns the preset allows to read.
    ///
    /// A scope entry containing `*`, `?`, `[` or `{` is a glob rather than a
    /// literal root; the two lists are parallel, not alternatives, so a preset
    /// that mixes `docs` and `src/**/*.rs` keeps both meanings.
    pub preset_read_globs: Vec<String>,
    /// Absolute GLOB patterns the preset allows to write.
    pub preset_write_globs: Vec<String>,
    /// Absolute GLOB patterns the preset always denies.
    ///
    /// A deny glob is decided BEFORE the read and write scopes, so it refuses a
    /// path the tier and the workspace root would otherwise allow.
    pub preset_deny_globs: Vec<String>,
    /// The preset's permission ceiling, mirrored into Rego.
    pub preset_permission: String,
    /// The effective permission: the session's mode capped by the preset's
    /// ceiling, mirrored into Rego as `data.permission`.
    ///
    /// The capability query reads it so a read-only turn refuses the
    /// side-effecting tools in Rego as well as in the Rust registration gate.
    pub permission: String,
    /// Whether the preset restricts egress to the preset allowlist.
    pub preset_egress_restricted: bool,
    /// Egress ADDRESS entries the preset allows, intersected with the
    /// workspace list. An exact address or a CIDR range; the strong door.
    pub preset_egress_allowlist: Vec<String>,
    /// Egress HOSTNAME entries the preset allows, lower-cased. A leading
    /// `*.` matches any subdomain. The weaker door, and named as such.
    pub preset_egress_hosts: Vec<String>,
    /// Destination ports the preset allows. Empty means the preset does not
    /// narrow the port dimension; a non-empty list refuses any other port.
    pub preset_egress_ports: Vec<u16>,
}

/// Lower-case one hostname entry and drop its trailing dot.
///
/// DNS names are case-insensitive and "example.com." is the same name, so a
/// policy that matched the raw text could be walked around.
fn normalize_host_entry(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

impl PolicyData {
    /// Data for `workspace_root`, with no preset and an empty allowlist.
    ///
    /// An empty root stays empty: a build with no workspace has no directory
    /// the baseline may call "inside", so its data does not silently become the
    /// process's current directory.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let raw = workspace_root.into();
        let workspace_root = if raw.as_os_str().is_empty() {
            raw
        } else {
            normalize_path(raw)
        };
        Self {
            workspace_root,
            preset: String::new(),
            egress_allowlist: Vec::new(),
            egress_hosts: Vec::new(),
            egress_ports: Vec::new(),
            egress_unrestricted: false,
            preset_read_restricted: false,
            preset_read_roots: Vec::new(),
            preset_write_restricted: false,
            preset_write_roots: Vec::new(),
            preset_deny_roots: Vec::new(),
            preset_read_globs: Vec::new(),
            preset_write_globs: Vec::new(),
            preset_deny_globs: Vec::new(),
            preset_permission: String::new(),
            permission: String::new(),
            preset_egress_restricted: false,
            preset_egress_allowlist: Vec::new(),
            preset_egress_hosts: Vec::new(),
            preset_egress_ports: Vec::new(),
        }
    }

    /// Set the preset id in force.
    pub fn with_preset(mut self, preset: impl Into<String>) -> Self {
        self.preset = preset.into();
        self
    }

    /// Replace the egress allowlist.
    pub fn with_egress_allowlist<I, S>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.egress_allowlist = entries.into_iter().map(Into::into).collect();
        self
    }

    /// Add one allowlist entry.
    pub fn allow_egress(mut self, entry: impl Into<String>) -> Self {
        self.egress_allowlist.push(entry.into());
        self
    }

    /// Replace the hostname allowlist.
    ///
    /// Entries are lower-cased and a trailing dot is dropped, because DNS names
    /// are case-insensitive and "example.com." is the same name: a policy that
    /// matched the raw text could be walked around.
    pub fn with_egress_hosts<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.egress_hosts = hosts
            .into_iter()
            .map(|host| normalize_host_entry(&host.into()))
            .filter(|host| !host.is_empty())
            .collect();
        self
    }

    /// Add one hostname to the allowlist.
    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        let host = normalize_host_entry(&host.into());
        if !host.is_empty() {
            self.egress_hosts.push(host);
        }
        self
    }

    /// Replace the port allowlist. An empty list means any port.
    pub fn with_egress_ports<I>(mut self, ports: I) -> Self
    where
        I: IntoIterator<Item = u16>,
    {
        self.egress_ports = ports.into_iter().collect();
        self
    }

    /// Allow one destination port.
    pub fn allow_port(mut self, port: u16) -> Self {
        self.egress_ports.push(port);
        self
    }

    /// Declare that the workspace has NO egress allowlist file.
    ///
    /// This is the absent arm of the tri-state: egress is unrestricted unless a
    /// preset narrows it. It is a fact about the workspace (the reader has not
    /// created the file yet), not a widening a caller may apply over a file
    /// that exists: a present file with no parsed rule must be handed over with
    /// this flag false so the baseline keeps its deny-all answer.
    pub fn with_egress_unrestricted(mut self, unrestricted: bool) -> Self {
        self.egress_unrestricted = unrestricted;
        self
    }

    /// Install the preset's path scope. Empty restricted lists deny all reads
    /// or writes; unrestricted inherits the baseline.
    pub fn with_preset_scope(
        mut self,
        read_restricted: bool,
        read_roots: Vec<String>,
        write_restricted: bool,
        write_roots: Vec<String>,
        deny_roots: Vec<String>,
    ) -> Self {
        self.preset_read_restricted = read_restricted;
        self.preset_read_roots = read_roots;
        self.preset_write_restricted = write_restricted;
        self.preset_write_roots = write_roots;
        self.preset_deny_roots = deny_roots;
        self
    }

    /// Install the preset's GLOB scopes beside its literal roots.
    ///
    /// The two shapes are parallel: a preset may declare `read = ["docs",
    /// "src/**/*.rs"]`, and a path matches when it is under the literal root
    /// OR matches the glob. Deny wins over both because the scope module
    /// decides it first.
    pub fn with_preset_globs(
        mut self,
        read_globs: Vec<String>,
        write_globs: Vec<String>,
        deny_globs: Vec<String>,
    ) -> Self {
        self.preset_read_globs = read_globs;
        self.preset_write_globs = write_globs;
        self.preset_deny_globs = deny_globs;
        self
    }

    /// Mirror the preset's permission ceiling into the data document.
    pub fn with_preset_permission(mut self, permission: impl Into<String>) -> Self {
        self.preset_permission = permission.into();
        self
    }

    /// Mirror the effective permission (session mode capped by the preset
    /// ceiling) into the data document as `data.permission`.
    pub fn with_permission(mut self, permission: impl Into<String>) -> Self {
        self.permission = permission.into();
        self
    }

    /// Install the preset's egress scope, classified into its three shapes.
    ///
    /// `restricted` is true whenever the preset DECLARED an `egress` field,
    /// even an empty one: a declared empty list denies everything, which is not
    /// the same as an absent field. The rules carry the preset's own addresses,
    /// hostnames and ports; the policy intersects each of them with the
    /// workspace list rather than replacing it.
    pub fn with_preset_egress(mut self, restricted: bool, rules: EgressRuleSet) -> Self {
        self.preset_egress_restricted = restricted;
        self.preset_egress_allowlist = rules.addresses;
        self.preset_egress_hosts = rules.hosts;
        self.preset_egress_ports = rules.ports;
        self
    }

    /// The workspace root as a policy compares it.
    pub fn workspace_root_text(&self) -> String {
        self.workspace_root.to_string_lossy().to_string()
    }

    /// The root with a trailing separator, for one `startswith` test.
    ///
    /// A root of `/` stays `/`; every other root becomes `root/`. A policy
    /// tests `startswith(concat("", [path, "/"]), data.workspace_prefix)`, which
    /// covers the root itself and every descendant with one call.
    pub fn workspace_prefix(&self) -> String {
        let root = self.workspace_root_text();
        if root.is_empty() {
            String::new()
        } else if root == "/" {
            root
        } else {
            format!("{root}/")
        }
    }

    fn to_rego_value(&self) -> Value {
        let allowlist: Vec<Value> = self
            .egress_allowlist
            .iter()
            .map(|entry| Value::from(entry.as_str()))
            .collect();
        let roots = |items: &[String]| -> Vec<Value> {
            items
                .iter()
                .map(|item| Value::from(item.as_str()))
                .collect()
        };
        let ports: Vec<Value> = self
            .egress_ports
            .iter()
            .map(|port| Value::from(u64::from(*port)))
            .collect();
        let mut root: BTreeMap<Value, Value> = BTreeMap::new();
        root.insert(
            Value::from("workspace_root"),
            Value::from(self.workspace_root_text().as_str()),
        );
        root.insert(
            Value::from("workspace_prefix"),
            Value::from(self.workspace_prefix().as_str()),
        );
        root.insert(Value::from("preset"), Value::from(self.preset.as_str()));
        root.insert(Value::from("egress_allowlist"), Value::from(allowlist));
        root.insert(
            Value::from("egress_hosts"),
            Value::from(roots(&self.egress_hosts)),
        );
        root.insert(Value::from("egress_ports"), Value::from(ports));
        root.insert(
            Value::from("egress_unrestricted"),
            Value::from(self.egress_unrestricted),
        );
        let mut scope: BTreeMap<Value, Value> = BTreeMap::new();
        scope.insert(
            Value::from("read_restricted"),
            Value::from(self.preset_read_restricted),
        );
        scope.insert(
            Value::from("read_roots"),
            Value::from(roots(&self.preset_read_roots)),
        );
        scope.insert(
            Value::from("write_restricted"),
            Value::from(self.preset_write_restricted),
        );
        scope.insert(
            Value::from("write_roots"),
            Value::from(roots(&self.preset_write_roots)),
        );
        scope.insert(
            Value::from("deny_roots"),
            Value::from(roots(&self.preset_deny_roots)),
        );
        scope.insert(
            Value::from("read_globs"),
            Value::from(roots(&self.preset_read_globs)),
        );
        scope.insert(
            Value::from("write_globs"),
            Value::from(roots(&self.preset_write_globs)),
        );
        scope.insert(
            Value::from("deny_globs"),
            Value::from(roots(&self.preset_deny_globs)),
        );
        root.insert(Value::from("preset_scope"), Value::from(scope));
        root.insert(
            Value::from("preset_permission"),
            Value::from(self.preset_permission.as_str()),
        );
        root.insert(
            Value::from("permission"),
            Value::from(self.permission.as_str()),
        );
        let preset_ports: Vec<Value> = self
            .preset_egress_ports
            .iter()
            .map(|port| Value::from(u64::from(*port)))
            .collect();
        let mut egress: BTreeMap<Value, Value> = BTreeMap::new();
        egress.insert(
            Value::from("restricted"),
            Value::from(self.preset_egress_restricted),
        );
        egress.insert(
            Value::from("allowlist"),
            Value::from(roots(&self.preset_egress_allowlist)),
        );
        egress.insert(
            Value::from("hosts"),
            Value::from(roots(&self.preset_egress_hosts)),
        );
        egress.insert(Value::from("ports"), Value::from(preset_ports));
        root.insert(Value::from("preset_egress"), Value::from(egress));
        Value::from(root)
    }
}

/// Why a [`PolicySet`] could not be built, or could not answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// No source was supplied.
    #[error("no policy sources were supplied")]
    NoSources,
    /// A source could not be read as a Rego module.
    #[error("policy source '{policy}' is invalid: {message}")]
    InvalidSource {
        /// The source label.
        policy: String,
        /// What was wrong.
        message: String,
    },
    /// Two sources declared the same package.
    #[error("policy modules '{first}' and '{second}' share a package")]
    DuplicatePackage {
        /// First source.
        first: String,
        /// Second source.
        second: String,
    },
    /// Two sources declared the same module id.
    #[error("policy modules '{first}' and '{second}' share the id '{id}'")]
    DuplicateId {
        /// First source.
        first: String,
        /// Second source.
        second: String,
        /// The id both claimed.
        id: String,
    },
    /// A workspace or extension module claimed an id reserved for a module
    /// shipped in the binary.
    #[error(
        "the {origin} policy module '{claimed}' claims the built-in id '{claimed}'; built-in ids are reserved for binary-shipped modules (for example '{builtin}')"
    )]
    BuiltinIdClaimed {
        /// The origin that tried to claim it.
        origin: &'static str,
        /// The id it claimed.
        claimed: String,
        /// The built-in module the id belongs to.
        builtin: String,
    },
    /// The data document could not be installed.
    #[error("policy data is invalid: {0}")]
    InvalidData(String),
    /// A module was evaluated and returned no value.
    #[error("policy source '{policy}' produced no verdict")]
    NoVerdict {
        /// The source label.
        policy: String,
    },
    /// A module's evaluation failed.
    #[error("policy source '{policy}' failed to evaluate: {message}")]
    Evaluation {
        /// The source label.
        policy: String,
        /// The engine's message.
        message: String,
    },
}

/// One compiled module: what to query, where it came from, and its rules.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PolicyModule {
    id: String,
    origin: PolicyOrigin,
    trusted: bool,
    advisory: bool,
    query: String,
    rule_ids: Vec<String>,
}

/// One module as the policy pane lists it.
///
/// The four facts answer "which policy refused me, and who is responsible for
/// it": the stable id, the origin that decides whether a trust gate applies,
/// that gate's answer, and the rule ids the module declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyModuleInfo {
    /// The stable module id (a `builtin:` id for a binary-shipped module).
    pub id: String,
    /// Where the module came from.
    pub origin: PolicyOrigin,
    /// Whether the module is trusted to apply.
    pub trusted: bool,
    /// The module's package path, as its `package` declaration reads.
    pub package: String,
    /// Every `# rule: <id>` the module declares, in source order.
    pub rule_ids: Vec<String>,
}

/// The stable ids of every policy module this crate ships in the binary.
///
/// A `builtin:` id is reserved: a workspace file or an extension module that
/// declares one is refused at compile time, naming both the claim and the
/// built-in it would shadow. The host may add its own built-ins (the preset
/// guard modules in `bsh`) under the same namespace.
pub fn builtin_policy_ids() -> &'static [&'static str] {
    &[
        "builtin:baseline",
        "builtin:session-permission",
        "builtin:preset-scope",
        "builtin:preset-egress",
        "builtin:preset-permission",
    ]
}

/// One bytecode program and the entry point it answers.
struct VmModule {
    vm: RegoVM,
    id: String,
    entry: String,
}

/// A compiled policy: one engine, one data document, one audit.
///
/// [`PolicySet::compile`] parses every source and installs the data once;
/// every later [`PolicySet::check`] reuses that engine. See the module docs for
/// why this is the only performance lever a JIT-less interpreter offers.
pub struct PolicySet {
    /// The interpreter, used as the fallback and as the compiler for the RVM.
    engine: Mutex<Option<Engine>>,
    /// One compiled bytecode program per module, when the RVM compiler
    /// accepted every module. It is the preferred evaluator: same policy, no
    /// JIT, roughly three times faster than the interpreter on the baseline.
    vms: Mutex<Option<Vec<VmModule>>>,
    modules: Vec<PolicyModule>,
    data: PolicyData,
    failure: Option<PolicyError>,
    audit: PolicyAudit,
    lang: String,
}

impl PolicySet {
    /// Compile `sources` into one engine and install `data` once.
    ///
    /// A source that does not parse does not make this method fail: the set is
    /// built in its failure mode and every check applies the failure asymmetry
    /// (deny writes and egress, allow in-workspace reads). That is deliberate -
    /// a caller cannot forget a `Result` and accidentally run with no policy.
    pub fn compile<I>(sources: I, data: PolicyData) -> Self
    where
        I: IntoIterator<Item = PolicySource>,
    {
        let sources: Vec<PolicySource> = sources.into_iter().collect();
        let mut set = Self {
            engine: Mutex::new(None),
            vms: Mutex::new(None),
            modules: Vec::new(),
            data,
            failure: None,
            audit: PolicyAudit::new(),
            lang: "en".to_string(),
        };
        if sources.is_empty() {
            set.failure = Some(PolicyError::NoSources);
            return set;
        }

        let mut engine = Engine::new();
        let mut modules: Vec<PolicyModule> = Vec::new();
        for source in sources {
            let module = match module_for(&source) {
                Ok(module) => module,
                Err(error) => {
                    set.failure = Some(error);
                    return set;
                }
            };
            // The `builtin:` namespace is RESERVED. A workspace file or an
            // extension module that declares one of these ids is refused by
            // name: a user module must never be able to shadow the floor the
            // binary ships, because the pane, the audit and the reader all read
            // a built-in id as trusted.
            if module.origin != PolicyOrigin::Builtin {
                if let Some(builtin) = builtin_id_claimed(&module.id) {
                    set.failure = Some(PolicyError::BuiltinIdClaimed {
                        origin: module.origin.as_str(),
                        claimed: module.id.clone(),
                        builtin,
                    });
                    return set;
                }
            }
            if let Some(previous) = modules.iter().find(|other| other.id == module.id) {
                set.failure = Some(PolicyError::DuplicateId {
                    first: previous.id.clone(),
                    second: module.id.clone(),
                    id: module.id.clone(),
                });
                return set;
            }
            if let Some(previous) = modules.iter().find(|other| other.query == module.query) {
                set.failure = Some(PolicyError::DuplicatePackage {
                    first: previous.id.clone(),
                    second: module.id.clone(),
                });
                return set;
            }
            let path = format!("{}.rego", module.id);
            if let Err(error) = engine.add_policy(path.clone(), source.rego.clone()) {
                set.failure = Some(PolicyError::InvalidSource {
                    policy: module.id,
                    message: error.to_string(),
                });
                return set;
            }
            modules.push(module);
        }
        let data_value = set.data.to_rego_value();
        if let Err(error) = engine.add_data(data_value.clone()) {
            set.failure = Some(PolicyError::InvalidData(error.to_string()));
            return set;
        }
        // Compile the bytecode programs before the interpreter is moved into
        // the set. A module the RVM compiler refuses is not an error: the
        // interpreter remains, and the decision is the same policy either way.
        let vms = compile_vms(&mut engine, &modules, &data_value);

        set.engine = Mutex::new(Some(engine));
        set.vms = Mutex::new(vms);
        set.modules = modules;
        set
    }

    /// Compile the shipped baseline alone.
    pub fn compile_baseline(data: PolicyData) -> Self {
        Self::compile([baseline_source()], data)
    }

    /// Cap the denial ring. Allows are always counted, never stored.
    pub fn with_audit_capacity(self, capacity: usize) -> Self {
        self.audit.set_capacity(capacity);
        self
    }

    /// Set the locale the refusal text uses (`zh…` selects Chinese).
    pub fn with_lang(mut self, lang: impl Into<String>) -> Self {
        self.lang = lang.into();
        self
    }

    /// The locale this set's refusals are written in.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// The data document this set was compiled with.
    pub fn data(&self) -> &PolicyData {
        &self.data
    }

    /// Why the set is in its failure mode, when it is.
    pub fn error(&self) -> Option<&PolicyError> {
        self.failure.as_ref()
    }

    /// Which evaluator answered: `"bytecode"` when every module lowered to an
    /// RVM program, `"interpreter"` when the set fell back.
    pub fn evaluator(&self) -> &'static str {
        match self.vms.lock() {
            Ok(guard) if guard.is_some() => "bytecode",
            _ => "interpreter",
        }
    }

    /// Every declared rule id, in source order, deduplicated.
    pub fn rule_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for module in &self.modules {
            for id in &module.rule_ids {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
        ids
    }

    /// The denial ring, oldest first.
    pub fn denials(&self) -> Vec<PolicyAuditEntry> {
        self.audit.denials()
    }

    /// Empty the denial ring and reset the allow counters.
    pub fn clear_audit(&self) {
        self.audit.clear();
    }

    /// The counted allows.
    pub fn allows(&self) -> super::audit::AllowCounts {
        self.audit.allows()
    }

    /// Every denial ever counted, including ones the ring has dropped.
    pub fn denials_total(&self) -> u64 {
        self.audit.denials_total()
    }

    /// The current denial-ring capacity.
    pub fn audit_capacity(&self) -> usize {
        self.audit.capacity()
    }

    /// A refusal in this set's locale, for a decision it produced.
    pub fn refusal(&self, decision: &Decision) -> String {
        decision.refusal(&self.lang)
    }

    /// Ask the compiled policy one question.
    ///
    /// Never panics. An engine error, a missing verdict or a poisoned lock all
    /// map to the failure asymmetry: deny writes and egress, allow a read
    /// inside the workspace root. Every decision is recorded in the audit.
    pub fn check(&self, input: &PolicyInput) -> Decision {
        let evaluated = self.evaluate(input);
        let decision = evaluated.decision;
        // ADVISORY is applied HERE, at the one point every tool's question
        // passes, and only to a module the host marked advisory (the preset's
        // own contributions). The engine's verdict is unchanged and the
        // refusal is recorded verbatim; the call simply proceeds with the
        // refusal in its reason so a caller can surface it. The floor modules
        // are never advisory, so nothing a preset says can widen the session,
        // the workspace policy or the baseline.
        if decision.is_denied() && self.module_is_advisory(evaluated.module.as_deref(), &decision) {
            self.audit
                .record_advisory(input, &decision, &self.data.permission);
            return Decision {
                allow: true,
                rule: decision.rule.clone(),
                reason: Some(format!("advisory: {}", decision.refusal(&self.lang))),
            };
        }
        self.audit.record(input, &decision, &self.data.permission);
        decision
    }

    /// Whether the module that denied this decision is advisory.
    ///
    /// A missing module, an engine failure and a failure-asymmetry verdict are
    /// all HARD: only a rule a real module supplied can be downgraded to a
    /// warning.
    fn module_is_advisory(&self, module: Option<&str>, decision: &Decision) -> bool {
        if decision
            .rule
            .as_deref()
            .is_some_and(|rule| rule.starts_with("policy-evaluator-"))
        {
            return false;
        }
        let Some(id) = module else {
            return false;
        };
        self.modules
            .iter()
            .any(|module| module.id == id && module.advisory)
    }

    /// Every compiled module, in evaluation order, as the policy pane lists it.
    pub fn modules(&self) -> Vec<PolicyModuleInfo> {
        self.modules
            .iter()
            .map(|module| PolicyModuleInfo {
                id: module.id.clone(),
                origin: module.origin,
                trusted: module.trusted,
                package: module
                    .query
                    .strip_prefix("data.")
                    .and_then(|rest| rest.strip_suffix(".verdict"))
                    .unwrap_or(&module.query)
                    .to_string(),
                rule_ids: module.rule_ids.clone(),
            })
            .collect()
    }

    /// The advisory refusals in the denial ring, oldest first.
    ///
    /// They are recorded in the same ring as hard denials - an advisory refusal
    /// is still a refusal a reader must be able to see - and filtered here for
    /// the pane that reports what was waved through.
    pub fn advisories(&self) -> Vec<PolicyAuditEntry> {
        self.audit.advisories()
    }

    /// Every advisory refusal ever counted.
    pub fn advisories_total(&self) -> u64 {
        self.audit.advisories_total()
    }

    /// Answer a question WITHOUT recording it.
    ///
    /// A grant candidate is the host's question, not a tool action: deriving a
    /// sandbox spec asks the policy about paths and addresses no tool touched,
    /// and those answers must not fill the denial ring or the allow counters a
    /// reader is shown. Enforcement keeps using [PolicySet::check], which
    /// audits; this is the same evaluation with the ring left alone.
    ///
    /// The answer is identical in every other respect - same modules, same
    /// data, same failure asymmetry - so a spec cannot be derived from a
    /// weaker policy than the one that gates the tools.
    pub fn probe(&self, input: &PolicyInput) -> Decision {
        self.evaluate(input).decision
    }

    /// Evaluate through the interpreter even when bytecode is available.
    ///
    /// Test-only: it exists so the two backends can be compared on the same
    /// policy. It still records in the audit, like [`PolicySet::check`].
    #[cfg(test)]
    pub(crate) fn check_with_interpreter(&self, input: &PolicyInput) -> Decision {
        let input_value = input.to_rego_value(&self.data.workspace_root_text());
        let evaluated = self.evaluate_interpreter(input, input_value);
        self.audit
            .record(input, &evaluated.decision, &self.data.permission);
        evaluated.decision
    }

    fn evaluate(&self, input: &PolicyInput) -> Evaluated {
        if let Some(error) = &self.failure {
            return Evaluated::hard(self.failure_decision(input, &error.to_string()));
        }
        if self.modules.is_empty() {
            return Evaluated::hard(self.failure_decision(input, "no policy modules are compiled"));
        }
        let input_value = input.to_rego_value(&self.data.workspace_root_text());

        // Prefer the bytecode VM. It is the same compiled policy, with no JIT
        // and no per-call compilation; the interpreter below is the fallback
        // for a module the RVM compiler could not lower.
        if self.has_vms() {
            return self.evaluate_vms(input, input_value);
        }
        self.evaluate_interpreter(input, input_value)
    }

    fn has_vms(&self) -> bool {
        self.vms
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Evaluate through the bytecode programs.
    fn evaluate_vms(&self, input: &PolicyInput, input_value: Value) -> Evaluated {
        let mut vm_guard = match self.vms.lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Evaluated::hard(
                    self.failure_decision(input, "the policy VM lock is poisoned"),
                )
            }
        };
        if let Some(vms) = vm_guard.as_mut() {
            let mut allow_rule: Option<String> = None;
            for module in vms.iter_mut() {
                module.vm.set_input(input_value.clone());
                let value = match module.vm.execute_entry_point_by_name(&module.entry) {
                    Ok(value) => value,
                    Err(error) => {
                        return Evaluated::hard(
                            self.failure_decision(
                                input,
                                &PolicyError::Evaluation {
                                    policy: module.id.clone(),
                                    message: error.to_string(),
                                }
                                .to_string(),
                            ),
                        );
                    }
                };
                match verdict_from_value(&value) {
                    Ok(Verdict::Allow { rule }) => {
                        if allow_rule.is_none() {
                            allow_rule = rule;
                        }
                    }
                    Ok(Verdict::Deny { rule, reason }) => {
                        return Evaluated {
                            decision: Decision {
                                allow: false,
                                rule,
                                reason: reason.or_else(|| {
                                    Some(format!("denied by the '{}' policy", module.id))
                                }),
                            },
                            module: Some(module.id.clone()),
                        };
                    }
                    Err(message) => {
                        return Evaluated::hard(self.failure_decision(
                            input,
                            &format!(
                                "{} ({message})",
                                PolicyError::NoVerdict {
                                    policy: module.id.clone(),
                                }
                            ),
                        ));
                    }
                }
            }
            return Evaluated::hard(Decision {
                allow: true,
                rule: allow_rule,
                reason: None,
            });
        }
        drop(vm_guard);
        Evaluated::hard(self.failure_decision(input, "the bytecode programs disappeared mid-check"))
    }

    /// Evaluate through the interpreter. Used as the fallback, and by tests to
    /// compare the two backends on the same policy.
    fn evaluate_interpreter(&self, input: &PolicyInput, input_value: Value) -> Evaluated {
        let mut guard = match self.engine.lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Evaluated::hard(
                    self.failure_decision(input, "the policy engine lock is poisoned"),
                )
            }
        };
        let Some(engine) = guard.as_mut() else {
            return Evaluated::hard(self.failure_decision(input, "no policy engine is compiled"));
        };

        engine.set_input(input_value);
        let mut allow_rule: Option<String> = None;
        for module in &self.modules {
            let value = match engine.eval_rule(module.query.clone()) {
                Ok(value) => value,
                Err(error) => {
                    return Evaluated::hard(
                        self.failure_decision(
                            input,
                            &PolicyError::Evaluation {
                                policy: module.id.clone(),
                                message: error.to_string(),
                            }
                            .to_string(),
                        ),
                    );
                }
            };
            match verdict_from_value(&value) {
                Ok(Verdict::Allow { rule }) => {
                    if allow_rule.is_none() {
                        allow_rule = rule;
                    }
                }
                Ok(Verdict::Deny { rule, reason }) => {
                    return Evaluated {
                        decision: Decision {
                            allow: false,
                            rule,
                            reason: reason
                                .or_else(|| Some(format!("denied by the '{}' policy", module.id))),
                        },
                        module: Some(module.id.clone()),
                    };
                }
                Err(message) => {
                    return Evaluated::hard(self.failure_decision(
                        input,
                        &format!(
                            "{} ({message})",
                            PolicyError::NoVerdict {
                                policy: module.id.clone(),
                            }
                        ),
                    ));
                }
            }
        }
        Evaluated::hard(Decision {
            allow: true,
            rule: allow_rule,
            reason: None,
        })
    }

    fn failure_decision(&self, input: &PolicyInput, why: &str) -> Decision {
        let in_workspace_read = input.action == Action::Read
            && matches!(
                &input.target,
                Target::Path(path) if inside_root(path, self.data.workspace_root.as_path())
            );
        if in_workspace_read {
            Decision {
                allow: true,
                rule: Some("policy-evaluator-failure-read".to_string()),
                reason: Some(format!(
                    "policy evaluation failed ({why}); an in-workspace read is allowed"
                )),
            }
        } else {
            Decision {
                allow: false,
                rule: Some("policy-evaluator-failure".to_string()),
                reason: Some(format!("policy evaluation failed: {why}")),
            }
        }
    }
}

impl std::fmt::Debug for PolicySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicySet")
            .field("modules", &self.modules)
            .field("data", &self.data)
            .field("failure", &self.failure)
            .field("lang", &self.lang)
            .finish_non_exhaustive()
    }
}

/// One evaluation: the verdict a caller enforces, and the module that denied.
///
/// The module is kept OUT of [`Decision`] so the public answer stays the same
/// size it was before advisory mode existed (a larger `Decision` pushed the
/// egress error types over clippy's size budget); the host uses it only to
/// decide whether the denial was advisory.
struct Evaluated {
    decision: Decision,
    module: Option<String>,
}

impl Evaluated {
    /// A verdict that no module's advisory flag may downgrade.
    fn hard(decision: Decision) -> Self {
        Self {
            decision,
            module: None,
        }
    }
}

enum Verdict {
    Allow {
        rule: Option<String>,
    },
    Deny {
        rule: Option<String>,
        reason: Option<String>,
    },
}

fn verdict_from_value(value: &Value) -> Result<Verdict, String> {
    match value {
        Value::Bool(true) => Ok(Verdict::Allow { rule: None }),
        Value::Bool(false) => Ok(Verdict::Deny {
            rule: None,
            reason: None,
        }),
        Value::Object(fields) => {
            let allow = fields
                .get(&Value::from("allow"))
                .and_then(|value| value.as_bool().ok().copied())
                .unwrap_or(false);
            let rule = fields
                .get(&Value::from("rule"))
                .and_then(|value| value.as_string().ok())
                .map(|value| value.to_string());
            if allow {
                return Ok(Verdict::Allow { rule });
            }
            Ok(Verdict::Deny {
                rule,
                reason: fields
                    .get(&Value::from("reason"))
                    .and_then(|value| value.as_string().ok())
                    .map(|value| value.to_string()),
            })
        }
        Value::Undefined => Err("the rule is undefined for this input".to_string()),
        other => Err(format!("the verdict is not a boolean or object: {other:?}")),
    }
}

/// Lower every module to a reusable bytecode program.
///
/// One program per module; the VM's data document is installed once here and
/// only the input is replaced per check. Any lowering failure returns None, so
/// the whole set falls back to the interpreter rather than mixing evaluators.
fn compile_vms(
    engine: &mut Engine,
    modules: &[PolicyModule],
    data: &Value,
) -> Option<Vec<VmModule>> {
    let mut vms = Vec::with_capacity(modules.len());
    for module in modules {
        let entry: Rc<str> = module.query.as_str().into();
        let compiled = match engine.compile_with_entrypoint(&entry) {
            Ok(compiled) => compiled,
            Err(error) => {
                log::warn!(
                    "policy module '{}' could not be lowered to bytecode: {error}",
                    module.id
                );
                return None;
            }
        };
        let program = match RvmCompiler::compile_from_policy(&compiled, &[module.query.as_str()]) {
            Ok(program) => program,
            Err(error) => {
                log::warn!(
                    "policy module '{}' could not be compiled to a program: {error}",
                    module.id
                );
                return None;
            }
        };
        let mut vm = RegoVM::new_with_policy(compiled);
        vm.load_program(program);
        if let Err(error) = vm.set_data(data.clone()) {
            log::warn!(
                "policy module '{}' could not install its data in the VM: {error}",
                module.id
            );
            return None;
        }
        vms.push(VmModule {
            vm,
            id: module.id.clone(),
            entry: module.query.clone(),
        });
    }
    Some(vms)
}

fn module_for(source: &PolicySource) -> Result<PolicyModule, PolicyError> {
    if source.id.trim().is_empty() {
        return Err(PolicyError::InvalidSource {
            policy: source.id.clone(),
            message: "the source id is empty".to_string(),
        });
    }
    let package = package_path(&source.rego).map_err(|message| PolicyError::InvalidSource {
        policy: source.id.clone(),
        message,
    })?;
    Ok(PolicyModule {
        id: effective_module_id(source),
        origin: source.origin,
        trusted: source.trusted,
        advisory: source.advisory,
        query: format!("data.{package}.verdict"),
        rule_ids: extract_rule_ids(&source.rego),
    })
}

/// The id a source's module is compiled under.
///
/// A module may declare its own id with a `# module: <id>` header comment. That
/// is what lets a workspace file or an extension package CLAIM an id - and the
/// claim is checked against the reserved `builtin:` namespace in
/// [`PolicySet::compile`], so a user module cannot shadow the binary's floor.
/// Without the directive the host-assigned label is the id.
/// The built-in id a non-built-in module tried to claim, when it claimed one.
///
/// Every id under the reserved prefix is refused, whether or not this binary
/// currently ships it: the namespace belongs to the binary, and a future module
/// must not collide with a name a user file could already have taken.
fn builtin_id_claimed(id: &str) -> Option<String> {
    id.trim()
        .starts_with("builtin:")
        .then(|| id.trim().to_string())
}

fn effective_module_id(source: &PolicySource) -> String {
    for line in source.rego.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix('#') else {
            continue;
        };
        let Some(rest) = rest.trim().strip_prefix("module:") else {
            continue;
        };
        let id = rest.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    source.id.clone()
}

/// Read the `package a.b` declaration and return its dotted path.
fn package_path(rego: &str) -> Result<String, String> {
    for line in rego.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("package ") {
            let path = rest.trim().trim_end_matches(';').trim();
            if path.is_empty() {
                return Err("the package declaration is empty".to_string());
            }
            if !path
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
            {
                return Err(format!("'{path}' is not a simple dotted package path"));
            }
            return Ok(path.to_string());
        }
    }
    Err("the source has no 'package' declaration".to_string())
}

/// Extract every `# rule: <id>` comment, in source order.
fn extract_rule_ids(rego: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in rego.lines() {
        let Some(rest) = line.trim().strip_prefix('#') else {
            continue;
        };
        let Some(id) = rest.trim().strip_prefix("rule:") else {
            continue;
        };
        let id = id.trim();
        if !id.is_empty() && !ids.iter().any(|known| known == id) {
            ids.push(id.to_string());
        }
    }
    ids
}
