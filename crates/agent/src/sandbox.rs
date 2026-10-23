//! One sandbox specification for every confined process an agent turn starts.
//!
//! # Why one type
//!
//! The shell tool and the right-pane terminal are two processes with one
//! question - what may this turn touch - and had two answers before this
//! module: the tool built a Seatbelt profile from a root path, the terminal
//! spawned a bare PTY. [SandboxSpec] is that answer, built once from the
//! effective permission and the regorus policy, and consumed by both spawn
//! paths, so the two cannot diverge by construction.
//!
//! # The mapping
//!
//! | permission | backend | working dir | workspace | build cache | host temp | .git / .bsh | egress |
//! |---|---|---|---|---|---|---|---|
//! | read-only | heel | private scratch (rw, no exec) | read (policy read scope) | not granted | denied | denied by the policy tier | allowlist |
//! | workspace-write | heel | private scratch (rw, no exec) | read+write (policy write scope) | write+exec when the policy permits the write | denied | denied by the policy tier | allowlist |
//! | danger-full-access | none | the workspace | full | full | full | writable | unrestricted |
//!
//! Three entries in that table are deliberate and each one is measured:
//!
//! - **The working directory is a private scratch, not the workspace.** heel's
//!   profile grants its working directory write and then denies execute under
//!   it, and those rules are emitted AFTER every configured grant. A build
//!   cache inside the working directory therefore can never be executed, so
//!   "cargo test" - which writes a binary under target/ and runs it
//!   immediately - cannot work when the workspace is the working directory.
//!   Making the working directory a scratch and the workspace a grant is what
//!   lets the cache be the one writable-and-executable path. The command still
//!   starts in the workspace through Command::current_dir, and TMPDIR points at
//!   the scratch, so temp files never land in the repository.
//! - **.git and .bsh cannot be denied by the kernel.** heel's model has one
//!   grant per path and no deny; a grant naming the workspace covers its whole
//!   tree, and granting each child instead would forbid creating a new file at
//!   the workspace root. The deny stays a policy-tier decision (the baseline's
//!   write-forbidden-directory rule), recorded in [SandboxSpec::denied_writes]
//!   and enforced by the shell tool before it spawns. A write reached through a
//!   shell variable is out of that tier's reach. The in-tree Seatbelt profile
//!   (the heel-sandbox-off build) DOES deny them in the kernel, which is the
//!   concrete reason the fallback is kept.
//! - **Full access is no sandbox at all**, not a permissive sandbox: the
//!   permission already means the reader released confinement, and wrapping it
//!   in heel's permissive level would only make the release slower.
//!
//! # What regorus decides
//!
//! The grant list is DERIVED, not written down. [SandboxSpec::for_policy] asks
//! the compiled policy one question per candidate, with the same [PolicyInput]
//! document every tool builds, and records every answer:
//!
//! | candidate | question | consequence |
//! |---|---|---|
//! | workspace write | write on the workspace write probe | writable grant, or none |
//! | preset write scope | write on each scoped root | writable grant per root the policy allows |
//! | workspace / scoped read | read on the workspace and each read root | readable grant, or none |
//! | .git, .bsh, preset deny roots | write on the path | denied-writes entry |
//! | host temp | write on the temp probe | host temp writable, or denied |
//! | build cache | write under the cache | write+exec grant, or none |
//! | toolchain | read on each candidate | grant, unless a narrowing module refused it |
//! | egress | egress on each allowlist entry | a proxy exists at all, or kernel deny |
//!
//! Two properties of that derivation matter:
//!
//! - Every question is a PROBE, not an audited check. A grant candidate is the
//!   host's question, and every turn asks about .git, .bsh and the host temp
//!   dir; auditing those would fill the denial ring with paths no tool touched.
//!   The answer is the same policy's answer, so a spec can never be derived from
//!   a weaker policy than the one that gates the tools.
//! - The preset's own scopes are CANDIDATES, not branches taken here. A preset
//!   that confines writes to one directory comes out as "the workspace was asked
//!   about and refused, the scoped root was allowed", and the kernel grant list
//!   follows the policy's answer rather than a flag read beside it.
//!
//! The egress half has a live arm as well: when the derived set is not empty,
//! the shell runs under a proxy whose policy is the RegoEgress type below,
//! which asks the same policy PER CONNECTION for the address it resolved. The
//! static set decides whether a proxy exists; the live callback decides each
//! request.
//!
//! # The terminal
//!
//! The right pane's shell runs under the SAME spec. [SandboxSpec::spawn_terminal]
//! starts it with the pane's tty granted, which is what makes an interactive
//! shell possible inside the sandbox: the shell opens the caller's tty by path
//! (portable-pty exposes the slave's name, not its fd) and the profile grants
//! that device write and ioctl. Measured end to end: a real prompt, echo, line
//! editing, a resize the shell reports back, an outside write refused by the
//! kernel, and exit. A shell that cannot run without a controlling terminal
//! (fish) is replaced, and the pane says so in one line.
//!
//! # What this module cannot do
//!
//! A .git or .bsh write reached through a shell variable is refused by the
//! policy tier only where the command line names the path; heel has no deny
//! inside a granted tree, so the kernel does not stop it. That is why the
//! heel-sandbox-off build still exists. On a backend that cannot grant the pty
//! slave (heel's Linux backend), [TerminalSandbox::PtyNotHostable] says so
//! instead of implying a confinement.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::policy::{Action, Decision, PolicyInput, PolicySet, Target};

/// The probe name used to ask the policy about a directory write.
///
/// A write question needs a path; naming a file that is never created keeps the
/// question concrete without touching the filesystem.
const WRITE_PROBE: &str = ".bsh-sandbox-probe";

/// Directory names inside the workspace that are build caches.
///
/// These are the paths a build writes to and then executes from, so they are
/// the only candidates for a write+execute grant. Everything else writable is
/// denied execute, which is the write-then-execute guarantee.
const BUILD_CACHES: &[&str] = &["target", "node_modules", ".venv"];

/// Toolchain paths a shell needs in order to run a build.
///
/// Reads are opt-in above heel's baseline. System paths stay readable in the
/// baseline and /usr/bin stays executable, so only what lives under the user's
/// home - a Rust or Node toolchain - has to be named. The list is filtered to
/// paths that exist (heel refuses a grant naming a missing path) and to paths
/// the policy does not narrow away.
const TOOLCHAIN_PATHS: &[ToolchainPath] = &[
    ToolchainPath::home(".cargo/bin", true),
    ToolchainPath::home(".cargo/registry", false),
    ToolchainPath::home(".cargo/git", false),
    ToolchainPath::home(".rustup", true),
    ToolchainPath::home(".local/bin", true),
    ToolchainPath::home(".bun/bin", true),
    ToolchainPath::home(".nvm", true),
    ToolchainPath::absolute("/opt/homebrew", true),
    ToolchainPath::absolute("/opt/local", true),
    ToolchainPath::absolute("/usr/local", true),
    ToolchainPath::absolute("/usr/share", false),
    ToolchainPath::absolute("/Library/Developer/CommandLineTools", true),
];

/// One toolchain candidate, absolute or relative to the home directory.
enum ToolchainPath {
    /// A path under the user's home.
    Home(&'static str, bool),
    /// An absolute path.
    Absolute(&'static str, bool),
}

impl ToolchainPath {
    /// A candidate under the home directory. The flag asks for read+execute
    /// rather than read only, which is what running a toolchain binary needs.
    const fn home(path: &'static str, executable: bool) -> Self {
        Self::Home(path, executable)
    }

    /// An absolute candidate.
    const fn absolute(path: &'static str, executable: bool) -> Self {
        Self::Absolute(path, executable)
    }

    /// The concrete path, or none when the home directory is unknown.
    fn resolve(&self) -> Option<(PathBuf, bool)> {
        match self {
            Self::Home(path, executable) => Some((dirs::home_dir()?.join(path), *executable)),
            Self::Absolute(path, executable) => Some((PathBuf::from(path), *executable)),
        }
    }
}

/// Environment variables the sandbox must set, never inherit.
///
/// TMPDIR is the sandbox's own scratch and a forwarded value would point it
/// back at a directory the policy denies. The proxy variables are the
/// confinement's egress path: an inherited HTTP_PROXY would route around the
/// sandbox's own proxy and the policy behind it.
#[cfg_attr(not(feature = "heel-sandbox"), allow(dead_code))]
const RESERVED_ENV: &[&str] = &[
    "TMPDIR",
    "TEMP",
    "TMP",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// The permission tier a spec was built for.
///
/// These are the three values the settings tree carries, resolved to the
/// effective level (the session's request capped by the preset's ceiling)
/// before they reach this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    /// Nothing may be written.
    ReadOnly,
    /// Writes are confined to the workspace and its build caches.
    WorkspaceWrite,
    /// No confinement: the reader released it.
    FullAccess,
}

impl SandboxMode {
    /// The mode an effective permission string names.
    ///
    /// Unknown text resolves to [SandboxMode::ReadOnly]: a typo must not select
    /// the unconfined build.
    pub fn from_permission(permission: &str) -> Self {
        match permission.trim() {
            "workspace-write" => Self::WorkspaceWrite,
            "danger-full-access" => Self::FullAccess,
            _ => Self::ReadOnly,
        }
    }

    /// The stable name, for a log line or an audit row.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::FullAccess => "danger-full-access",
        }
    }

