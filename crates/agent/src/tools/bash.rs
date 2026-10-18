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

/// A [`Tool`] that runs shell commands via `sh -c`.
pub struct BashTool {
    name: String,
    config: BashToolConfig,
    validator: Option<WorkspaceValidator>,
}

impl BashTool {
    /// Create a bash tool with the default configuration.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            config: BashToolConfig::default(),
            validator: None,
        }
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

impl Tool for BashTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        "Run a shell command with `sh -c` and return its stdout, stderr and exit \
         code. Pass the WHOLE command line in `command`; pipes, redirection and \
         `&&` are supported. Relative paths resolve from the workspace root when \
         one is configured."
            .to_string()
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
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The complete shell command line to run."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    /// The OpenAI projection carries the same one-parameter schema.
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

        // A command runs in the workspace it is confined to, so `ls` lists the
        // project rather than whatever directory the app was launched from.
        let mut process = std::process::Command::new("sh");
        process.args(["-c", command]);
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
                .is_some_and(|properties| properties.contains_key("command")),
            "and the property it names"
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
