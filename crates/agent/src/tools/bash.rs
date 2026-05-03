use crate::security::WorkspaceValidator;
use react::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashToolConfig {
    pub workspace_root: Option<String>,
    pub allowed_commands: Option<Vec<String>>,
    pub denied_commands: Option<Vec<String>>,
    pub timeout_secs: u64,
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

pub struct BashTool {
    name: String,
    config: BashToolConfig,
    validator: Option<WorkspaceValidator>,
}

impl BashTool {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            config: BashToolConfig::default(),
            validator: None,
        }
    }

    pub fn with_config(mut self, config: BashToolConfig) -> Self {
        if let Some(ref root) = config.workspace_root {
            self.validator = Some(WorkspaceValidator::new(std::path::PathBuf::from(root)));
        }
        self.config = config;
        self
    }

    pub fn with_workspace(mut self, workspace_root: &str) -> Self {
        self.validator = Some(WorkspaceValidator::new(std::path::PathBuf::from(
            workspace_root,
        )));
        self.config.workspace_root = Some(workspace_root.to_string());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
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

        Ok(serde_json::to_value(result).map_err(|e| ToolError::Failed(e.to_string()))?)
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