    /// Whether this mode confines anything at all.
    pub fn is_confined(&self) -> bool {
        *self != Self::FullAccess
    }
}

/// What a grant candidate asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantKind {
    /// Write on the workspace tree.
    WorkspaceWrite,
    /// Read on the workspace tree, for a tier that may not write.
    WorkspaceRead,
    /// Write and execute on a build cache.
    BuildCache,
    /// The host's temporary directory.
    HostTemp,
    /// A path the policy refuses writes to, such as .git.
    ForbiddenWrite,
    /// A toolchain path a command needs to run.
    Toolchain,
    /// One egress allowlist entry.
    Egress,
}

impl GrantKind {
    /// The stable name, for a log line or a test.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspace-write",
            Self::WorkspaceRead => "workspace-read",
            Self::BuildCache => "build-cache",
            Self::HostTemp => "host-temp",
            Self::ForbiddenWrite => "forbidden-write",
            Self::Toolchain => "toolchain",
            Self::Egress => "egress",
        }
    }
}

/// One candidate the policy was asked about, and what it answered.
///
/// The whole list is kept on the spec rather than discarded after use: a
/// refusal that cannot name the rule behind it is indistinguishable from a
/// grant that was never asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantDecision {
    /// What was asked about.
    pub kind: GrantKind,
    /// The concrete path or address, as the policy saw it.
    pub subject: String,
    /// Whether the policy allowed it.
    pub allowed: bool,
    /// The rule that answered, when the policy named one.
    pub rule: Option<String>,
}

impl GrantDecision {
    /// A one-line rendering: kind, subject and the rule that decided it.
    pub fn describe(&self) -> String {
        let verdict = if self.allowed { "allow" } else { "deny" };
        match self.rule.as_deref() {
            Some(rule) => format!("{} {} {verdict} ({rule})", self.kind.as_str(), self.subject),
            None => format!("{} {} {verdict}", self.kind.as_str(), self.subject),
        }
    }
}

/// The network a sandbox may reach.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Egress {
    /// Nothing, enforced by the kernel with no proxy running at all.
    #[default]
    DenyAll,
    /// The rules the policy allowlisted: addresses (and CIDRs) plus hostnames.
    ///
    /// A non-empty set starts heel's proxy, which asks the RegoEgress policy
    /// per connection with the host, the resolved address AND the port; the set
    /// itself is used only to decide that a proxy exists, so its contents are
    /// the rationale rather than the enforcement.
    Allow(Vec<String>),
}

impl Egress {
    /// Whether any address is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow(entries) if !entries.is_empty())
    }

    /// The allowlisted entries.
    pub fn entries(&self) -> &[String] {
        match self {
            Self::DenyAll => &[],
            Self::Allow(entries) => entries,
        }
    }
}

/// Who is asking, for the policy questions the spec builds.
///
/// The principal is the same four facts every tool sends: the session, the
/// preset, the tool name and the action. The spec fixes the tool (it is the
/// shell or the terminal) and carries the other three.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Principal {
    /// The session id, empty when the caller has none.
    pub session: String,
    /// The preset id in effect.
    pub preset: String,
    /// The tool the spec is for, for example "bash".
    pub tool: String,
}

impl Principal {
    /// The principal of one tool in one session under one preset.
    pub fn new(
        tool: impl Into<String>,
        session: impl Into<String>,
        preset: impl Into<String>,
    ) -> Self {
        Self {
            session: session.into(),
            preset: preset.into(),
            tool: tool.into(),
        }
    }
}

/// Everything one confined process may do, decided once.
///
/// Build it with [SandboxSpec::for_policy] when a compiled policy is available
/// (the normal path) or with [SandboxSpec::workspace_write] when only a root is
/// (the shell tool's public constructor). The fields are read by the spawn path
/// in this module and by callers that want to explain themselves; see the
/// module documentation for the mapping table.
#[derive(Clone)]
pub struct SandboxSpec {
    mode: SandboxMode,
    root: PathBuf,
    principal: Principal,
    /// The policy every grant was derived from, held for the live egress
    /// callback. None for a spec built without one.
    policy: Option<Arc<PolicySet>>,
    writable: Vec<PathBuf>,
    writable_executable: Vec<PathBuf>,
    executable: Vec<PathBuf>,
    readable: Vec<PathBuf>,
    denied_writes: Vec<PathBuf>,
    egress: Egress,
    decisions: Vec<GrantDecision>,
}

impl std::fmt::Debug for SandboxSpec {
    /// Render the spec without its policy: the grants are the fact a reader
    /// needs, and a compiled engine has no useful Debug form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxSpec")
            .field("mode", &self.mode)
            .field("root", &self.root)
            .field("tool", &self.principal.tool)
            .field("writable", &self.writable)
            .field("writable_executable", &self.writable_executable)
            .field("executable", &self.executable)
            .field("readable", &self.readable)
            .field("denied_writes", &self.denied_writes)
            .field("egress", &self.egress)
            .field("decisions", &self.decisions.len())
            .finish()
    }
}

