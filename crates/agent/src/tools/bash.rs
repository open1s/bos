use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::jobs::{JobRegistry, JobStatus};
use crate::policy::{Action, PolicyInput, PolicySet, Target};
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
    /// When set, the command runs under a macOS Seatbelt profile that denies
    /// writes outside this root (plus temp dirs), so the confinement is
    /// enforced per syscall rather than only by the lexical scan.
    sandbox_root: Option<PathBuf>,
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
            sandbox_root: None,
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

    /// Confine the child with a macOS Seatbelt profile rooted at `root`.
    ///
    /// The profile allows writes under the root, the temp directories and the
    /// standard device sinks, and explicitly denies the root's `.git` and
    /// `.bsh` directories. It is applied only when `/usr/bin/sandbox-exec`
    /// exists; otherwise the lexical scan remains the boundary. The kernel
    /// enforces this per write syscall, including writes reached through shell
    /// variables or an interpreter that the lexical scan cannot see.
    pub fn with_sandbox(mut self, root: impl Into<PathBuf>) -> Self {
        self.sandbox_root = Some(root.into());
        self
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

    /// The child command line for one shell invocation, before its pipes.
    ///
    /// With a sandbox root the child is wrapped in sandbox-exec: the profile
    /// denies writes outside the workspace and temp dirs per syscall, which
    /// closes the lexical scan's variable/interpreter gap. This is the existing
    /// per-syscall boundary for a background job too; no new confinement is
    /// implied by detaching it.
    fn command_process(&self, command: &str) -> std::process::Command {
        let mut process = match self.sandbox_root.as_deref().and_then(seatbelt_profile) {
            Some(profile) => {
                let mut process = std::process::Command::new("/usr/bin/sandbox-exec");
                process.args(["-p", profile.as_str(), "/bin/sh", "-c", command]);
                process
            }
            None => {
                let mut process = std::process::Command::new("sh");
                process.args(["-c", command]);
                process
            }
        };
        if let Some(root) = self
            .config
            .workspace_root
            .as_deref()
            .map(str::trim)
            .filter(|root| !root.is_empty())
        {
            process.current_dir(root);
        }
        process
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
        let mut process = self.command_process(command);
        process
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            process.process_group(0);
        }
        let mut child = process
            .spawn()
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let pid = child.id();
        let id = jobs.begin_in(self.job_session.clone(), self.name.clone(), command, cwd);
        jobs.set_pid(&id, pid);
        if let Some(stdout) = child.stdout.take() {
            pipe_to_jobs(stdout, jobs.clone(), id.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            pipe_to_jobs(stderr, jobs.clone(), id.clone());
        }
        let waiter_jobs = jobs.clone();
        let waiter_id = id.clone();
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
        let mut process = self.command_process(command);
        process
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = process
            .spawn()
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .map(|pipe| std::thread::spawn(move || drain(pipe)));
        let stderr = child
            .stderr
            .take()
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
                        let _ = child.kill();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => {
                    let _ = child.kill();
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
        };

        serde_json::to_value(result).map_err(|e| ToolError::Failed(e.to_string()))
    }
}

/// The write commands whose path arguments the pre-spawn scan checks.
const WRITE_COMMANDS: &[&str] = &[
    "touch", "mkdir", "rm", "rmdir", "cp", "mv", "ln", "tee", "truncate", "install", "dd",
];

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

/// A macOS Seatbelt profile that allows writes under `root` and the temp
/// directories, and denies the root's `.git` and `.bsh` internals.
///
/// Returns none when this host has no `sandbox-exec`, so the caller falls back
/// to the lexical scan. The profile is a single SBPL string: `allow default`
/// keeps reads, process execution and network as they were, `deny file-write*`
/// closes every write, and the following rules re-open the intended paths.
fn seatbelt_profile(root: &Path) -> Option<String> {
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        return None;
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut writable: Vec<PathBuf> = vec![
        root.clone(),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        std::env::temp_dir(),
    ];
    if let Some(temp) = std::env::var_os("TMPDIR").map(PathBuf::from) {
        if let Ok(canonical) = temp.canonicalize() {
            writable.push(canonical);
        }
        writable.push(temp);
    }
    if let Ok(canonical) = std::env::temp_dir().canonicalize() {
        writable.push(canonical);
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
    #[cfg(target_os = "macos")]
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
}
