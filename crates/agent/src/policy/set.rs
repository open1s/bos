//! Compiling policy sources into one engine and evaluating a decision.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use regorus::languages::rego::compiler::Compiler as RvmCompiler;
use regorus::rvm::vm::RegoVM;
use regorus::{Engine, Rc, Value};

use super::audit::{PolicyAudit, PolicyAuditEntry};
use super::baseline::baseline_source;
use super::target::{inside_root, normalize_path, Action, Decision, PolicyInput, Target};

/// One Rego module the host supplies to [`PolicySet::compile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySource {
    /// Stable label, used as the module's file name and in audit text.
    pub id: String,
    /// The Rego source.
    pub rego: String,
}

impl PolicySource {
    /// Name one source.
    pub fn new(id: impl Into<String>, rego: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            rego: rego.into(),
        }
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
    /// Egress entries the preset allows, intersected with the workspace list.
    pub preset_egress_allowlist: Vec<String>,
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
            preset_read_restricted: false,
            preset_read_roots: Vec::new(),
            preset_write_restricted: false,
            preset_write_roots: Vec::new(),
            preset_deny_roots: Vec::new(),
            preset_permission: String::new(),
            permission: String::new(),
            preset_egress_restricted: false,
            preset_egress_allowlist: Vec::new(),
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

    /// Install the preset's egress scope. A restricted empty list denies all.
    pub fn with_preset_egress(mut self, restricted: bool, allowlist: Vec<String>) -> Self {
        self.preset_egress_restricted = restricted;
        self.preset_egress_allowlist = allowlist;
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
        root.insert(Value::from("preset_scope"), Value::from(scope));
        root.insert(
            Value::from("preset_permission"),
            Value::from(self.preset_permission.as_str()),
        );
        root.insert(
            Value::from("permission"),
            Value::from(self.permission.as_str()),
        );
        let mut egress: BTreeMap<Value, Value> = BTreeMap::new();
        egress.insert(
            Value::from("restricted"),
            Value::from(self.preset_egress_restricted),
        );
        egress.insert(
            Value::from("allowlist"),
            Value::from(roots(&self.preset_egress_allowlist)),
        );
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

/// One compiled module: what to query, and the rule ids it declares.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PolicyModule {
    id: String,
    query: String,
    rule_ids: Vec<String>,
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
        let decision = self.evaluate(input);
        self.audit.record(input, &decision, &self.data.permission);
        decision
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
        self.evaluate(input)
    }

    /// Evaluate through the interpreter even when bytecode is available.
    ///
    /// Test-only: it exists so the two backends can be compared on the same
    /// policy. It still records in the audit, like [`PolicySet::check`].
    #[cfg(test)]
    pub(crate) fn check_with_interpreter(&self, input: &PolicyInput) -> Decision {
        let input_value = input.to_rego_value(&self.data.workspace_root_text());
        let decision = self.evaluate_interpreter(input, input_value);
        self.audit.record(input, &decision, &self.data.permission);
        decision
    }

    fn evaluate(&self, input: &PolicyInput) -> Decision {
        if let Some(error) = &self.failure {
            return self.failure_decision(input, &error.to_string());
        }
        if self.modules.is_empty() {
            return self.failure_decision(input, "no policy modules are compiled");
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
    fn evaluate_vms(&self, input: &PolicyInput, input_value: Value) -> Decision {
        let mut vm_guard = match self.vms.lock() {
            Ok(guard) => guard,
            Err(_) => return self.failure_decision(input, "the policy VM lock is poisoned"),
        };
        if let Some(vms) = vm_guard.as_mut() {
            let mut allow_rule: Option<String> = None;
            for module in vms.iter_mut() {
                module.vm.set_input(input_value.clone());
                let value = match module.vm.execute_entry_point_by_name(&module.entry) {
                    Ok(value) => value,
                    Err(error) => {
                        return self.failure_decision(
                            input,
                            &PolicyError::Evaluation {
                                policy: module.id.clone(),
                                message: error.to_string(),
                            }
                            .to_string(),
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
                        return Decision {
                            allow: false,
                            rule,
                            reason: reason
                                .or_else(|| Some(format!("denied by the '{}' policy", module.id))),
                        };
                    }
                    Err(message) => {
                        return self.failure_decision(
                            input,
                            &format!(
                                "{} ({message})",
                                PolicyError::NoVerdict {
                                    policy: module.id.clone(),
                                }
                            ),
                        );
                    }
                }
            }
            return Decision {
                allow: true,
                rule: allow_rule,
                reason: None,
            };
        }
        drop(vm_guard);
        self.failure_decision(input, "the bytecode programs disappeared mid-check")
    }

    /// Evaluate through the interpreter. Used as the fallback, and by tests to
    /// compare the two backends on the same policy.
    fn evaluate_interpreter(&self, input: &PolicyInput, input_value: Value) -> Decision {
        let mut guard = match self.engine.lock() {
            Ok(guard) => guard,
            Err(_) => return self.failure_decision(input, "the policy engine lock is poisoned"),
        };
        let Some(engine) = guard.as_mut() else {
            return self.failure_decision(input, "no policy engine is compiled");
        };

        engine.set_input(input_value);
        let mut allow_rule: Option<String> = None;
        for module in &self.modules {
            let value = match engine.eval_rule(module.query.clone()) {
                Ok(value) => value,
                Err(error) => {
                    return self.failure_decision(
                        input,
                        &PolicyError::Evaluation {
                            policy: module.id.clone(),
                            message: error.to_string(),
                        }
                        .to_string(),
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
                    return Decision {
                        allow: false,
                        rule,
                        reason: reason
                            .or_else(|| Some(format!("denied by the '{}' policy", module.id))),
                    };
                }
                Err(message) => {
                    return self.failure_decision(
                        input,
                        &format!(
                            "{} ({message})",
                            PolicyError::NoVerdict {
                                policy: module.id.clone(),
                            }
                        ),
                    );
                }
            }
        }
        Decision {
            allow: true,
            rule: allow_rule,
            reason: None,
        }
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
        id: source.id.clone(),
        query: format!("data.{package}.verdict"),
        rule_ids: extract_rule_ids(&source.rego),
    })
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