impl SandboxSpec {
    /// The spec for one permission, derived from a compiled policy.
    ///
    /// Every candidate is asked as the principal's tool, so a preset scope or a
    /// session tier narrows the grant list exactly as it narrows a tool call.
    pub fn for_policy(policy: &Arc<PolicySet>, permission: &str, principal: Principal) -> Self {
        let mode = SandboxMode::from_permission(permission);
        let root = policy.data().workspace_root.clone();
        let mut spec = Self {
            mode,
            root: root.clone(),
            principal,
            policy: Some(Arc::clone(policy)),
            writable: Vec::new(),
            writable_executable: Vec::new(),
            executable: Vec::new(),
            readable: Vec::new(),
            denied_writes: Vec::new(),
            egress: Egress::DenyAll,
            decisions: Vec::new(),
        };
        if mode == SandboxMode::FullAccess || root.as_os_str().is_empty() {
            return spec;
        }
        // Two spellings, on purpose: every QUESTION uses the policy's own root
        // (a policy compares prefixes, so a canonicalized path against a
        // symlinked root answers "outside the workspace"), while every GRANT
        // uses the resolved path the kernel matches profiles against.
        let asked_root = root.clone();
        let root = canonical(&root);

        // The preset's scopes are policy DATA, and the kernel grants follow
        // them: a preset that confines writes to one directory must not be handed
        // the whole workspace, and a preset that confines reads to one directory
        // must not have the whole tree readable inside the sandbox. Both are read
        // here and each candidate is still asked, so the modules that narrow the
        // answer keep the last word.
        // 1. The writable roots. The workspace is always a CANDIDATE, and the
        //    preset's write scope adds its roots to the same list: whether any of
        //    them is granted is the policy's answer, never a decision taken here
        //    from the data flags. A restricted scope therefore comes out as "the
        //    workspace was asked about and refused", which is what the rationale
        //    then shows.
        let mut write_candidates: Vec<PathBuf> = vec![asked_root.clone()];
        if policy.data().preset_write_restricted {
            write_candidates.extend(policy.data().preset_write_roots.iter().map(PathBuf::from));
        }
        for candidate in &write_candidates {
            // heel refuses a grant naming a path that does not resolve, so a
            // scope naming a directory that does not exist yet is not granted:
            // the sandbox fails CLOSED there and the policy tier still refuses
            // the write, which is visible rather than silently widened.
            if !candidate.exists() {
                continue;
            }
            let probe = candidate.join(WRITE_PROBE);
            let decision = spec.ask(policy, Action::Write, Target::path(&probe));
            spec.record(GrantKind::WorkspaceWrite, canonical(candidate), &decision);
            if decision.is_allowed() && mode == SandboxMode::WorkspaceWrite {
                spec.writable.push(canonical(candidate));
            }
        }

        // 2. The readable roots, asked the same way: the workspace plus the
        //    preset's read scope when it declares one. A writable root is not
        //    repeated (write implies read), and a read-only tier needs this to
        //    see its tree at all: heel's strict mode denies the user's home.
        let mut read_candidates: Vec<PathBuf> = vec![asked_root.clone()];
        if policy.data().preset_read_restricted {
            read_candidates.extend(policy.data().preset_read_roots.iter().map(PathBuf::from));
        }
        for candidate in &read_candidates {
            if !candidate.exists() {
                continue;
            }
            let resolved = canonical(candidate);
            if spec
                .writable
                .iter()
                .any(|writable| resolved.starts_with(writable))
            {
                continue;
            }
            let decision = spec.ask(policy, Action::Read, Target::path(candidate));
            spec.record(GrantKind::WorkspaceRead, resolved.clone(), &decision);
            if decision.is_allowed() {
                spec.readable.push(resolved);
            }
        }

        // 3. The directories the policy forbids: the baseline's .git/.bsh, plus
        //    every root the preset denies. Asked, not assumed: a policy that
        //    allows them gets the same answer here that a tool call would get.
        let mut forbidden: Vec<PathBuf> = vec![asked_root.join(".git"), asked_root.join(".bsh")];
        forbidden.extend(policy.data().preset_deny_roots.iter().map(PathBuf::from));
        for asked in forbidden {
            let decision = spec.ask(policy, Action::Write, Target::path(asked.join(WRITE_PROBE)));
            let denied_path = canonical(&asked);
            spec.record(GrantKind::ForbiddenWrite, denied_path.clone(), &decision);
            if decision.is_denied() {
                spec.denied_writes.push(denied_path);
            }
        }

        // 3. The host's temporary directory. Denied under Workspace Write, so
        //    the sandbox's scratch is the only temp a command gets.
        let temp_probe = std::env::temp_dir().join(WRITE_PROBE);
        let decision = spec.ask(policy, Action::Write, Target::path(&temp_probe));
        spec.record(GrantKind::HostTemp, temp_probe, &decision);
        let temp_writable = decision.is_allowed() && mode == SandboxMode::WorkspaceWrite;

        // 4. Build caches: the one place write-then-execute is given up, and
        //    only where the policy permits the write.
        for name in BUILD_CACHES {
            let path = root.join(name);
            if !path.is_dir() {
                continue;
            }
            let probe = asked_root.join(name).join(WRITE_PROBE);
            let decision = spec.ask(policy, Action::Write, Target::path(&probe));
            spec.record(GrantKind::BuildCache, path.clone(), &decision);
            if decision.is_allowed() && mode == SandboxMode::WorkspaceWrite {
                spec.writable_executable.push(path);
            }
        }
        if temp_writable {
            spec.writable.push(std::env::temp_dir());
        }

        // 5. The toolchain. A read question the baseline refuses by design (it
        //    confines every tool read to the workspace), so only a NARROWING
        //    refusal removes a candidate; see narrowed_read.
        for candidate in TOOLCHAIN_PATHS {
            let Some((path, executable)) = candidate.resolve() else {
                continue;
            };
            if !path.exists() {
                continue;
            }
            let decision = spec.ask(policy, Action::Read, Target::path(&path));
            let path = canonical(&path);
            spec.record(GrantKind::Toolchain, path.clone(), &decision);
            if narrowed_read(&decision) {
                continue;
            }
            if executable {
                spec.executable.push(path);
            } else {
                spec.readable.push(path);
            }
        }

        // 6. Egress. Every rule the data carries is a CANDIDATE: an address
        //    entry is asked as its own address (the strong door), a hostname
        //    entry as a name with no address (the weak door, which the policy
        //    names as such). The port list is a constraint on both, so a probe
        //    uses one of its ports - the policy's port rule then answers exactly
        //    as it will per request. What survives is the set that makes a proxy
        //    exist at all; each request is still decided live by RegoEgress.
        let data = policy.data();
        let probe_port = data.egress_ports.first().copied();
        let mut allowed: Vec<String> = Vec::new();
        for entry in &data.egress_allowlist {
            let address = entry.split('/').next().unwrap_or(entry).to_string();
            let target = match probe_port {
                Some(port) => Target::host_on(entry, &address, port),
                None => Target::host(entry, &address),
            };
            let decision = spec.ask(policy, Action::Egress, target);
            spec.record(GrantKind::Egress, entry.clone(), &decision);
            if decision.is_allowed() {
                allowed.push(entry.clone());
            }
        }
        for entry in &data.egress_hosts {
            let target = match probe_port {
                Some(port) => Target::host_on(entry, "", port),
                None => Target::host(entry, ""),
            };
            let decision = spec.ask(policy, Action::Egress, target);
            spec.record(GrantKind::Egress, entry.clone(), &decision);
            if decision.is_allowed() {
                allowed.push(entry.clone());
            }
        }
        if !allowed.is_empty() {
            spec.egress = Egress::Allow(allowed);
        }
        spec
    }

    /// The default Workspace Write spec for a root, with no policy.
    ///
    /// This is what the shell tool's public with_sandbox constructor builds:
    /// the same grant set the policy-derived default produces, minus the
    /// questions. A caller that has a compiled policy should use
    /// [SandboxSpec::for_policy] so the policy - not this default - decides.
    pub fn workspace_write(root: impl Into<PathBuf>) -> Self {
        let root = canonical(&root.into());
        let mut spec = Self {
            mode: SandboxMode::WorkspaceWrite,
            root: root.clone(),
            principal: Principal::default(),
            policy: None,
            writable: vec![root.clone()],
            writable_executable: Vec::new(),
            executable: Vec::new(),
            readable: Vec::new(),
            denied_writes: vec![root.join(".git"), root.join(".bsh")],
            egress: Egress::DenyAll,
            decisions: Vec::new(),
        };
        for name in BUILD_CACHES {
            let path = root.join(name);
            if path.is_dir() {
                spec.writable_executable.push(path);
            }
        }
        for candidate in TOOLCHAIN_PATHS {
            let Some((path, executable)) = candidate.resolve() else {
                continue;
            };
            if !path.exists() {
                continue;
            }
            let path = canonical(&path);
            if executable {
                spec.executable.push(path);
            } else {
                spec.readable.push(path);
            }
        }
        spec
    }

    /// The spec for a permission with no policy at all.
    ///
    /// Only Full access and Read only are reachable this way; a caller that
    /// wants a workspace write grant must say what its policy permits, which is
    /// what [SandboxSpec::workspace_write] or a policy-derived spec does.
    pub fn bare(mode: SandboxMode, root: impl Into<PathBuf>) -> Self {
        let root = canonical(&root.into());
        let readable = if mode == SandboxMode::ReadOnly {
            vec![root.clone()]
        } else {
            Vec::new()
        };
        Self {
            mode,
            root,
            principal: Principal::default(),
            policy: None,
            readable,
            writable: Vec::new(),
            writable_executable: Vec::new(),
            executable: Vec::new(),
            denied_writes: Vec::new(),
            egress: Egress::DenyAll,
            decisions: Vec::new(),
        }
    }

    /// Ask the policy one question as this spec's principal.
    ///
    /// PROBE, not check: a grant candidate is the host's question, and putting
    /// it through the auditing path would fill the denial ring with paths no
    /// tool touched (every turn asks about .git, .bsh and the host temp dir).
    /// The answer is the same policy's answer; only the audit row is skipped.
    fn ask(&self, policy: &PolicySet, action: Action, target: Target) -> Decision {
        let input = PolicyInput::new(self.principal.tool.clone(), action, target)
            .with_session(self.principal.session.clone())
            .with_preset(self.principal.preset.clone());
        policy.probe(&input)
    }

    /// Keep one answer, so the spec can explain itself later.
    fn record(&mut self, kind: GrantKind, subject: impl Into<PathBuf>, decision: &Decision) {
        self.decisions.push(GrantDecision {
            kind,
            subject: subject.into().to_string_lossy().to_string(),
            allowed: decision.is_allowed(),
            rule: decision.rule.clone(),
        });
    }

    /// The permission tier the spec was built for.
    pub fn mode(&self) -> SandboxMode {
        self.mode
    }

    /// The workspace root the spec confines.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The principal every question was asked as.
    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    /// The policy the grants were derived from, when there was one.
    pub fn policy(&self) -> Option<&Arc<PolicySet>> {
        self.policy.as_ref()
    }

    /// Every answer the policy gave, rendered one per line.
    ///
    /// A refusal that cannot name the rule behind it is indistinguishable from
    /// a grant nobody asked for, so this is what the pane and the log show.
    pub fn rationale(&self) -> Vec<String> {
        self.decisions.iter().map(GrantDecision::describe).collect()
    }

    /// Paths granted write (never execute).
    pub fn writable(&self) -> &[PathBuf] {
        &self.writable
    }

