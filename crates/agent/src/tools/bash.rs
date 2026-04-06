use async_trait::async_trait;
use super::{Tool, ToolDescription};
use crate::error::ToolError;
use crate::security::WorkspaceValidator;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

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
        self.validator = Some(WorkspaceValidator::new(std::path::PathBuf::from(workspace_root)));
        self.config.workspace_root = Some(workspace_root.to_string());
        self
    }

    fn validate_command(&self, command: &str) -> Result<(), String> {
        if WorkspaceValidator::is_destructive_command(command) {
            warn!("[BashTool] Destructive command detected: {}", command);
            return Err(format!(
                "Destructive command not allowed: {}. Use with caution.",
                command
            ));
        }
        if WorkspaceValidator::requires_elevated_privilege(command) {
            warn!("[BashTool] Elevated privilege command detected: {}", command);
            return Err(format!(
                "Elevated privilege command not allowed: {}",
                command
            ));
        }

        if let Some(ref allowed) = self.config.allowed_commands {
            if !allowed.iter().any(|c| command.contains(c)) {
                return Err("Command not in allowed list".to_string());
            }
        }

        if let Some(ref denied) = self.config.denied_commands {
            if denied.iter().any(|c| command.contains(c)) {
                return Err("Command is in denied list".to_string());
            }
        }

        Ok(())
    }

    async fn execute_streaming(
        &self,
        command: &str,
    ) -> Result<BashExecutionResult, String> {
        let validation = self.validate_command(command);
        if let Err(e) = validation {
            return Err(e);
        }

        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.args(["/C", command]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", command]);
            c
        };

        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        info!("[BashTool] Executing: {}", command);

        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn: {}", e))?;

        let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut stdout_output = String::new();
        let mut stderr_output = String::new();

        loop {
            tokio::select! {
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            stdout_output.push_str(&l);
                            stdout_output.push('\n');
                            info!("[BashTool] stdout: {}", l);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!("[BashTool] stdout error: {}", e);
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            stderr_output.push_str(&l);
                            stderr_output.push('\n');
                            info!("[BashTool] stderr: {}", l);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!("[BashTool] stderr error: {}", e);
                        }
                    }
                }
                status = child.wait() => {
                    match status {
                        Ok(exit_status) => {
                            let exit_code = exit_status.code().unwrap_or(-1);
                            info!("[BashTool] Exit code: {}", exit_code);
                            return Ok(BashExecutionResult {
                                stdout: stdout_output,
                                stderr: stderr_output,
                                exit_code,
                                success: exit_status.success(),
                            });
                        }
                        Err(e) => {
                            return Err(format!("Process error: {}", e));
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Execute shell commands with streaming output and exit code propagation".to_string(),
            parameters: "JSON object with 'command' (required) and 'working_directory' (optional)".to_string(),
        }
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "working_directory": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                }
            },
            "required": ["command"]
        })
    }

    fn category(&self) -> &str {
        "system"
    }

    fn is_skill(&self) -> bool {
        false
    }

    async fn execute(&self, input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or(ToolError::ExecutionFailed("Missing required field: command".to_string()))?;

        let result = self.execute_streaming(command).await
            .map_err(|e| ToolError::ExecutionFailed(e))?;

        Ok(serde_json::to_value(result).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_command() {
        let tool = BashTool::new("bash");
        let input = serde_json::json!({ "command": "echo hello" });
        let result = tool.execute(&input).await;
        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["stdout"].as_str().unwrap().trim(), "hello");
    }

    #[tokio::test]
    async fn test_exit_code() {
        let tool = BashTool::new("bash");
        let input = serde_json::json!({ "command": "exit 42" });
        let result = tool.execute(&input).await;
        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["exit_code"], 42);
        assert!(!value["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_destructive_blocked() {
        let tool = BashTool::new("bash").with_workspace("/tmp");
        let input = serde_json::json!({ "command": "rm -rf /" });
        let result = tool.execute(&input).await;
        assert!(result.is_err());
    }
}