use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::jobs::{JobRegistry, JobStatus};
use crate::policy::{Action, PolicyInput, PolicySet, Target};
use crate::sandbox::{SandboxMode, SandboxSpec, SandboxedChild};
use crate::security::WorkspaceValidator;
use react::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Configuration for [`BashTool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashToolConfig {
    /// Workspace root; when set, commands are validated against it.
    pub workspace_root: Option<String>,
    /// Optional allowlist of command names.
    pub allowed_commands: Option<Vec<String>>,
    /// Optional denylist of command names.
    pub denied_commands: Option<Vec<String>>,
    /// Command timeout in seconds.
    pub timeout_secs: u64,
    /// Whether shell metacharacters are permitted.
    pub allow_shell_injection: bool,
}

impl Default for BashToolConfig {
    fn default() -> Self {
        Self {
            workspace_root: None,
            allowed_commands: None,
            denied_commands: None,
            timeout_secs: 300,
            allow_shell_injection: false,
        }
    }
}

/// The per-turn policy a shell tool asks before it spawns or writes.
pub struct BashPolicy {
    policy: Arc<PolicySet>,
    session: String,
    preset: String,
}

impl BashPolicy {
    /// Bind a compiled policy to one session and preset.
    pub fn new(
        policy: Arc<PolicySet>,
        session: impl Into<String>,
        preset: impl Into<String>,
    ) -> Self {
        Self {
            policy,
            session: session.into(),
            preset: preset.into(),
        }
    }

    pub(crate) fn check(
        &self,
        tool: &str,
        action: Action,
        target: Target,
    ) -> Result<(), ToolError> {
        let input = PolicyInput::new(tool, action, target)
            .with_session(self.session.clone())
            .with_preset(self.preset.clone());
        let decision = self.policy.check(&input);
        if decision.is_allowed() {
            Ok(())
        } else {
            Err(ToolError::Failed(self.policy.refusal(&decision)))
        }
    }
}

impl std::fmt::Debug for BashPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BashPolicy")
            .field("session", &self.session)
            .field("preset", &self.preset)
            .finish_non_exhaustive()
    }
}

/// A [`Tool`] that runs shell commands via `sh -c`.
///
/// With a [`BashPolicy`] the tool is gated as a **spawn**, not per syscall:
/// the working directory must be the workspace root, and the command is scanned
/// for obvious outside write targets (redirections and the arguments of common
/// writing commands). That scan is a guard rail, not a sandbox - a command can
/// still write through an interpreter, a variable or an unlisted tool - so the
/// spawn gate is the boundary this tool can honestly promise.
pub struct BashTool {
    name: String,
    config: BashToolConfig,
    validator: Option<WorkspaceValidator>,
    policy: Option<BashPolicy>,
    /// When set, the command runs inside the sandbox this spec describes.
    ///
    /// The spec is the ONE seam the tool and the terminal share: it is built
    /// from the effective permission and the regorus policy, so the grants a
    /// command gets are the policy's answers rather than a constant. With the
    /// heel-sandbox feature on, heel's native backend enforces them; with it
    /// off the spec still describes them and the in-tree Seatbelt profile is
    /// the boundary.
    sandbox: Option<Arc<SandboxSpec>>,
    /// The roster a run_in_background call registers in, when the shell wires
    /// one. Without it the flag is refused rather than silently run foreground.
    jobs: Option<Arc<JobRegistry>>,
    /// The session a background job belongs to, which is the principal the
    /// roster scopes a reader to.
    job_session: String,
    /// Whether the turn's capability decision grants `run_in_background`.
    ///
    /// A denied flag is absent from the advertised schema and refused at call
    /// time rather than silently run in the foreground.
    background: bool,
}

impl BashTool {
    /// Create a bash tool with the default configuration.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            config: BashToolConfig::default(),
            validator: None,
            policy: None,
            sandbox: None,
            jobs: None,
            job_session: String::new(),
            background: true,
        }
    }

    /// Grant or withhold the `run_in_background` capability for this turn.
    ///
    /// The registration gate passes its policy/preset decision here. When
    /// withheld, the parameter is not advertised and a call that sends it is
    /// refused before any process or job exists.
    pub fn with_background(mut self, enabled: bool) -> Self {
        self.background = enabled;
        self
    }

    /// Register background runs in the shell's roster, under `session`.
    pub fn with_jobs(mut self, jobs: Arc<JobRegistry>, session: impl Into<String>) -> Self {
        self.jobs = Some(jobs);
        self.job_session = session.into();
        self
    }

    /// Gate spawns and obvious outside writes through `policy`.
    pub fn with_policy(
        mut self,
        policy: Arc<PolicySet>,
        session: impl Into<String>,
        preset: impl Into<String>,
    ) -> Self {
        self.policy = Some(BashPolicy::new(policy, session, preset));
        self
    }

    /// Confine the child to `root` with the default Workspace Write spec.
    ///
    /// This is the policy-free constructor: the grants are the ones a default
    /// Workspace Write turn gets (the root writable, its build caches writable
    /// and executable, the toolchain readable and executable) without asking a
    /// policy. A caller that has a compiled policy should pass the derived spec
    /// with [BashTool::with_sandbox_spec] instead, so the policy decides.
    pub fn with_sandbox(mut self, root: impl Into<PathBuf>) -> Self {
        self.sandbox = Some(Arc::new(SandboxSpec::workspace_write(root)));
        self
    }

    /// Confine the child with a spec derived from the turn's policy.
    ///
    /// This is what the shell wire-up uses, and the same value is handed to the
    /// terminal's spawn path, so the two cannot disagree about what the turn
    /// may touch.
    pub fn with_sandbox_spec(mut self, spec: Arc<SandboxSpec>) -> Self {
        self.sandbox = Some(spec);
        self
    }

    /// The spec this tool runs commands under, when it has one.
    pub fn sandbox_spec(&self) -> Option<&Arc<SandboxSpec>> {
        self.sandbox.as_ref()
    }

    /// Replace the configuration; a workspace root also installs a validator.
    pub fn with_config(mut self, config: BashToolConfig) -> Self {
        if let Some(ref root) = config.workspace_root {
            self.validator = Some(WorkspaceValidator::new(std::path::PathBuf::from(root)));
        }
        self.config = config;
        self
    }

    /// Restrict the tool to `workspace_root`.
    pub fn with_workspace(mut self, workspace_root: &str) -> Self {
        self.validator = Some(WorkspaceValidator::new(std::path::PathBuf::from(
            workspace_root,
        )));
        self.config.workspace_root = Some(workspace_root.to_string());
        self
    }

    /// Refuse a command whose literal network endpoints the policy denies.
    ///
    /// Every resolved address must be allowed, exactly as the egress gate
    /// requires: a name that resolves to one allowlisted and one unknown address
    /// is refused, because the client could pick the unknown one. A name that
    /// does not resolve is asked as a NAME, so a hostname rule can still admit
    /// it (an ssh alias, a host only the command's own resolver knows).
    fn check_network(&self, policy: &BashPolicy, command: &str) -> Result<(), ToolError> {
        use std::net::ToSocketAddrs as _;
        for (host, port) in network_targets(command) {
            let addresses: Vec<String> = (host.as_str(), port)
                .to_socket_addrs()
                .map(|addrs| addrs.map(|addr| addr.ip().to_string()).collect())
                .unwrap_or_default();
            if addresses.is_empty() {
                policy.check(&self.name, Action::Egress, Target::host_on(&host, "", port))?;
                continue;
            }
            for address in addresses {
                policy.check(
                    &self.name,
                    Action::Egress,
                    Target::host_on(&host, address, port),
                )?;
            }
        }
        Ok(())
    }

    /// What the command's network could reach, as the spec enforces it.
    fn network_label(&self) -> &'static str {
        use crate::sandbox::Egress;
        match self.sandbox.as_deref() {
            Some(spec) if !spec.is_confined() => "unrestricted",
            Some(spec) => match spec.egress() {
                Egress::DenyAll => "deny-all",
                Egress::Allow(_) => "allowlist",
            },
            None => "unrestricted",
        }
    }

    /// Start one shell invocation under this tool's spec, with its output piped.
    ///
    /// Every spawn - foreground, background, confined, released - goes through
    /// here, so there is one place that decides what a command runs inside. With
    /// no spec the command is bare; with one, the spec's backend starts it and
    /// the spec's grants are what the kernel enforces.
    fn spawn_command(&self, command: &str, cwd: &Path) -> Result<SandboxedChild, ToolError> {
        let spec = match self.sandbox.as_deref() {
            Some(spec) => spec.clone(),
            // No spec means Full access: the permission already released
            // confinement, and a bare shell is that release.
            None => SandboxSpec::bare(SandboxMode::FullAccess, cwd),
        };
        spec.spawn(command, cwd).map_err(ToolError::Failed)
    }

    /// Spawn a command without waiting for it, registering it in the roster.
    ///
    /// The policy checks have already run in `run` before this is reached, so a
    /// refused spawn never creates a job or a process. The child is placed in
    /// its own process group so a kill reaches the shell and every command it
    /// started, and its stdin is closed so a command that reads it sees EOF
    /// instead of hanging forever.
    fn spawn_background(&self, command: &str, cwd: &Path) -> Result<Value, ToolError> {
        let Some(jobs) = self.jobs.clone() else {
            return Err(ToolError::Failed(
                "run_in_background is not available: no job registry is configured".to_string(),
            ));
        };
        let mut child = self.spawn_command(command, cwd)?;
        let pid = child.id();
        let backend = child.backend();
        let network = self.network_label();
        let id = jobs.begin_in(self.job_session.clone(), self.name.clone(), command, cwd);
        jobs.set_pid(&id, pid);
        if let Some(stdout) = child.take_stdout() {
            pipe_to_jobs(stdout, jobs.clone(), id.clone());
        }
        if let Some(stderr) = child.take_stderr() {
            pipe_to_jobs(stderr, jobs.clone(), id.clone());
        }
        let waiter_jobs = jobs.clone();
        let waiter_id = id.clone();
        // The child moves into the waiter: it carries the sandbox that started
        // it, and dropping that sandbox kills every process it owns. Detaching
        // the job must not detach the confinement.
        std::thread::spawn(move || {
            let status = match child.wait() {
                Ok(status) if status.success() => JobStatus::Succeeded { code: 0 },
                Ok(status) => JobStatus::Failed {
                    code: status.code().unwrap_or(-1),
                },
                Err(_) => JobStatus::Failed { code: -1 },
            };
            waiter_jobs.finish(&waiter_id, status);
        });
        Ok(serde_json::json!({
            "job_id": id,
            "message": "started in background",
            "pid": pid,
            "sandbox": backend,
            "network": network,
            "status": JobStatus::Running.as_str(),
        }))
    }
}

/// Captured output of one bash command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashExecutionResult {
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Process exit code, or -1 when killed by a signal.
    pub exit_code: i32,
    /// Whether the process exited successfully.
    pub success: bool,
    /// The backend that ran the command: "heel", "seatbelt" or "none".
    ///
    /// A reader can tell a refusal the sandbox made from a command that simply
    /// failed, and a test can prove which boundary was actually in force.
    pub sandbox: String,
    /// What the command's network could reach: "deny-all", "allowlist" or
    /// "unrestricted".
    ///
    /// Reported beside the backend because a reader asking "did this touch the
    /// network" needs the answer the sandbox enforced, not an inference from the
    /// command line.
    pub network: String,
}

/// Read one child pipe to the end on its own thread.
///
/// Both pipes are drained while the process is waited for: a command whose
/// output fills a pipe buffer would otherwise block before it could exit, and a
/// timeout could never fire.
fn drain<R: std::io::Read + Send + 'static>(mut pipe: R) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    bytes
}

/// Feed one child pipe into a job's bounded ring until the pipe closes.
fn pipe_to_jobs<R: Read + Send + 'static>(mut pipe: R, jobs: Arc<JobRegistry>, id: String) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => jobs.append_output(&id, &buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
}

impl Tool for BashTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        let mut description =
            "Run a shell command with `sh -c` and return its stdout, stderr and exit \
             code. Pass the WHOLE command line in `command`; pipes, redirection and \
             `&&` are supported. Relative paths resolve from the workspace root when \
             one is configured."
                .to_string();
        if self.background {
            description.push_str(
                " With run_in_background the call returns a job id at once instead \
                 of waiting; read it with job_output, stop it with job_kill.",
            );
        }
        description
    }

    fn category(&self) -> String {
        "system".to_string()
    }

    /// The arguments the model must send.
    ///
    /// Without this the tool was advertised with an empty parameter schema, so a
    /// model had no way to know `command` was required and called it with `{}` -
    /// which the tool then refused. The schema is what makes the call possible,
    /// not merely a hint.
    fn json_schema(&self) -> Value {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The complete shell command line to run."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        });
        if self.background {
            schema["properties"]["run_in_background"] = serde_json::json!({
                "type": "boolean",
                "description": "Start the command as a background job and return its id at once.",
                "default": false
            });
        }
        schema
    }

    /// The OpenAI projection carries the same parameter schema.
    fn to_openai_definition(&self) -> react::tool::ToolDefinition {
        let mut properties = std::collections::HashMap::new();
        properties.insert(
            "command".to_string(),
            react::tool::ToolParameterProperty {
                param_type: "string".to_string(),
                description: Some("The complete shell command line to run.".to_string()),
                enum_values: None,
            },
        );
        if self.background {
            properties.insert(
                "run_in_background".to_string(),
                react::tool::ToolParameterProperty {
                    param_type: "boolean".to_string(),
                    description: Some(
                        "Start the command as a background job and return its id at once."
                            .to_string(),
                    ),
                    enum_values: None,
                },
            );
        }
        react::tool::ToolDefinition::new(self.name(), self.description()).with_parameters(
            react::tool::ToolParameters {
                param_type: "object".to_string(),
                properties: Some(properties),
                required: Some(vec!["command".to_string()]),
            },
        )
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Failed("Missing required field: command".to_string()))?;

        let cwd = self
            .config
            .workspace_root
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        if let Some(policy) = &self.policy {
            // The boundary is the spawn: the shell starts only in the
            // workspace, and the command's obvious write targets are checked
            // against the same policy before a process exists.
            policy.check(&self.name, Action::Spawn, Target::path(cwd.clone()))?;
            for target in write_targets(command, &cwd) {
                policy.check(&self.name, Action::Write, Target::path(target))?;
            }
        }

        // The second tier, from the spec rather than from the policy: a target
        // the derived spec marks as forbidden is refused even when the policy
        // tier's own answer was reached another way. Under heel this is the
        // ONLY tier that denies .git and .bsh writes - the backend has no way to
        // carve a deny out of the workspace grant - so the refusal names the
        // rule that decided it.
        if let Some(spec) = self.sandbox.as_deref() {
            for target in write_targets(command, &cwd) {
                if let Some(denial) = spec.denies_write(&target) {
                    return Err(ToolError::Failed(format!(
                        "sandbox: {} is refused ({})",
                        target.display(),
                        denial.describe()
                    )));
                }
            }
        }

        // The network tier: every endpoint the command literally names is put to
        // the policy BEFORE a process exists, so a refused connection is a
        // refusal with a rule rather than a failed command. This is the same
        // egress question the proxy asks per connection; here it is asked about
        // a command line, which is also the only network gate the
        // heel-sandbox-off build has.
        if let Some(policy) = &self.policy {
            self.check_network(policy, command)?;
        }

        if let Some(ref _validator) = self.validator {
            if WorkspaceValidator::is_destructive_command(command) {
                return Err(ToolError::Failed(
                    "Destructive command blocked by security policy".to_string(),
                ));
            }

            if WorkspaceValidator::requires_elevated_privilege(command) {
                return Err(ToolError::Failed(
                    "Elevated privilege command blocked by security policy".to_string(),
                ));
            }
        }

        // The spawn gate above is the whole policy boundary for this call. A
        // background run inherits the turn's principal and is checked here at
        // spawn time; per-syscall enforcement inside it is the existing
        // sandbox/lexical boundary, not a new one.
        let background = input
            .get("run_in_background")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if background {
            if !self.background {
                // The schema no longer advertises the flag, so a model that
                // still sends it is refused before a process or job exists.
                return Err(ToolError::Failed(
                    "run_in_background is not available: this turn's policy does not grant a background shell"
                        .to_string(),
                ));
            }
            return self.spawn_background(command, &cwd);
        }

        // A command runs in the workspace it is confined to, so `ls` lists the
        // project rather than whatever directory the app was launched from.
        let mut child = self.spawn_command(command, &cwd)?;
        let backend = child.backend();
        let stdout = child
            .take_stdout()
            .map(|pipe| std::thread::spawn(move || drain(pipe)));
        let stderr = child
            .take_stderr()
            .map(|pipe| std::thread::spawn(move || drain(pipe)));

        // The configured timeout is enforced here: a command that never returns
        // must not hold the turn open forever. Zero means no limit.
        let started = std::time::Instant::now();
        let limit = std::time::Duration::from_secs(self.config.timeout_secs);
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if self.config.timeout_secs > 0 && started.elapsed() >= limit {
                        timed_out = true;
                        child.kill();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => {
                    child.kill();
                    return Err(ToolError::Failed(error.to_string()));
                }
            }
        }
        let status = child.wait().map_err(|e| ToolError::Failed(e.to_string()))?;
        let stdout = stdout
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        let stderr = stderr
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        if timed_out {
            return Err(ToolError::Timeout);
        }

        let result = BashExecutionResult {
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
            exit_code: status.code().unwrap_or(-1),
            success: status.success(),
            sandbox: backend.to_string(),
            network: self.network_label().to_string(),
        };

        serde_json::to_value(result).map_err(|e| ToolError::Failed(e.to_string()))
    }
}

/// The write commands whose path arguments the pre-spawn scan checks.
const WRITE_COMMANDS: &[&str] = &[
    "touch", "mkdir", "rm", "rmdir", "cp", "mv", "ln", "tee", "truncate", "install", "dd",
];

/// The commands whose arguments name a network endpoint.
///
/// Only these are scanned. A URL in some other command's argument is not a
/// connection - refusing "echo https://example.com" would be a false refusal -
/// and a command that reaches the network through its own configuration (a
/// package manager's index, a git remote named in .git/config) names no host
/// here at all. Those are left to the kernel and to the sandbox's proxy, which
/// see the connection itself rather than a guess about a command line.
const NETWORK_COMMANDS: &[&str] = &[
    "curl", "wget", "nc", "ncat", "netcat", "ssh", "scp", "sftp", "telnet", "rsync", "ping",
];

/// The git subcommands that open a connection, and so may name a host.
const GIT_NETWORK_SUBCOMMANDS: &[&str] =
    &["clone", "fetch", "pull", "push", "ls-remote", "submodule"];

/// The endpoints a command line literally names, best effort.
///
/// A URL with a scheme yields its host and port (443 or 80 by scheme); an ssh
/// or rsync destination (`user@host:path`) yields its host and port 22. A
/// host named only inside a config file is not visible here, which is why this
/// tier is a guard rail: the sandbox's proxy is the enforcement.
fn network_targets(command: &str) -> Vec<(String, u16)> {
    let tokens = shell_tokens(command);
    let mut targets: Vec<(String, u16)> = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        // The command word is the first token that is neither an assignment nor
        // a separator.
        while index < tokens.len()
            && (tokens[index].contains('=')
                || matches!(
                    tokens[index].as_str(),
                    "|" | "||" | ";" | "&&" | "&" | "(" | ")"
                ))
        {
            index += 1;
        }
        let Some(word) = tokens.get(index) else {
            break;
        };
        let name = Path::new(word)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(word.as_str())
            .to_string();
        let mut wants_network = NETWORK_COMMANDS.contains(&name.as_str());
        if name == "git" {
            wants_network = tokens
                .get(index + 1)
                .is_some_and(|sub| GIT_NETWORK_SUBCOMMANDS.contains(&sub.as_str()));
        }
        index += 1;
        while index < tokens.len() {
            let token = &tokens[index];
            if matches!(token.as_str(), "|" | "||" | ";" | "&&" | "&") {
                break;
            }
            if wants_network {
                if let Some(target) = endpoint_in(token) {
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
            index += 1;
        }
    }
    targets
}

/// The endpoint one argument names, when it names one.
fn endpoint_in(token: &str) -> Option<(String, u16)> {
    // A URL: scheme://[user@]host[:port][/path]
    if let Some((scheme, rest)) = token.split_once("://") {
        let default_port = match scheme.to_ascii_lowercase().as_str() {
            "https" => 443,
            "http" => 80,
            "ftp" => 21,
            "ssh" | "git" | "sftp" => 22,
            _ => return None,
        };
        let authority = rest.split('/').next().unwrap_or(rest);
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let (host, port) = split_host_port(authority, default_port)?;
        return Some((host, port));
    }
    // An ssh/rsync destination: [user@]host:path. A bare "word:word" is a
    // local path far more often than a host, so the ssh form needs a user or a
    // dotted name; anything else is left to the sandbox, which sees the socket.
    let (user_host, path) = token.split_once(':')?;
    if path.is_empty() || path.starts_with("//") || user_host.is_empty() {
        return None;
    }
    let host = user_host.rsplit('@').next().unwrap_or(user_host);
    if host.is_empty()
        || host.contains('/')
        || host.contains(':')
        || (!user_host.contains('@') && !host.contains('.'))
    {
        return None;
    }
    let (host, port) = split_host_port(host, 22)?;
    Some((host, port))
}

/// Split `host[:port]`, refusing anything that is not a hostname or address.
fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host, port),
            Err(_) => return None,
        },
        None => (authority, default_port),
    };
    let host = host.trim_matches(['[', ']']);
    let looks_like_host = !host.is_empty()
        && !host.starts_with('-')
        && host.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '*' | '%')
        });
    looks_like_host.then(|| (host.to_ascii_lowercase(), port))
}

/// Split a shell line into words and the operators the scan cares about.
///
/// This is a lexer, not a shell: it handles quotes and backslash so a quoted
/// destination is one token, and it keeps redirection and pipeline operators
/// separate. It does not expand variables, substitutions or globs.
fn shell_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else if ch == '\\' && q == '"' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                '>' | '<' | '|' | ';' | '&' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    let mut op = String::from(ch);
                    if ch == '>' || ch == '&' || ch == '|' {
                        if let Some(next) = chars.peek() {
                            if (*next == '>' && ch != '|') || (*next == '&' && ch == '&') {
                                op.push(chars.next().unwrap_or(ch));
                            }
                        }
                    }
                    tokens.push(op);
                }
                _ => current.push(ch),
            },
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_sink(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some("/dev/null") | Some("/dev/stdout") | Some("/dev/stderr")
    )
}

fn push_target(targets: &mut Vec<PathBuf>, token: &str, cwd: &Path) {
    if token.is_empty() || token.starts_with("&") {
        return;
    }
    let path = Path::new(token);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    if !is_sink(&resolved) && !targets.contains(&resolved) {
        targets.push(resolved);
    }
}

/// The paths a command obviously writes to, best effort.
///
/// Redirections are checked wherever they appear. The first word of the command
/// is checked against a small list of writing commands: for `cp`, `mv`, `ln`
/// and `install` only the destination is taken, for the others every path
/// argument. Quoting is honoured; variable expansion and interpreters are not,
/// which is why the spawn gate - not this scan - is the promised boundary.
fn write_targets(command: &str, cwd: &Path) -> Vec<PathBuf> {
    let tokens = shell_tokens(command);
    let mut targets = Vec::new();
    let mut redirect_next = false;
    let mut command_index = 0usize;
    while command_index < tokens.len() && tokens[command_index].contains('=') {
        command_index += 1;
    }
    let command_word = tokens.get(command_index).map(String::as_str);
    let destination_only = matches!(command_word, Some("cp" | "mv" | "ln" | "install"));

    let mut path_args: Vec<&str> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if index == command_index {
            continue;
        }
        if token == ">" || token == ">>" || token == ">|" || token == "&>" || token == "&>>" {
            redirect_next = true;
            continue;
        }
        if redirect_next {
            push_target(&mut targets, token, cwd);
            redirect_next = false;
            continue;
        }
        if let Some(rest) = token
            .strip_prefix(">>")
            .or_else(|| token.strip_prefix(">|"))
            .or_else(|| token.strip_prefix(">"))
        {
            if !rest.is_empty() {
                push_target(&mut targets, rest, cwd);
            }
            continue;
        }
        if matches!(
            token.as_str(),
            "|" | "||" | ";" | "&&" | "<" | "2>" | "2>>" | "&"
        ) {
            continue;
        }
        if command_word.is_some() && command_word == Some("dd") {
            if let Some(rest) = token.strip_prefix("of=") {
                push_target(&mut targets, rest, cwd);
            }
            continue;
        }
        if let Some(word) = command_word {
            if WRITE_COMMANDS.contains(&word) && !token.starts_with('-') {
                path_args.push(token);
            }
        }
    }

    if destination_only {
        if let Some(last) = path_args.last() {
            push_target(&mut targets, last, cwd);
        }
    } else {
        for arg in path_args {
            push_target(&mut targets, arg, cwd);
        }
    }
    targets
}

// The Seatbelt profile moved to crate::sandbox: the spec owns the confinement
// now, so the tool and the terminal cannot build two different ones.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_command() {
        let tool = BashTool::new("bash");
        let input = serde_json::json!({ "command": "echo hello" });
        let result = tool.run(&input);
        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["stdout"].as_str().unwrap().trim(), "hello");
    }

    #[test]
    fn test_exit_code() {
        let tool = BashTool::new("bash");
        let input = serde_json::json!({ "command": "exit 42" });
        let result = tool.run(&input);
        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["exit_code"], 42);
        assert!(!value["success"].as_bool().unwrap());
    }

    #[test]
    fn test_destructive_blocked() {
        let tool = BashTool::new("bash").with_workspace("/tmp");
        let input = serde_json::json!({ "command": "rm -rf /" });
        let result = tool.run(&input);
        assert!(result.is_err());
    }

    /// The model sees the arguments the tool actually needs. Before this the
    /// schema was empty, so a model could only guess and sent `{}`.
    #[test]
    fn the_schema_names_the_command_field() {
        let tool = BashTool::new("bash");
        let schema = tool.json_schema();
        assert_eq!(schema["type"], serde_json::json!("object"));
        assert_eq!(schema["required"], serde_json::json!(["command"]));
        assert_eq!(
            schema["properties"]["command"]["type"],
            serde_json::json!("string")
        );
        assert!(
            crate::tools::validate_args(&schema, &serde_json::json!({})).is_err(),
            "an empty call is refused BEFORE the tool runs"
        );
        assert!(
            crate::tools::validate_args(&schema, &serde_json::json!({ "command": "ls" })).is_ok(),
            "a well-formed call passes validation"
        );
        assert_eq!(
            schema["properties"]["run_in_background"]["type"],
            serde_json::json!("boolean"),
            "the background flag is advertised, not implied"
        );
        assert!(
            crate::tools::validate_args(
                &schema,
                &serde_json::json!({ "command": "sleep 1", "run_in_background": true })
            )
            .is_ok(),
            "and a background call is well formed"
        );

        let definition = tool.to_openai_definition();
        let parameters = definition.function.parameters.expect("parameters");
        assert_eq!(
            parameters.required.as_deref(),
            Some(&["command".to_string()][..]),
            "the OpenAI projection carries the same requirement"
        );
        assert!(
            parameters
                .properties
                .as_ref()
                .is_some_and(|properties| properties.contains_key("command")),
            "and the property it names"
        );
        assert!(
            parameters
                .properties
                .as_ref()
                .is_some_and(|properties| properties.contains_key("run_in_background")),
            "the OpenAI projection carries the background flag too"
        );
    }

    /// A configured workspace is the command's working directory, so `ls` lists
    /// the project rather than wherever the app happened to launch from.
    #[test]
    fn a_command_runs_in_the_workspace_root() {
        let dir = std::env::temp_dir().join(format!("bsh-bash-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the scratch workspace");
        std::fs::write(dir.join("probe.txt"), b"x").expect("the probe file");
        let tool = BashTool::new("bash").with_workspace(dir.to_str().expect("a utf-8 path"));
        let result = tool
            .run(&serde_json::json!({ "command": "ls" }))
            .expect("the command runs");
        let stdout = result["stdout"].as_str().unwrap_or_default();
        assert!(
            stdout.lines().any(|line| line.trim() == "probe.txt"),
            "the command ran in the workspace: {stdout}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shell boundary is a spawn: with a policy, a command may not start
    /// outside the workspace even when it would only read.
    #[test]
    fn a_policy_refuses_a_spawn_outside_the_workspace() {
        let dir = tempfile::tempdir().expect("workspace");
        let policy = std::sync::Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new(dir.path()),
        ));
        // No workspace_root in the config, so the command's cwd is the test
        // process's directory, which is not the policy's workspace.
        let tool = BashTool::new("bash").with_policy(policy, "session", "preset");
        let result = tool.run(&serde_json::json!({ "command": "echo hello" }));
        match result {
            Err(ToolError::Failed(message)) => {
                assert!(message.contains("spawn-not-allowed"), "{message}")
            }
            other => panic!("expected a spawn refusal, got {other:?}"),
        }
    }

    /// A redirect outside the workspace is detected before a process exists,
    /// and the probe proves the target was never created.
    #[test]
    fn a_policy_refuses_a_write_outside_the_workspace() {
        let dir = tempfile::tempdir().expect("workspace");
        let outside = dir
            .path()
            .parent()
            .expect("parent")
            .join(format!("bsh-bash-escape-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&outside);
        let policy = std::sync::Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new(dir.path()),
        ));
        let tool = BashTool::new("bash")
            .with_workspace(dir.path().to_str().expect("utf-8"))
            .with_policy(policy, "session", "preset");
        let command = format!("echo escaped > {}", outside.display());
        let result = tool.run(&serde_json::json!({ "command": command }));
        match result {
            Err(ToolError::Failed(message)) => {
                assert!(message.contains("write-outside-workspace"), "{message}")
            }
            other => panic!("expected a write refusal, got {other:?}"),
        }
        assert!(!outside.exists(), "the denied write created the file");
        let _ = std::fs::remove_file(&outside);
    }

    /// The scan is lexical and small: redirections and known writing commands
    /// are inspected, /dev/null is not a stored write, and a relative target
    /// resolves against the workspace.
    #[test]
    fn the_write_scan_finds_obvious_targets() {
        let root = std::path::Path::new("/workspace");
        let targets = write_targets("echo hi > /tmp/out.txt", root);
        assert!(targets
            .iter()
            .any(|path| path == std::path::Path::new("/tmp/out.txt")));
        assert!(write_targets("true 2>/dev/null", root).is_empty());
        let relative = write_targets("touch src/new.rs", root);
        assert_eq!(
            relative,
            vec![std::path::PathBuf::from("/workspace/src/new.rs")]
        );
        let copy = write_targets("cp a.txt /tmp/b.txt", root);
        assert_eq!(copy, vec![std::path::PathBuf::from("/tmp/b.txt")]);
        assert!(write_targets("ls -la", root).is_empty());
    }

    /// The Seatbelt profile blocks a write the lexical scan cannot see: the
    /// target lives in a shell variable, so the scan resolves it inside the
    /// workspace while the kernel resolves it in the home directory.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_seatbelt_sandbox_blocks_a_variable_write_outside() {
        if !std::path::Path::new("/usr/bin/sandbox-exec").exists() {
            return;
        }
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let dir = tempfile::tempdir().expect("workspace");
        let outside = home.join(format!("bsh-sandbox-escape-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&outside);
        let policy = std::sync::Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new(dir.path()),
        ));
        let tool = BashTool::new("bash")
            .with_workspace(dir.path().to_str().expect("utf-8"))
            .with_policy(policy, "session", "preset")
            .with_sandbox(dir.path());
        let command = format!("T=\"{}\"; printf hi > \"$T\"", outside.display());
        let result = tool
            .run(&serde_json::json!({ "command": command }))
            .expect("the command runs and reports its own failure");
        assert!(
            !result["success"].as_bool().unwrap_or(true),
            "the sandboxed command failed: {result}"
        );
        assert!(
            !outside.exists(),
            "the kernel sandbox let the variable write escape"
        );
    }

    /// The same profile still allows a write inside the workspace, and denies
    /// the workspace's own .git internals even through a variable.
    ///
    /// This is the heel-sandbox-off build. heel has no way to carve a deny out
    /// of a granted tree (see heel_cannot_deny_a_git_write_inside_the_workspace_grant),
    /// so this kernel-level deny is exactly what the fallback is kept for.
    #[cfg(all(target_os = "macos", not(feature = "heel-sandbox")))]
    #[test]
    fn a_seatbelt_sandbox_allows_the_workspace_and_denies_git() {
        if !std::path::Path::new("/usr/bin/sandbox-exec").exists() {
            return;
        }
        let dir = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(dir.path().join(".git")).expect(".git");
        std::fs::write(dir.path().join(".git").join("config"), "original").expect("config");
        let policy = std::sync::Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new(dir.path()),
        ));
        let tool = BashTool::new("bash")
            .with_workspace(dir.path().to_str().expect("utf-8"))
            .with_policy(policy, "session", "preset")
            .with_sandbox(dir.path());

        tool.run(&serde_json::json!({
            "command": "T=inside-sbx.txt; printf hi > \"$T\""
        }))
        .expect("an in-workspace write runs");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("inside-sbx.txt")).expect("inside file"),
            "hi"
        );

        tool.run(&serde_json::json!({
            "command": "T=.git/config; printf hacked > \"$T\""
        }))
        .expect("the command runs and reports its own failure");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".git").join("config")).expect("config"),
            "original",
            "the kernel sandbox let a .git write through"
        );
    }

    /// A background run returns at once and leaves a running job behind.
    #[test]
    fn a_background_run_returns_at_once_and_registers_a_job() {
        let registry = Arc::new(crate::jobs::JobRegistry::new());
        let tool = BashTool::new("bash").with_jobs(registry.clone(), "session-a");
        let started = std::time::Instant::now();
        let value = tool
            .run(&serde_json::json!({
                "command": "sleep 30",
                "run_in_background": true
            }))
            .expect("the spawn is accepted");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "the call did not wait for the command"
        );
        assert_eq!(value["message"], serde_json::json!("started in background"));
        let id = value["job_id"].as_str().expect("a job id").to_string();
        let job = registry.job(&id).expect("the job is registered");
        assert_eq!(job.session, "session-a");
        assert_eq!(job.tool, "bash");
        assert_eq!(job.command, "sleep 30");
        assert_eq!(job.status, JobStatus::Running);
        assert!(job.pid.is_some(), "the pid is recorded for a later kill");
        assert!(registry.kill(&id), "the test cleans the child up");
        let summary = registry
            .wait_for(&id, std::time::Duration::from_secs(3))
            .expect("the job exists");
        assert_eq!(summary.status, JobStatus::Killed);
    }

    /// When the command exits, the wait thread settles the job with its code
    /// and both pipes have landed in the one ring.
    #[test]
    fn a_background_job_settles_with_its_code_and_merged_output() {
        let registry = Arc::new(crate::jobs::JobRegistry::new());
        let tool = BashTool::new("bash").with_jobs(registry.clone(), "session-a");
        let value = tool
            .run(&serde_json::json!({
                "command": "printf out; printf err 1>&2; exit 3",
                "run_in_background": true
            }))
            .expect("the spawn is accepted");
        let id = value["job_id"].as_str().expect("a job id").to_string();
        let summary = registry
            .wait_for(&id, std::time::Duration::from_secs(5))
            .expect("the job exists");
        assert_eq!(summary.status, JobStatus::Failed { code: 3 });
        assert!(summary.finished_at.is_some());
        let output = registry.output_for(&id).expect("captured output").text;
        assert!(output.contains("out"), "stdout was captured: {output:?}");
        assert!(output.contains("err"), "stderr was captured: {output:?}");
    }

    /// A denied spawn never reaches the roster and never starts a child.
    #[test]
    fn a_policy_refusal_creates_no_job_and_no_child() {
        let dir = tempfile::tempdir().expect("workspace");
        let marker = dir.path().join("marker.txt");
        let registry = Arc::new(crate::jobs::JobRegistry::new());
        let policy = std::sync::Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new(dir.path()),
        ));
        // No workspace_root, so the command's cwd is the test process directory
        // while the policy workspace is the temp dir: the spawn is refused.
        let tool = BashTool::new("bash").with_jobs(registry.clone(), "session-a");
        let tool = tool.with_policy(policy, "session-a", "preset");
        let command = format!("touch {}", marker.display());
        let result = tool.run(&serde_json::json!({
            "command": command,
            "run_in_background": true
        }));
        match result {
            Err(ToolError::Failed(message)) => {
                assert!(message.contains("spawn-not-allowed"), "{message}")
            }
            other => panic!("expected a spawn refusal, got {other:?}"),
        }
        assert!(
            registry.list_for_session("session-a").is_empty(),
            "a refused spawn must not register a job"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!marker.exists(), "a refused spawn started a child");
    }

    /// A turn whose capability decision withheld `run_in_background` gets no
    /// flag in the schema, and a call that sends it anyway is refused before a
    /// process or a job exists.
    #[test]
    fn a_withheld_background_flag_creates_no_job_and_no_child() {
        let dir = tempfile::tempdir().expect("workspace");
        let marker = dir.path().join("marker.txt");
        let registry = Arc::new(crate::jobs::JobRegistry::new());
        let tool = BashTool::new("bash")
            .with_workspace(dir.path().to_str().expect("utf-8"))
            .with_jobs(registry.clone(), "session-a")
            .with_background(false);
        assert!(
            tool.json_schema()["properties"]
                .get("run_in_background")
                .is_none(),
            "the withheld capability is not advertised"
        );
        let command = format!("touch {}", marker.display());
        let result = tool.run(&serde_json::json!({
            "command": command,
            "run_in_background": true
        }));
        match result {
            Err(ToolError::Failed(message)) => {
                assert!(message.contains("not available"), "{message}")
            }
            other => panic!("expected a capability refusal, got {other:?}"),
        }
        assert!(
            registry.list_for_session("session-a").is_empty(),
            "the refused flag must not register a job"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!marker.exists(), "the refused flag started a child");
    }

    /// A kill marks the job and reaches the whole process group: a marker the
    /// command would have written after its sleep never appears.
    #[test]
    fn killing_a_background_job_ends_its_process_group() {
        let dir = tempfile::tempdir().expect("workspace");
        let marker = dir.path().join("marker.txt");
        let registry = Arc::new(crate::jobs::JobRegistry::new());
        let tool = BashTool::new("bash")
            .with_workspace(dir.path().to_str().expect("utf-8"))
            .with_jobs(registry.clone(), "session-a");
        let command = format!("sleep 30; printf made > {}", marker.display());
        let value = tool
            .run(&serde_json::json!({
                "command": command,
                "run_in_background": true
            }))
            .expect("the spawn is accepted");
        let id = value["job_id"].as_str().expect("a job id").to_string();
        let pid = registry.job(&id).and_then(|job| job.pid).expect("a pid");
        assert!(registry.kill(&id));
        assert_eq!(
            registry.job(&id).expect("the job").status,
            JobStatus::Killed
        );
        registry.wait_for(&id, std::time::Duration::from_secs(3));
        let mut gone = false;
        for _ in 0..40 {
            if !crate::jobs::pid_exists(pid) {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(gone, "the killed process group is still alive (pid {pid})");
        assert!(
            !marker.exists(),
            "the command outlived its kill and wrote the marker"
        );
    }

    /// The configured timeout is enforced instead of being merely advertised.
    #[test]
    fn the_timeout_is_enforced() {
        let tool = BashTool::new("bash").with_config(BashToolConfig {
            timeout_secs: 1,
            ..BashToolConfig::default()
        });
        let started = std::time::Instant::now();
        // A busy loop the shell itself runs: no child process is left behind, so
        // killing the shell really ends the command.
        let result = tool.run(&serde_json::json!({ "command": "while :; do :; done" }));
        assert!(
            matches!(result, Err(ToolError::Timeout)),
            "the call reports a timeout, got {result:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "it did not wait for the loop to end on its own"
        );
    }

    // =====================================================================
    // The sandbox seam: one spec, two callers, measured observables.
    // =====================================================================

    /// A compiled policy for one root and permission, plus an egress allowlist.
    fn sandbox_policy(
        root: &std::path::Path,
        permission: &str,
        egress: &[&str],
    ) -> std::sync::Arc<crate::policy::PolicySet> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let data = crate::policy::PolicyData::new(root)
            .with_permission(permission)
            .with_egress_allowlist(egress.iter().map(|entry| entry.to_string()));
        std::sync::Arc::new(crate::policy::PolicySet::compile(
            vec![
                crate::policy::baseline_source(),
                crate::policy::session_permission_source(permission),
            ],
            data,
        ))
    }

    /// A shell tool whose policy carries all three egress rule shapes.
    #[cfg(feature = "heel-sandbox")]
    fn sandboxed_tool_with(
        root: &std::path::Path,
        permission: &str,
        addresses: &[&str],
        hosts: &[&str],
        ports: &[u16],
    ) -> BashTool {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let data = crate::policy::PolicyData::new(&root)
            .with_permission(permission)
            .with_egress_allowlist(addresses.iter().map(|entry| entry.to_string()))
            .with_egress_hosts(hosts.iter().map(|entry| entry.to_string()))
            .with_egress_ports(ports.iter().copied());
        let policy = std::sync::Arc::new(crate::policy::PolicySet::compile(
            vec![
                crate::policy::baseline_source(),
                crate::policy::session_permission_source(permission),
            ],
            data,
        ));
        let spec = std::sync::Arc::new(crate::sandbox::SandboxSpec::for_policy(
            &policy,
            permission,
            crate::sandbox::Principal::new("bash", "session", "preset"),
        ));
        BashTool::new("bash")
            .with_workspace(root.to_str().expect("a utf-8 path"))
            .with_policy(policy, "session", "preset")
            .with_sandbox_spec(spec)
    }

    /// A shell tool confined by a spec derived from the policy.
    fn sandboxed_tool(root: &std::path::Path, permission: &str, egress: &[&str]) -> BashTool {
        // One spelling, as the shell wires it: the policy compares the spawn's
        // cwd against its own root, so a symlinked temp root must not be given
        // two names.
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let policy = sandbox_policy(&root, permission, egress);
        let spec = std::sync::Arc::new(crate::sandbox::SandboxSpec::for_policy(
            &policy,
            permission,
            crate::sandbox::Principal::new("bash", "session", "preset"),
        ));
        BashTool::new("bash")
            .with_workspace(root.to_str().expect("a utf-8 path"))
            .with_policy(policy, "session", "preset")
            .with_sandbox_spec(spec)
    }

    /// A scratch workspace with a .git directory, so the deny can be probed.
    fn sandbox_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a scratch workspace");
        std::fs::create_dir_all(dir.path().join(".git")).expect(".git");
        std::fs::write(dir.path().join(".git").join("config"), "original").expect("the config");
        dir
    }

    /// With the heel backend a write OUTSIDE the workspace is refused by the
    /// kernel, and the probe proves it: the path is reached through a shell
    /// variable, so the lexical scan cannot see it and only the sandbox can
    /// stop it. The same command's in-workspace write succeeds.
    #[cfg(feature = "heel-sandbox")]
    #[test]
    fn heel_refuses_a_write_outside_the_workspace_and_allows_one_inside() {
        let dir = sandbox_workspace();
        let outside = std::env::temp_dir().join(format!(
            "bsh-heel-escape-{}-{}.txt",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&outside);
        let tool = sandboxed_tool(dir.path(), "workspace-write", &[]);

        let denied = tool
            .run(&serde_json::json!({
                "command": format!("T=\"{}\"; printf escaped > \"$T\"", outside.display())
            }))
            .expect("the command runs and reports its own failure");
        assert_eq!(
            denied["sandbox"],
            serde_json::json!("heel"),
            "the result names the backend that ran it"
        );
        assert!(
            !denied["success"].as_bool().unwrap_or(true),
            "the sandbox did not refuse the write: {denied}"
        );
        assert!(
            !outside.exists(),
            "the kernel let the write escape through a variable"
        );

        let allowed = tool
            .run(&serde_json::json!({ "command": "T=inside.txt; printf hi > \"$T\"" }))
            .expect("an in-workspace write runs");
        assert_eq!(allowed["sandbox"], serde_json::json!("heel"));
        assert!(allowed["success"].as_bool().unwrap_or(false), "{allowed}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("inside.txt")).expect("the inside file"),
            "hi"
        );
        let _ = std::fs::remove_file(&outside);
    }

    /// WRITE and EXEC are separate grants: a payload written into the workspace
    /// is there, and running it is refused. A build cache the POLICY granted is
    /// the deliberate exception, and executes.
    #[cfg(feature = "heel-sandbox")]
    #[test]
    fn heel_refuses_write_then_execute_except_in_a_granted_build_cache() {
        let dir = sandbox_workspace();
        std::fs::create_dir_all(dir.path().join("target")).expect("the cache");
        let tool = sandboxed_tool(dir.path(), "workspace-write", &[]);

        let payload = tool
            .run(&serde_json::json!({
                "command": "printf '#!/bin/sh\\nprintf RAN > ran-root.txt\\n' > payload.sh \
                            && chmod +x payload.sh && ./payload.sh; echo rc=$?"
            }))
            .expect("the command runs");
        assert!(
            dir.path().join("payload.sh").exists(),
            "the workspace write itself is allowed"
        );
        assert!(
            !dir.path().join("ran-root.txt").exists(),
            "a payload written into the workspace was executed: {payload}"
        );
        // The outer shell's own exit code is masked by the trailing echo, so the
        // named refusal is the kernel's, not the shell's.
        let stderr = payload["stderr"].as_str().unwrap_or_default();
        assert!(
            stderr.contains("Operation not permitted"),
            "the drop-and-run did not fail with the sandbox's named refusal: {payload}"
        );

        let cache = tool
            .run(&serde_json::json!({
                "command": "printf '#!/bin/sh\\nprintf CACHE > cache-ran.txt\\n' > target/tool.sh \
                            && chmod +x target/tool.sh && ./target/tool.sh; echo rc=$?"
            }))
            .expect("the command runs");
        assert!(
            dir.path().join("cache-ran.txt").exists(),
            "a policy-granted build cache could not be executed: {cache}"
        );
    }

    /// A write the policy forbids is refused before a process exists, and the
    /// refusal names the rule that decided it.
    #[test]
    fn a_git_write_is_refused_with_the_rule_that_refused_it() {
        let dir = sandbox_workspace();
        let tool = sandboxed_tool(dir.path(), "workspace-write", &[]);
        let result = tool.run(&serde_json::json!({ "command": "printf hacked > .git/config" }));
        match result {
            Err(ToolError::Failed(message)) => assert!(
                message.contains("write-forbidden-directory"),
                "the refusal names its rule: {message}"
            ),
            other => panic!("expected a refused .git write, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".git").join("config")).expect("the config"),
            "original"
        );
    }

    /// The measured limitation, pinned so nobody assumes otherwise: heel has no
    /// way to deny a path inside a granted tree, so a .git write reached
    /// through a variable gets through the kernel. That is why the
    /// heel-sandbox-off build still exists, and why this test will fail the day
    /// heel grows a deny grant.
    #[cfg(feature = "heel-sandbox")]
    #[test]
    fn heel_cannot_deny_a_git_write_inside_the_workspace_grant() {
        let dir = sandbox_workspace();
        let tool = sandboxed_tool(dir.path(), "workspace-write", &[]);
        let result = tool
            .run(&serde_json::json!({ "command": "T=.git/config; printf hacked > \"$T\"" }))
            .expect("the command runs");
        assert!(
            result["success"].as_bool().unwrap_or(false),
            "the write was refused, so heel DOES deny inside the grant now: {result}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".git").join("config")).expect("the config"),
            "hacked",
            "the kernel denied the variable write: the limitation is closed, update the docs"
        );
    }

    /// The proxy asks the policy PER CONNECTION, for the address it resolved:
    /// the decision is regorus's, not a list baked in at build time.
    #[cfg(feature = "heel-sandbox")]
    #[test]
    fn the_egress_callback_asks_regorus_per_connection() {
        use heel::NetworkPolicy as _;
        let dir = sandbox_workspace();
        let policy = sandbox_policy(dir.path(), "workspace-write", &["127.0.0.1"]);
        let egress = crate::sandbox::RegoEgress::new(
            policy,
            crate::sandbox::Principal::new("bash", "session", "preset"),
            vec!["127.0.0.1".to_string()],
        );
        assert_eq!(egress.allowlist(), &["127.0.0.1".to_string()]);
        let decide = |host: &str| {
            futures::executor::block_on(egress.check(&heel::DomainRequest::new(host, 80)))
        };
        assert!(decide("127.0.0.1"), "an allowlisted address was refused");
        assert!(
            !decide("198.51.100.9"),
            "an address the policy does not list was allowed"
        );

        // A PORT list is enforced by the same callback: the request's port
        // travels in the target, so the policy's port rule answers per request.
        let ported = std::sync::Arc::new(crate::policy::PolicySet::compile(
            vec![
                crate::policy::baseline_source(),
                crate::policy::session_permission_source("workspace-write"),
            ],
            crate::policy::PolicyData::new(dir.path())
                .with_permission("workspace-write")
                .with_egress_allowlist(["127.0.0.1"])
                .with_egress_ports([443u16]),
        ));
        let ported = crate::sandbox::RegoEgress::new(
            ported,
            crate::sandbox::Principal::new("bash", "session", "preset"),
            vec!["127.0.0.1".to_string()],
        );
        assert!(
            futures::executor::block_on(ported.check(&heel::DomainRequest::new("127.0.0.1", 443))),
            "an allowlisted address on an allowlisted port was refused"
        );
        assert!(
            !futures::executor::block_on(ported.check(&heel::DomainRequest::new("127.0.0.1", 80))),
            "a port outside the list was allowed"
        );

        // The same callback with an empty allowlist refuses everything: the
        // policy, not the presence of a proxy, is what decides.
        let empty = crate::sandbox::RegoEgress::new(
            sandbox_policy(dir.path(), "workspace-write", &[]),
            crate::sandbox::Principal::new("bash", "session", "preset"),
            Vec::new(),
        );
        assert!(!futures::executor::block_on(
            empty.check(&heel::DomainRequest::new("127.0.0.1", 80))
        ));
    }

    /// A policy confined to one port refuses a command that names another, at
    /// the command tier, before a process exists.
    #[cfg(feature = "heel-sandbox")]
    #[test]
    fn a_port_rule_refuses_a_network_command_with_its_own_rule() {
        let Some(host) = host_lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));
        let url = format!("http://{host}:{port}/");
        let command = format!("printf ran > ran.txt && curl -sS --max-time 3 {url}");
        let dir = sandbox_workspace();

        // Allowed address, wrong port: refused, and the rule names the port.
        let refused =
            sandboxed_tool_with(dir.path(), "workspace-write", &[host.as_str()], &[], &[1])
                .run(&serde_json::json!({ "command": command }));
        match refused {
            Err(ToolError::Failed(message)) => assert!(
                message.contains("egress-port-not-allowed"),
                "the refusal names the port rule: {message}"
            ),
            other => panic!("expected a port refusal, got {other:?}"),
        }
        assert!(
            !dir.path().join("ran.txt").exists(),
            "the command ran despite the port rule"
        );
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a port-refused command connected"
        );

        // The same command on an allowed port goes through the proxy.
        let allowed = sandboxed_tool_with(
            dir.path(),
            "workspace-write",
            &[host.as_str()],
            &[],
            &[port],
        )
        .run(&serde_json::json!({ "command": command }))
        .expect("the command runs");
        assert_eq!(allowed["network"], serde_json::json!("allowlist"));
        assert!(allowed["success"].as_bool().unwrap_or(false), "{allowed}");
        assert!(
            hits.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "the proxy never reached the listener: {allowed}"
        );
    }

    /// The result says what the command's network could reach, so a reader does
    /// not have to infer it from a command line.
    #[test]
    fn the_result_reports_the_network_confinement() {
        let plain = BashTool::new("bash")
            .run(&serde_json::json!({ "command": "printf ok" }))
            .expect("the command runs");
        assert_eq!(plain["network"], serde_json::json!("unrestricted"));

        let dir = sandbox_workspace();
        let denied = sandboxed_tool(dir.path(), "workspace-write", &[])
            .run(&serde_json::json!({ "command": "printf ok" }))
            .expect("the command runs");
        assert_eq!(denied["network"], serde_json::json!("deny-all"));

        let allowed = sandboxed_tool(dir.path(), "workspace-write", &["203.0.113.7"])
            .run(&serde_json::json!({ "command": "printf ok" }))
            .expect("the command runs");
        assert_eq!(allowed["network"], serde_json::json!("allowlist"));
    }

    /// A non-loopback address of this host.
    ///
    /// heel's proxy refuses to forward to a host-local address, so a loopback
    /// listener cannot prove an allow case; a listener on the host's LAN
    /// address can, because that is not loopback or link-local.
    #[cfg(feature = "heel-sandbox")]
    fn host_lan_address() -> Option<String> {
        for interface in ["en0", "en1"] {
            let output = std::process::Command::new("/usr/sbin/ipconfig")
                .args(["getifaddr", interface])
                .output()
                .ok()?;
            let host = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !host.is_empty() {
                return Some(host);
            }
        }
        None
    }

    /// Serve HTTP 200 to every connection, counting them.
    #[cfg(feature = "heel-sandbox")]
    fn serve_http(
        listener: std::net::TcpListener,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        std::thread::spawn(move || {
            use std::io::Write as _;
            for stream in listener.incoming().flatten() {
                let hits = std::sync::Arc::clone(&hits);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while stream.read(&mut byte).unwrap_or(0) == 1 {
                        request.push(byte[0]);
                        if request.ends_with(b"\r\n\r\n") {
                            break;
                        }
                    }
                    hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    );
                });
            }
        });
    }

    /// DenyAll refuses the connection in the kernel, and the policy's allowlist
    /// lets the SAME connection through the proxy.
    #[cfg(feature = "heel-sandbox")]
    #[test]
    fn network_activity_is_gated_before_a_process_and_again_by_the_kernel() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = host_lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));
        let url = format!("http://{host}:{port}/");
        let dir = sandbox_workspace();
        let literal = format!("printf ran > ran.txt && curl -sS --max-time 3 -o /dev/null {url}");

        // 1. The command tier: DenyAll, a literal URL, nothing spawned.
        let refused = sandboxed_tool(dir.path(), "workspace-write", &[])
            .run(&serde_json::json!({ "command": literal }));
        match refused {
            Err(ToolError::Failed(message)) => assert!(
                message.contains("egress-not-allowlisted"),
                "the refusal names its rule: {message}"
            ),
            other => panic!("expected a refused network command, got {other:?}"),
        }
        assert!(
            !dir.path().join("ran.txt").exists(),
            "the command ran before the network decision"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "a refused command connected"
        );

        // 2. The same endpoint reached through variables is invisible to the
        //    command tier, so the process runs and the KERNEL refuses.
        let hidden = format!(
            "H={host}; P={port}; printf ran2 > ran2.txt && curl -sS --max-time 3 -o /dev/null \"http://$H:$P/\""
        );
        let kernel = sandboxed_tool(dir.path(), "workspace-write", &[])
            .run(&serde_json::json!({ "command": hidden }))
            .expect("the command runs");
        assert_eq!(kernel["sandbox"], serde_json::json!("heel"));
        assert_eq!(kernel["network"], serde_json::json!("deny-all"));
        assert!(
            !kernel["success"].as_bool().unwrap_or(true),
            "deny-all let the request out: {kernel}"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "a connection reached the listener under DenyAll"
        );

        // 3. The same literal endpoint, allowlisted by the policy, goes through
        //    the proxy and reaches the listener.
        let allowed = sandboxed_tool(dir.path(), "workspace-write", &[host.as_str()])
            .run(&serde_json::json!({ "command": literal }))
            .expect("the command runs");
        assert_eq!(allowed["sandbox"], serde_json::json!("heel"));
        assert_eq!(allowed["network"], serde_json::json!("allowlist"));
        assert!(allowed["success"].as_bool().unwrap_or(false), "{allowed}");
        assert!(
            hits.load(Ordering::SeqCst) >= 1,
            "the proxy never reached the listener: {allowed}"
        );
    }

    /// With the feature off the in-tree Seatbelt profile is the boundary, and
    /// it still confines writes to the workspace.
    #[cfg(not(feature = "heel-sandbox"))]
    #[test]
    fn the_fallback_runs_commands_under_the_seatbelt_profile() {
        if !std::path::Path::new("/usr/bin/sandbox-exec").exists() {
            return;
        }
        let dir = sandbox_workspace();
        // The home directory, not the temp directory: the in-tree profile
        // deliberately allows writes under the shared temp dirs, because that is
        // where a command legitimately puts scratch files. Home is the escape.
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let outside = home.join(format!(
            "bsh-fallback-escape-{}-{}.txt",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&outside);
        let tool = sandboxed_tool(dir.path(), "workspace-write", &[]);

        let allowed = tool
            .run(&serde_json::json!({ "command": "printf hi > inside.txt" }))
            .expect("an in-workspace write runs");
        assert_eq!(allowed["sandbox"], serde_json::json!("seatbelt"));
        assert!(allowed["success"].as_bool().unwrap_or(false), "{allowed}");

        let denied = tool
            .run(&serde_json::json!({
                "command": format!("T=\"{}\"; printf escaped > \"$T\"", outside.display())
            }))
            .expect("the command runs");
        assert_eq!(denied["sandbox"], serde_json::json!("seatbelt"));
        assert!(
            !outside.exists(),
            "the fallback profile let the write escape through a variable"
        );
        let _ = std::fs::remove_file(&outside);
    }
}