    /// Paths granted write AND execute: the build caches.
    ///
    /// Granting one gives up the write-then-execute guarantee for that path
    /// alone, which is what a build cache needs and why the list is derived
    /// from a policy question rather than being a constant.
    pub fn writable_executable(&self) -> &[PathBuf] {
        &self.writable_executable
    }

    /// Paths granted execute (and read) without write.
    pub fn executable(&self) -> &[PathBuf] {
        &self.executable
    }

    /// Paths granted read without write.
    pub fn readable(&self) -> &[PathBuf] {
        &self.readable
    }

    /// Paths the policy refuses writes to.
    ///
    /// Enforced before a process exists: a command whose own write target is
    /// one of these is refused, and the spec is also what a caller without the
    /// policy checks against.
    pub fn denied_writes(&self) -> &[PathBuf] {
        &self.denied_writes
    }

    /// The egress the spec permits.
    pub fn egress(&self) -> &Egress {
        &self.egress
    }

    /// Every candidate the policy was asked about.
    pub fn decisions(&self) -> &[GrantDecision] {
        &self.decisions
    }

    /// Whether this spec confines anything.
    pub fn is_confined(&self) -> bool {
        self.mode.is_confined()
    }

    /// The deny decision covering a path, when the policy forbids writing it.
    ///
    /// Every spelling of the path is compared, because a write target usually
    /// does not exist yet: /var/folders and /private/var/folders are the same
    /// directory and a repo under a symlinked temp root must not slip past a
    /// deny that names the other spelling.
    pub fn denies_write(&self, path: &Path) -> Option<&GrantDecision> {
        let spellings = write_spellings(path);
        self.decisions.iter().find(|decision| {
            if decision.kind != GrantKind::ForbiddenWrite || decision.allowed {
                return false;
            }
            let denied = Path::new(&decision.subject);
            spellings
                .iter()
                .any(|candidate| candidate.starts_with(denied))
        })
    }

    /// What the terminal pane may do with this spec.
    ///
    /// Measured, not assumed. heel's own interactive entry point cannot be used:
    /// Sandbox::run_interactive allocates its OWN pty and relays this process's
    /// stdin/stdout, with no way to hand it the pane's terminal and no
    /// current_dir. What DOES work is a tool heel already offers: a normal
    /// Command whose standard streams are the null device, with the shell
    /// opening the pane's tty BY PATH (portable-pty exposes the slave's name,
    /// not its fd). heel's profile grants the tty write and ioctl when
    /// allow_tty_write is on, so the shell is interactive AND confined.
    /// Measured end to end: a real zsh prompt, echo, line editing, a resize the
    /// shell reports back, an outside write refused by the kernel, and exit.
    pub fn terminal(&self) -> TerminalSandbox {
        if !self.is_confined() {
            return TerminalSandbox::Unconfined;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // Landlock grants path by path, and heel's Linux backend does not
            // name the pty slave, so the tty-by-path door is not open there.
            TerminalSandbox::PtyNotHostable {
                reason: "this platform's backend does not grant the pane's pty slave",
            }
        }
        #[cfg(any(not(unix), target_os = "macos"))]
        {
            TerminalSandbox::Sandboxed
        }
    }
}

/// What the terminal pane may do with a spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSandbox {
    /// The permission released confinement, so the shell runs bare.
    Unconfined,
    /// The spec confines the shell, and the pane's tty is reachable from inside
    /// the sandbox: the shell runs interactive AND confined.
    Sandboxed,
    /// The spec confines the shell, but this backend cannot host a PTY the
    /// caller already owns. The pane states this instead of implying a confine.
    PtyNotHostable {
        /// Why, in the backend's own terms.
        reason: &'static str,
    },
}

impl TerminalSandbox {
    /// Why the shell is not confined, when it is not.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Unconfined | Self::Sandboxed => None,
            Self::PtyNotHostable { reason } => Some(reason),
        }
    }

    /// Whether the pane's shell runs inside the sandbox.
    pub fn is_sandboxed(&self) -> bool {
        matches!(self, Self::Sandboxed)
    }
}

/// Canonicalize a path when it exists, and lexically normalize it otherwise.
///
/// heel refuses a grant naming a path that does not resolve, and macOS matches
/// profiles against resolved paths, so /tmp/x must reach it as /private/tmp/x.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| canonical_lexical(path))
}

/// Absolute and lexically normalized, without touching the filesystem.
fn canonical_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(component.as_os_str())
            }
        }
    }
    out
}

/// Every spelling of a write target a comparison might see.
///
/// A target that does not exist yet cannot be canonicalized, so its directory
/// is resolved instead: that is the spelling the kernel and the profile use.
fn write_spellings(path: &Path) -> Vec<PathBuf> {
    let lexical = canonical_lexical(path);
    let mut spellings = vec![lexical.clone()];
    if let Ok(resolved) = path.canonicalize() {
        spellings.push(resolved);
    }
    if let Some(parent) = lexical.parent() {
        if let Ok(directory) = parent.canonicalize() {
            spellings.push(directory.join(lexical.file_name().unwrap_or_default()));
        }
    }
    spellings
}

/// Whether a read refusal came from a module that NARROWS the baseline.
///
/// The baseline answers a read with "inside the workspace or nothing" for every
/// tool, which would remove every toolchain path from the grant list and leave
/// a shell that can start and run nothing. A preset scope or a deny list is a
/// narrowing and is honoured here; the baseline's own workspace confinement is
/// not, because a process loading a shared library is not a tool read.
fn narrowed_read(decision: &Decision) -> bool {
    decision.is_denied() && decision.rule.as_deref() != Some("read-outside-workspace")
}

//
// The spawn half.
//

/// One running process, whichever backend started it.
///
/// The sandbox that owns the process is held here rather than by the caller:
/// heel kills every process a sandbox started when the sandbox is dropped, so a
/// child that outlived its sandbox would be a process nobody can explain. The
/// shell tool moves this value into its waiter thread for a background job, and
/// the terminal pane holds one for the shell's whole life; in both cases the
/// confinement lives exactly as long as the process it confines.
pub struct SandboxedChild {
    inner: ChildKind,
    /// What started it: "heel", "seatbelt" or "none". A field rather than a
    /// function of the variant, because the fallback backend also spawns through
    /// a plain child process.
    backend: &'static str,
}

enum ChildKind {
    Plain(std::process::Child),
    #[cfg(feature = "heel-sandbox")]
    Heel {
        /// Declared before the child so it is dropped after it: dropping the
        /// engine kills the child, and the child must already be reaped.
        ///
        /// Boxed because the engine is large (a backend, a tracker and an
        /// optional proxy) while every other field here is a few words.
        _engine: Box<Engine>,
        child: heel::Child,
    },
}

impl SandboxedChild {
    /// The process id.
    pub fn id(&self) -> u32 {
        match &self.inner {
            ChildKind::Plain(child) => child.id(),
            #[cfg(feature = "heel-sandbox")]
            ChildKind::Heel { child, .. } => child.id(),
        }
    }

    /// Which backend started the process: "heel", "seatbelt" or "none".
    ///
    /// It is reported in the tool's result so a reader can tell a refused write
    /// from a command that simply failed.
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    /// Whether the process has exited, without blocking.
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match &mut self.inner {
            ChildKind::Plain(child) => child.try_wait(),
            #[cfg(feature = "heel-sandbox")]
            ChildKind::Heel { child, .. } => child
                .try_wait()
                .map_err(|error| std::io::Error::other(error.to_string())),
        }
    }

    /// Kill the process and everything it started.
    pub fn kill(&mut self) {
        match &mut self.inner {
            ChildKind::Plain(child) => {
                let _ = child.kill();
            }
            #[cfg(feature = "heel-sandbox")]
            ChildKind::Heel { child, .. } => {
                let _ = child.kill();
            }
        }
    }

    /// Block until the process exits.
    pub fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        match &mut self.inner {
            ChildKind::Plain(child) => child.wait(),
            #[cfg(feature = "heel-sandbox")]
            ChildKind::Heel { child, .. } => futures::executor::block_on(child.wait())
                .map_err(|error| std::io::Error::other(error.to_string())),
        }
    }

    /// Take the process's standard output, when it was piped.
    pub(crate) fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        match &mut self.inner {
            ChildKind::Plain(child) => child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
            #[cfg(feature = "heel-sandbox")]
            ChildKind::Heel { child, .. } => child
                .take_stdout()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
        }
    }

    /// Take the process's standard error, when it was piped.
    pub(crate) fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        match &mut self.inner {
            ChildKind::Plain(child) => child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
            #[cfg(feature = "heel-sandbox")]
            ChildKind::Heel { child, .. } => child
                .take_stderr()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
        }
    }
}

