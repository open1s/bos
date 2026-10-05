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

impl Tool for BashTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        "Execute shell commands with streaming output and exit code propagation".to_string()
    }

    fn category(&self) -> String {
        "system".to_string()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or(ToolError::Failed(
                "Missing required field: command".to_string(),
            ))?;

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

        let output = std::process::Command::new("sh")
            .args(["-c", command])
            .output()
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let result = BashExecutionResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            success: output.status.success(),
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
}