impl SandboxSpec {
    /// Start the pane's shell on a terminal the CALLER owns.
    ///
    /// This is what makes the right pane's shell run inside the same sandbox as
    /// the shell tool. heel's own interactive entry point cannot host a pty it
    /// did not allocate, but a normal heel Command can still start the shell:
    /// the shell opens the caller's tty BY PATH (portable-pty exposes the slave's
    /// name, not its fd), and the profile grants that tty's write and ioctl
    /// because the engine sets allow_tty_write. Standard streams go to the null
    /// device so nothing of the application's own stdio leaks into the shell.
    ///
    /// The command is sh -c with the shell path as $0, so a path containing
    /// spaces needs no quoting of its own.
    pub fn spawn_terminal(
        &self,
        program: &Path,
        tty: &Path,
        cwd: &Path,
        envs: &[(String, String)],
    ) -> Result<SandboxedChild, String> {
        let tty = tty.display().to_string();
        // Single-quoted: a device path has no quotes today, and the shell must
        // not get the chance to interpret one if that ever changes.
        let tty = tty.replace('\'', "'\\''");
        let script = format!("exec \"$0\" -i <'{tty}' >'{tty}' 2>'{tty}'");
        if self.mode.is_confined() {
            #[cfg(feature = "heel-sandbox")]
            {
                return self.spawn_heel_terminal(&script, program, cwd, envs);
            }
            #[cfg(not(feature = "heel-sandbox"))]
            {
                return self.spawn_terminal_seatbelt(&script, program, cwd, envs);
            }
        }
        self.spawn_terminal_plain(&script, program, cwd, envs)
    }

    /// Start the pane's shell with no sandbox at all.
    ///
    /// The same tty-by-path door, so the pane has ONE spawn shape and the
    /// difference between a released and a confined turn is only the boundary.
    fn spawn_terminal_plain(
        &self,
        script: &str,
        program: &Path,
        cwd: &Path,
        envs: &[(String, String)],
    ) -> Result<SandboxedChild, String> {
        let mut process = std::process::Command::new("/bin/sh");
        process
            .args(["-c", script])
            .arg(program.to_string_lossy().as_ref());
        process.current_dir(cwd).envs(envs.iter().cloned());
        process
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = process.spawn().map_err(|error| error.to_string())?;
        Ok(SandboxedChild {
            inner: ChildKind::Plain(child),
            backend: "none",
        })
    }

    /// Start the pane's shell under the in-tree Seatbelt profile.
    ///
    /// The heel-sandbox-off build confines the terminal too, so the pane's
    /// behaviour does not depend on which backend is compiled: the profile
    /// allows writes to a tty device and denies everything else outside the
    /// workspace.
    #[cfg_attr(feature = "heel-sandbox", allow(dead_code))]
    fn spawn_terminal_seatbelt(
        &self,
        script: &str,
        program: &Path,
        cwd: &Path,
        envs: &[(String, String)],
    ) -> Result<SandboxedChild, String> {
        let Some(profile) = seatbelt_profile(&self.root) else {
            return self.spawn_terminal_plain(script, program, cwd, envs);
        };
        let mut process = std::process::Command::new("/usr/bin/sandbox-exec");
        process
            .args(["-p", profile.as_str(), "/bin/sh", "-c", script])
            .arg(program.to_string_lossy().as_ref());
        process.current_dir(cwd).envs(envs.iter().cloned());
        process
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = process.spawn().map_err(|error| error.to_string())?;
        Ok(SandboxedChild {
            inner: ChildKind::Plain(child),
            backend: "seatbelt",
        })
    }

    /// Start the pane's shell under heel, with the tty granted.
    #[cfg(feature = "heel-sandbox")]
    fn spawn_heel_terminal(
        &self,
        script: &str,
        program: &Path,
        cwd: &Path,
        extra: &[(String, String)],
    ) -> Result<SandboxedChild, String> {
        let engine = futures::executor::block_on(build_engine(self, true))?;
        let mut envs: Vec<(String, String)> = std::env::vars()
            .filter(|(key, _)| {
                !RESERVED_ENV
                    .iter()
                    .any(|reserved| reserved.eq_ignore_ascii_case(key))
            })
            .collect();
        // The pane's own settings win, exactly as an explicit env does in heel.
        envs.extend(extra.iter().cloned());
        let child = match &engine {
            Engine::Deny(sandbox) => {
                spawn_terminal_command(sandbox.command("/bin/sh"), script, program, cwd, envs)
            }
            Engine::Rego(sandbox) => {
                spawn_terminal_command(sandbox.command("/bin/sh"), script, program, cwd, envs)
            }
        }?;
        Ok(SandboxedChild {
            inner: ChildKind::Heel {
                _engine: Box::new(engine),
                child,
            },
            backend: "heel",
        })
    }

    /// Start a shell command under this spec, with its output piped.
    ///
    /// Standard input is the null device: a command that reads an inherited
    /// stream would block until that stream closed, which for a long-lived
    /// parent means forever. The child starts in the workspace, not in the
    /// sandbox's scratch; see the module documentation for why those differ.
    pub(crate) fn spawn(&self, command: &str, cwd: &Path) -> Result<SandboxedChild, String> {
        if self.mode.is_confined() {
            #[cfg(feature = "heel-sandbox")]
            {
                return self.spawn_heel(command, cwd);
            }
            #[cfg(not(feature = "heel-sandbox"))]
            {
                return self.spawn_seatbelt(command, cwd);
            }
        }
        self.spawn_plain(command, cwd)
    }

    /// Start the command with no sandbox at all.
    fn spawn_plain(&self, command: &str, cwd: &Path) -> Result<SandboxedChild, String> {
        let mut process = std::process::Command::new("sh");
        process.args(["-c", command]);
        process.current_dir(cwd);
        configure_plain(&mut process);
        let child = process.spawn().map_err(|error| error.to_string())?;
        Ok(SandboxedChild {
            inner: ChildKind::Plain(child),
            backend: "none",
        })
    }

    /// Start the command under the in-tree Seatbelt profile.
    ///
    /// This is the heel-sandbox-off build, and it keeps today's semantics
    /// exactly: writes under the workspace, the shared temp directory and
    /// TMPDIR are allowed, and the workspace's .git and .bsh are denied in the
    /// kernel. It is a supported build, not an error path.
    #[cfg_attr(feature = "heel-sandbox", allow(dead_code))]
    fn spawn_seatbelt(&self, command: &str, cwd: &Path) -> Result<SandboxedChild, String> {
        let Some(profile) = seatbelt_profile(&self.root) else {
            return self.spawn_plain(command, cwd);
        };
        let mut process = std::process::Command::new("/usr/bin/sandbox-exec");
        process.args(["-p", profile.as_str(), "/bin/sh", "-c", command]);
        process.current_dir(cwd);
        configure_plain(&mut process);
        let child = process.spawn().map_err(|error| error.to_string())?;
        Ok(SandboxedChild {
            inner: ChildKind::Plain(child),
            backend: "seatbelt",
        })
    }

    /// Start the command under heel: the one sandbox implementation.
    ///
    /// The engine is built per call. That is one binary probe (heel checks the
    /// macOS version), a scratch directory and a profile string - small next to
    /// the command it wraps - and it keeps a process's sandbox and its grants
    /// from outliving the call that made them.
    #[cfg(feature = "heel-sandbox")]
    fn spawn_heel(&self, command: &str, cwd: &Path) -> Result<SandboxedChild, String> {
        let engine = futures::executor::block_on(build_engine(self, false))?;
        // The interface a sandboxed process gets, minus what the sandbox owns.
        let envs: Vec<(String, String)> = std::env::vars()
            .filter(|(key, _)| {
                !RESERVED_ENV
                    .iter()
                    .any(|reserved| reserved.eq_ignore_ascii_case(key))
            })
            .collect();
        let child = match &engine {
            Engine::Deny(sandbox) => {
                spawn_heel_command(sandbox.command("/bin/sh"), command, cwd, envs)
            }
            Engine::Rego(sandbox) => {
                spawn_heel_command(sandbox.command("/bin/sh"), command, cwd, envs)
            }
        }?;
        Ok(SandboxedChild {
            inner: ChildKind::Heel {
                _engine: Box::new(engine),
                child,
            },
            backend: "heel",
        })
    }
}

/// Configure the streams every spawned command shares.
fn configure_plain(process: &mut std::process::Command) {
    process
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Its own process group, so a timeout reaches the shell's children too and
    // a killed command cannot leave a grandchild holding the pipes open.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        process.process_group(0);
    }
}

/// Hand the pane's shell to the sandbox and return its child.
///
/// The shell is sh -c with the program as $0 and the tty redirect in the
/// script; the three standard streams are the null device so the application's
/// own stdio is never the pane's terminal.
#[cfg(feature = "heel-sandbox")]
fn spawn_terminal_command(
    command: heel::Command<'_>,
    script: &str,
    program: &Path,
    cwd: &Path,
    envs: Vec<(String, String)>,
) -> Result<heel::Child, String> {
    futures::executor::block_on(
        command
            .arg("-c")
            .arg(script)
            .arg(program.to_string_lossy().as_ref())
            .current_dir(cwd)
            .envs(envs)
            .stdin(heel::StdioConfig::Null)
            .stdout(heel::StdioConfig::Null)
            .stderr(heel::StdioConfig::Null)
            .spawn(),
    )
    .map_err(|error| error.to_string())
}

/// Hand one assembled heel command to the sandbox and return its child.
#[cfg(feature = "heel-sandbox")]
fn spawn_heel_command(
    command: heel::Command<'_>,
    script: &str,
    cwd: &Path,
    envs: Vec<(String, String)>,
) -> Result<heel::Child, String> {
    futures::executor::block_on(
        command
            .arg("-c")
            .arg(script)
            .current_dir(cwd)
            .envs(envs)
            .stdin(heel::StdioConfig::Null)
            .stdout(heel::StdioConfig::Piped)
            .stderr(heel::StdioConfig::Piped)
            .spawn(),
    )
    .map_err(|error| error.to_string())
}

/// The sandboxed engine, one variant per egress answer.
#[cfg(feature = "heel-sandbox")]
enum Engine {
    /// No egress at all: heel starts no proxy and the kernel denies outbound
    /// traffic, which is stronger than refusing it in userspace.
    Deny(heel::Sandbox<heel::DenyAll>),
    /// An allowlist exists, so a proxy runs and asks [RegoEgress] per request.
    Rego(heel::Sandbox<RegoEgress>),
}

/// Build the heel configuration one spec describes.
#[cfg(feature = "heel-sandbox")]
async fn build_engine(spec: &SandboxSpec, allow_tty: bool) -> Result<Engine, String> {
    use heel::{Access, Sandbox, SandboxConfig};

    // The terminal's engine is the SAME grants plus the one thing a shell on a
    // pty needs: permission to write and ioctl its own terminal device. Without
    // it the shell opens the tty and every write to it is refused, which is a
    // dead pane rather than a confined one.
    let mut builder = SandboxConfig::builder()
        .filesystem_strict(true)
        .allow_tty_write(allow_tty);
    // Order is load-bearing: SBPL resolves an operation with the last matching
    // rule, so a build cache granted write+execute must come AFTER the
    // workspace's write grant.
    for path in &spec.writable {
        builder = builder.writable(path);
    }
    for path in &spec.writable_executable {
        builder = builder.grant(path, Access::WRITE | Access::EXEC);
    }
    for path in &spec.executable {
        builder = builder.executable(path);
    }
    for path in &spec.readable {
        builder = builder.readable(path);
    }

    match &spec.egress {
        Egress::DenyAll => Sandbox::with_config(builder.build())
            .await
            .map(Engine::Deny)
            .map_err(|error| error.to_string()),
        Egress::Allow(entries) => {
            let policy = spec.policy.clone().ok_or_else(|| {
                "an egress allowlist needs the policy that derived it".to_string()
            })?;
            let network = RegoEgress::new(policy, spec.principal.clone(), entries.clone());
            Sandbox::with_config(builder.network(network).build())
                .await
                .map(Engine::Rego)
                .map_err(|error| error.to_string())
        }
    }
}

/// The egress policy a sandboxed shell runs under: regorus, asked live.
///
/// heel calls this per request with the hostname and port the process asked
/// for. The policy's unit is the RESOLVED ADDRESS, so this resolves the name
/// here and asks the same [PolicySet] the tools ask, once per address. Two
/// limits are stated rather than hidden: the lookup is blocking (it runs on a
/// proxy worker thread, and a slow resolver delays only that request), and the
/// proxy resolves the name a second time to connect, so a name that rebinds
/// between the two lookups could reach an address this check did not see. That
/// gap is the reason the static allowlist still exists: it decides whether a
/// proxy runs at all.
#[cfg(feature = "heel-sandbox")]
pub struct RegoEgress {
    policy: std::sync::Arc<PolicySet>,
    principal: Principal,
    allowlist: Vec<String>,
}

#[cfg(feature = "heel-sandbox")]
impl heel::NetworkPolicy for RegoEgress {
    const DENIES_ALL: bool = false;

    async fn check(&self, request: &heel::DomainRequest) -> bool {
        use std::net::ToSocketAddrs as _;
        let host = request.host().to_string();
        let port = request.port();
        let Ok(addresses) = (host.as_str(), port).to_socket_addrs() else {
            return false;
        };
        let mut seen = false;
        for address in addresses {
            seen = true;
            // One entry, so the policy's own intersection runs: the workspace
            // list and the preset's egress scope both have a say.
            let input = PolicyInput::new(
                self.principal.tool.clone(),
                Action::Egress,
                Target::host_on(host.clone(), address.ip().to_string(), port),
            )
            .with_session(self.principal.session.clone())
            .with_preset(self.principal.preset.clone());
            if self.policy.check(&input).is_denied() {
                return false;
            }
        }
        seen
    }
}

/// The egress allowlist this policy derived, for a caller that wants to log it.
#[cfg(feature = "heel-sandbox")]
impl RegoEgress {
    /// The policy every request is asked about, as the turn's own principal.
    ///
    /// The allowlist is carried only so the set that made a proxy necessary is
    /// readable; the decision itself is the policy's.
    pub fn new(policy: Arc<PolicySet>, principal: Principal, allowlist: Vec<String>) -> Self {
        Self {
            policy,
            principal,
            allowlist,
        }
    }

    /// The entries the spec asked the policy about.
    pub fn allowlist(&self) -> &[String] {
        &self.allowlist
    }
}

/// A macOS Seatbelt profile rooted at a workspace.
///
/// Returns none when this host has no /usr/bin/sandbox-exec, so the caller
/// falls back to the unconfined path. Writes are allowed under the root, the
/// shared temp directory and TMPDIR, and the root's .git and .bsh are denied
/// after that allowance, so a later deny wins.
#[cfg_attr(feature = "heel-sandbox", allow(dead_code))]
fn seatbelt_profile(root: &Path) -> Option<String> {
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        return None;
    }
    let root = canonical(root);
    let mut writable: Vec<PathBuf> = vec![
        root.clone(),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        std::env::temp_dir(),
    ];
    if let Some(temp) = std::env::var_os("TMPDIR").map(PathBuf::from) {
        if let Ok(canonical_temp) = temp.canonicalize() {
            writable.push(canonical_temp);
        }
        writable.push(temp);
    }
    if let Ok(canonical_temp) = std::env::temp_dir().canonicalize() {
        writable.push(canonical_temp);
    }
    writable.sort();
    writable.dedup();

    let escape = |path: &Path| {
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    };
    let mut profile =
        String::from("(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write*\n");
    for path in &writable {
        profile.push_str(&format!("  (subpath \"{}\")\n", escape(path)));
    }
    for sink in ["/dev/null", "/dev/stdout", "/dev/stderr", "/dev/tty"] {
        profile.push_str(&format!("  (literal \"{sink}\")\n"));
    }
    // The terminal pane's shell writes to the pty IT owns. macOS names those
    // devices /dev/ttysN; without this the pane's shell would be started and
    // then refused every write to its own screen.
    profile.push_str("  (regex #\"^/dev/ttys[0-9]+$\")\n");
    profile.push_str(")\n");
    // Later rules win, so these denials override the workspace allowance.
    profile.push_str(&format!(
        "(deny file-write* (subpath \"{}/.git\") (subpath \"{}/.bsh\"))\n",
        escape(&root),
        escape(&root)
    ));
    Some(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{baseline_source, PolicyData, PolicySource};

    /// A compiled policy for one root and permission, plus an egress allowlist.
    fn policy(root: &Path, permission: &str, egress: &[&str]) -> Arc<PolicySet> {
        // The workspace's own resolution, which is what bsh hands the policy.
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let data = PolicyData::new(root)
            .with_permission(permission)
            .with_egress_allowlist(egress.iter().map(|entry| entry.to_string()));
        let sources: Vec<PolicySource> = vec![
            baseline_source(),
            crate::policy::session_permission_source(permission),
        ];
        Arc::new(PolicySet::compile(sources, data))
    }

    /// A policy whose preset declares a read scope, a write scope and a deny
    /// list, with the module that enforces them.
    fn policy_with_scope(
        root: &Path,
        write_roots: &[PathBuf],
        read_roots: &[PathBuf],
        deny_roots: &[PathBuf],
    ) -> Arc<PolicySet> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let text = |roots: &[PathBuf]| -> Vec<String> {
            roots
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect()
        };
        let data = PolicyData::new(root)
            .with_permission("workspace-write")
            .with_preset_scope(
                !read_roots.is_empty(),
                text(read_roots),
                !write_roots.is_empty(),
                text(write_roots),
                text(deny_roots),
            );
        let sources: Vec<PolicySource> = vec![
            baseline_source(),
            crate::policy::session_permission_source("workspace-write"),
            crate::policy::preset_scope_source(),
        ];
        Arc::new(PolicySet::compile(sources, data))
    }

    /// A policy with all three egress rule shapes.
    fn policy_with_egress(
        root: &Path,
        addresses: &[&str],
        hosts: &[&str],
        ports: &[u16],
    ) -> Arc<PolicySet> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let data = PolicyData::new(root)
            .with_permission("workspace-write")
            .with_egress_allowlist(addresses.iter().map(|entry| entry.to_string()))
            .with_egress_hosts(hosts.iter().map(|entry| entry.to_string()))
            .with_egress_ports(ports.iter().copied());
        let sources: Vec<PolicySource> = vec![
            baseline_source(),
            crate::policy::session_permission_source("workspace-write"),
        ];
        Arc::new(PolicySet::compile(sources, data))
    }

    fn spec(root: &Path, permission: &str, egress: &[&str]) -> SandboxSpec {
        SandboxSpec::for_policy(
            &policy(root, permission, egress),
            permission,
            Principal::new("bash", "session", "preset"),
        )
    }

    fn scoped_spec(
        root: &Path,
        write_roots: &[PathBuf],
        read_roots: &[PathBuf],
        deny_roots: &[PathBuf],
    ) -> SandboxSpec {
        SandboxSpec::for_policy(
            &policy_with_scope(root, write_roots, read_roots, deny_roots),
            "workspace-write",
            Principal::new("bash", "session", "preset"),
        )
    }

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().expect("a scratch workspace")
    }

    /// Deriving a spec ASKS the policy but does not AUDIT the answers: a grant
    /// candidate is the host's question, and every turn asks about .git, .bsh
    /// and the host temp dir, so an audited derivation would fill the denial
    /// ring with paths no tool touched.
    #[test]
    fn deriving_a_spec_leaves_the_audit_ring_alone() {
        let dir = workspace();
        let policy = policy(dir.path(), "workspace-write", &[]);
        let allows = policy.allows().total;
        let denials = policy.denials().len();
        let spec = SandboxSpec::for_policy(
            &policy,
            "workspace-write",
            Principal::new("bash", "session", "preset"),
        );
        assert!(
            !spec.decisions().is_empty(),
            "the derivation really did ask questions"
        );
        assert_eq!(policy.allows().total, allows, "no allow was counted");
        assert_eq!(policy.denials().len(), denials, "no denial was recorded");
        assert_eq!(policy.denials_total(), 0, "and none is pending either");
    }

    /// A preset that confines writes to one directory narrows the KERNEL grant:
    /// the workspace is not handed writable, it is handed readable, and the
    /// scoped directory is the one the sandbox may write.
    #[test]
    fn a_preset_write_scope_narrows_the_kernel_grants() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("docs")).expect("the write scope");
        let root = dir.path().canonicalize().expect("canonical root");
        let docs = root.join("docs");
        let spec = scoped_spec(dir.path(), std::slice::from_ref(&docs), &[], &[]);

        assert_eq!(spec.writable(), std::slice::from_ref(&docs));
        assert!(
            !spec.writable().contains(&root),
            "the whole workspace was granted writable despite the scope: {:?}",
            spec.writable()
        );
        assert!(
            spec.readable().contains(&root),
            "the rest of the workspace stays readable: {:?}",
            spec.readable()
        );
        let workspace_question = spec
            .decisions()
            .iter()
            .find(|decision| {
                decision.kind == GrantKind::WorkspaceWrite
                    && decision.subject == root.display().to_string()
            })
            .expect("the workspace was still asked about");
        assert!(!workspace_question.allowed);
        assert_eq!(
            workspace_question.rule.as_deref(),
            Some("preset-write-scope"),
            "the preset's own module refused it"
        );
        let scoped = spec
            .decisions()
            .iter()
            .find(|decision| {
                decision.kind == GrantKind::WorkspaceWrite
                    && decision.subject == docs.display().to_string()
            })
            .expect("the scoped directory was asked about");
        assert!(scoped.allowed);
        assert_eq!(scoped.rule.as_deref(), Some("write-inside-workspace"));
    }

    /// A preset's deny list is a denied write in the spec too, so the tool tier
    /// refuses it before a process exists.
    #[test]
    fn a_preset_deny_root_is_a_denied_write() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("secrets")).expect("the deny root");
        let root = dir.path().canonicalize().expect("canonical root");
        let secrets = root.join("secrets");
        let spec = scoped_spec(dir.path(), &[], &[], std::slice::from_ref(&secrets));

        assert!(
            spec.denied_writes().contains(&secrets),
            "the deny root is not on the deny list: {:?}",
            spec.denied_writes()
        );
        let denial = spec
            .denies_write(&secrets.join("key.txt"))
            .expect("a write inside the deny root is refused");
        assert_eq!(denial.rule.as_deref(), Some("preset-deny-path"));
        assert!(
            spec.denies_write(&root.join("src/main.rs")).is_none(),
            "an ordinary workspace path is not refused"
        );
        // The workspace itself is still writable: the deny is the only narrowing.
        assert!(spec.writable().contains(&root));
    }

    /// A hostname rule is its own door: it admits a name the ADDRESS list does
    /// not, and the decision names the weaker rule that did it.
    #[test]
    fn a_hostname_rule_admits_a_name_the_address_list_does_not() {
        let dir = workspace();
        let policy = policy_with_egress(dir.path(), &[], &["api.example.test", "*.svc.test"], &[]);
        let decision = |host: &str, ip: &str, port: Option<u16>| {
            let target = match port {
                Some(port) => Target::host_on(host, ip, port),
                None => Target::host(host, ip),
            };
            policy.probe(
                &PolicyInput::new("bash", Action::Egress, target)
                    .with_session("session")
                    .with_preset("preset"),
            )
        };
        let exact = decision("api.example.test", "203.0.113.9", Some(443));
        assert!(exact.is_allowed());
        assert_eq!(exact.rule.as_deref(), Some("egress-host-allowlisted"));
        // Case and the root label's trailing dot are the same name.
        assert!(decision("API.Example.TEST.", "203.0.113.9", Some(443)).is_allowed());
        // A wildcard covers subdomains and nothing else.
        assert!(decision("cache.svc.test", "203.0.113.9", Some(443)).is_allowed());
        assert!(!decision("svc.test", "203.0.113.9", Some(443)).is_allowed());
        assert_eq!(
            decision("other.test", "203.0.113.9", Some(443))
                .rule
                .as_deref(),
            Some("egress-not-allowlisted")
        );
        // The spec starts a proxy for a host-only policy, or the live arm would
        // never be consulted.
        let spec = SandboxSpec::for_policy(
            &policy,
            "workspace-write",
            Principal::new("bash", "session", "preset"),
        );
        assert!(
            spec.egress().is_allowed(),
            "a hostname rule must start the proxy: {:?}",
            spec.egress()
        );
        assert!(
            spec.egress()
                .entries()
                .iter()
                .any(|entry| entry == "api.example.test"),
            "the rationale names the host rule: {:?}",
            spec.egress()
        );
    }

    /// A port list constrains every door, and a target whose port is unknown
    /// fails a non-empty list rather than riding through it.
    #[test]
    fn a_port_rule_refuses_every_other_port() {
        let dir = workspace();
        let policy =
            policy_with_egress(dir.path(), &["203.0.113.7"], &["api.example.test"], &[443]);
        let decide = |host: &str, ip: &str, port: Option<u16>| {
            let target = match port {
                Some(port) => Target::host_on(host, ip, port),
                None => Target::host(host, ip),
            };
            policy.probe(
                &PolicyInput::new("bash", Action::Egress, target)
                    .with_session("session")
                    .with_preset("preset"),
            )
        };
        assert!(decide("example.test", "203.0.113.7", Some(443)).is_allowed());
        let wrong_port = decide("example.test", "203.0.113.7", Some(80));
        assert!(!wrong_port.is_allowed());
        assert_eq!(wrong_port.rule.as_deref(), Some("egress-port-not-allowed"));
        // The port rule is checked before either door, so an allowlisted name
        // on a refused port is refused.
        let named = decide("api.example.test", "203.0.113.7", Some(80));
        assert_eq!(named.rule.as_deref(), Some("egress-port-not-allowed"));
        // A caller that does not state its port cannot satisfy a port list.
        let unknown = decide("example.test", "203.0.113.7", None);
        assert!(!unknown.is_allowed(), "an unknown port rode a port list");
    }

    /// The workspace grant is the policy's own answer, not a constant: the
    /// question is a write, and the rule that allows it is the baseline's
    /// in-workspace rule.
    #[test]
    fn the_workspace_grant_is_the_policys_answer() {
        let dir = workspace();
        let spec = spec(dir.path(), "workspace-write", &[]);
        let root = dir.path().canonicalize().expect("canonical root");
        assert_eq!(spec.mode(), SandboxMode::WorkspaceWrite);
        assert_eq!(spec.writable(), std::slice::from_ref(&root));
        let decision = spec
            .decisions()
            .iter()
            .find(|decision| decision.kind == GrantKind::WorkspaceWrite)
            .expect("the workspace was asked about");
        assert!(decision.allowed);
        assert_eq!(decision.rule.as_deref(), Some("write-inside-workspace"));
    }

    /// A read-only tier writes nothing, and still reads its workspace.
    #[test]
    fn a_read_only_permission_grants_no_write() {
        let dir = workspace();
        let spec = spec(dir.path(), "read-only", &[]);
        assert!(spec.writable().is_empty());
        let root = dir.path().canonicalize().expect("canonical root");
        assert!(
            spec.readable().contains(&root),
            "a read-only shell can still open its workspace: {:?}",
            spec.readable()
        );
        assert!(spec.is_confined());
        let decision = spec
            .decisions()
            .iter()
            .find(|decision| decision.kind == GrantKind::WorkspaceWrite)
            .expect("the question is still asked");
        assert!(
            !decision.allowed,
            "a read-only session refuses the write: {decision:?}"
        );
        assert_eq!(decision.rule.as_deref(), Some("session-read-only-write"));
    }

    /// Full access is no sandbox, not a permissive one.
    #[test]
    fn full_access_is_no_sandbox() {
        let dir = workspace();
        let spec = spec(dir.path(), "danger-full-access", &[]);
        assert!(!spec.is_confined());
        assert!(spec.writable().is_empty());
        assert!(spec.decisions().is_empty(), "nothing is asked of a release");
        assert_eq!(spec.terminal(), TerminalSandbox::Unconfined);
    }

    /// The .git and .bsh denies are the policy's refusals, recorded with the
    /// rule that produced them.
    #[test]
    fn the_forbidden_directories_come_from_the_policy() {
        let dir = workspace();
        let spec = spec(dir.path(), "workspace-write", &[]);
        let root = dir.path().canonicalize().expect("canonical root");
        for name in [".git", ".bsh"] {
            let path = root.join(name);
            assert!(
                spec.denied_writes().contains(&path),
                "{name} is not on the deny list: {:?}",
                spec.denied_writes()
            );
            let denial = spec
                .denies_write(&path.join("config"))
                .expect("a write inside it is refused");
            assert_eq!(denial.rule.as_deref(), Some("write-forbidden-directory"));
        }
        assert!(
            spec.denies_write(&root.join("src/main.rs")).is_none(),
            "an ordinary workspace path is not refused"
        );
    }

    /// The host temp directory is a candidate the policy refuses, so the
    /// sandbox's own scratch is the only temp a command gets.
    #[test]
    fn the_host_temp_write_is_denied() {
        let dir = workspace();
        let spec = spec(dir.path(), "workspace-write", &[]);
        let decision = spec
            .decisions()
            .iter()
            .find(|decision| decision.kind == GrantKind::HostTemp)
            .expect("the temp directory was asked about");
        assert!(!decision.allowed);
        assert_eq!(decision.rule.as_deref(), Some("write-outside-workspace"));
        assert!(
            !spec
                .writable()
                .iter()
                .any(|path| path == &std::env::temp_dir() || path.starts_with(std::env::temp_dir())),
            "the host temp directory is not writable: {:?}",
            spec.writable()
        );
    }

    /// A build cache the policy permits is the one place write+execute is
    /// granted; a directory that is not a cache is granted nothing extra.
    #[test]
    fn a_build_cache_is_writable_and_executable() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("target")).expect("the cache");
        std::fs::create_dir_all(dir.path().join("src")).expect("the source tree");
        let spec = spec(dir.path(), "workspace-write", &[]);
        let root = dir.path().canonicalize().expect("canonical root");
        assert_eq!(spec.writable_executable(), &[root.join("target")]);
        let decision = spec
            .decisions()
            .iter()
            .find(|decision| decision.kind == GrantKind::BuildCache)
            .expect("the cache was asked about");
        assert!(decision.allowed);
        assert_eq!(decision.rule.as_deref(), Some("write-inside-workspace"));
    }

    /// The egress set is the policy's allowlist, entry by entry.
    #[test]
    fn egress_is_the_policys_allowlist() {
        let dir = workspace();
        let spec = spec(
            dir.path(),
            "workspace-write",
            &["203.0.113.7", "198.51.100.0/24"],
        );
        assert_eq!(
            spec.egress(),
            &Egress::Allow(vec![
                "203.0.113.7".to_string(),
                "198.51.100.0/24".to_string()
            ])
        );
        let allowed = spec
            .decisions()
            .iter()
            .find(|decision| decision.kind == GrantKind::Egress && decision.allowed)
            .expect("an allowlisted entry");
        assert_eq!(allowed.rule.as_deref(), Some("egress-allowlisted"));

        let none = SandboxSpec::for_policy(
            &policy(dir.path(), "workspace-write", &[]),
            "workspace-write",
            Principal::new("bash", "session", "preset"),
        );
        assert_eq!(none.egress(), &Egress::DenyAll);
        assert!(!none.egress().is_allowed());
    }

    /// The terminal plan is the backend's measured answer: macOS can host the
    /// pane's tty by path and confine the shell; a backend that cannot says why.
    #[test]
    fn the_terminal_plan_matches_this_backend() {
        let dir = workspace();
        let confined = spec(dir.path(), "workspace-write", &[]);
        let answer = confined.terminal();
        if cfg!(target_os = "macos") {
            assert_eq!(answer, TerminalSandbox::Sandboxed);
            assert!(answer.is_sandboxed());
            assert!(answer.reason().is_none());
        } else {
            assert!(matches!(answer, TerminalSandbox::PtyNotHostable { .. }));
            assert!(answer.reason().is_some(), "the refusal says why");
        }
        // Read only is confined too, and answers the same way.
        assert_eq!(
            spec(dir.path(), "read-only", &[]).terminal().is_sandboxed(),
            cfg!(target_os = "macos")
        );
        // Full access has no confinement to apply.
        assert_eq!(
            spec(dir.path(), "danger-full-access", &[]).terminal(),
            TerminalSandbox::Unconfined
        );
    }

    /// A spec built from a root alone still describes the default grants, and
    /// an unknown permission string never selects the unconfined build.
    #[test]
    fn the_policy_free_spec_and_the_mode_parser_agree_with_the_default() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("target")).expect("the cache");
        let spec = SandboxSpec::workspace_write(dir.path());
        let root = dir.path().canonicalize().expect("canonical root");
        assert_eq!(spec.writable(), std::slice::from_ref(&root));
        assert_eq!(spec.writable_executable(), &[root.join("target")]);
        assert_eq!(
            spec.denied_writes(),
            &[root.join(".git"), root.join(".bsh")]
        );
        assert_eq!(spec.mode(), SandboxMode::WorkspaceWrite);

        assert_eq!(SandboxMode::from_permission(""), SandboxMode::ReadOnly);
        assert_eq!(
            SandboxMode::from_permission("workspace-writ"),
            SandboxMode::ReadOnly
        );
        assert_eq!(
            SandboxMode::from_permission("danger-full-access"),
            SandboxMode::FullAccess
        );
    }

    /// The fallback build still describes the same grants: the spec is the
    /// contract, and which backend enforces it is a feature choice.
    #[test]
    fn the_spec_describes_grants_with_or_without_the_feature() {
        let dir = workspace();
        let spec = spec(dir.path(), "workspace-write", &[]);
        assert!(
            !spec.writable().is_empty(),
            "a workspace-write spec always names its writable root"
        );
        let rendered: Vec<String> = spec.rationale();
        assert!(
            rendered.iter().any(|line| line.contains("workspace-write")),
            "the rationale names the workspace question: {rendered:?}"
        );
    }
}
