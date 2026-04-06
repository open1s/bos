//! MCP Server Configuration
//!
//! This module provides configuration loading for MCP servers from TOML/JSON files.
//! Supports both STDIO-based and HTTP-based MCP server configurations.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Configuration for a single MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerConfig {
    /// STDIO-based MCP server (spawned as a process)
    Stdio {
        /// Server name/identifier
        name: String,
        /// Command to execute (e.g., "npx", "python3")
        command: String,
        /// Command arguments
        args: Vec<String>,
        /// Optional: working directory for the server
        #[serde(default)]
        cwd: Option<String>,
        /// Optional: environment variables
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
    },
    /// HTTP-based MCP server (connected via HTTP)
    Http {
        /// Server name/identifier
        name: String,
        /// Base URL for the HTTP server
        url: String,
        /// Optional: headers for HTTP requests
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
    },
}

/// MCP Configuration that can be loaded from a file
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    /// List of MCP server configurations
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl McpConfig {
    /// Load MCP configuration from a file
    ///
    /// Supports TOML (.toml), JSON (.json), and YAML (.yaml/.yml) formats.
    ///
    /// # Example TOML config:
    /// ```toml
    /// [[servers]]
    /// type = "stdio"
    /// name = "filesystem"
    /// command = "npx"
    /// args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"]
    ///
    /// [[servers]]
    /// type = "http"
    /// name = "custom"
    /// url = "http://localhost:3000/mcp"
    /// ```
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, McpConfigError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .map_err(|e| McpConfigError::IoError(path.display().to_string(), e))?;

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        match ext.as_str() {
            "toml" => toml::from_str(&content).map_err(McpConfigError::ParseToml),
            "json" => serde_json::from_str(&content).map_err(McpConfigError::ParseJson),
            "yaml" | "yml" => serde_yaml::from_str(&content).map_err(McpConfigError::ParseYaml),
            _ => Err(McpConfigError::UnknownFormat(ext)),
        }
        .map_err(|e| McpConfigError::ParseError(path.display().to_string(), e.to_string()))
    }

    /// Load MCP configuration from a directory
    ///
    /// Searches for `mcp.toml`, `mcp.json`, `mcp.yaml`, or `mcp.yml` in the directory.
    /// Returns the first config file found, or an empty config if none found.
    pub fn load_from_directory(dir: impl AsRef<Path>) -> Result<Self, McpConfigError> {
        let dir = dir.as_ref();
        let config_names = ["mcp.toml", "mcp.json", "mcp.yaml", "mcp.yml"];

        for name in config_names {
            let path = dir.join(name);
            if path.exists() {
                return Self::load_from_file(&path);
            }
        }

        // Return empty config if no config file found
        Ok(Self::default())
    }

    /// Load MCP configuration using the standard discovery pattern
    ///
    /// Searches in order:
    /// 1. `./mcp.toml` (current directory)
    /// 2. `./mcp.json`
    /// 3. `./mcp.yaml`
    /// 4. `$HOME/.bos/conf/mcp.toml`
    /// 5. `$HOME/.bos/conf/mcp.json`
    /// 6. `$HOME/.bos/conf/mcp.yaml`
    pub fn discover() -> Self {
        // Check current directory first
        for name in ["mcp.toml", "mcp.json", "mcp.yaml", "mcp.yml"] {
            if Path::new(name).exists() {
                if let Ok(config) = Self::load_from_file(name) {
                    tracing::info!("Loaded MCP config from {}", name);
                    return config;
                }
            }
        }

        // Check user config directory
        if let Some(home) = dirs::home_dir() {
            let bos_conf = home.join(".bos").join("conf");
            for name in ["mcp.toml", "mcp.json", "mcp.yaml", "mcp.yml"] {
                let path = bos_conf.join(name);
                if path.exists() {
                    if let Ok(config) = Self::load_from_file(&path) {
                        tracing::info!("Loaded MCP config from {}", path.display());
                        return config;
                    }
                }
            }
        }

        tracing::debug!("No MCP config file found, using empty config");
        Self::default()
    }

    /// Get a server config by name
    pub fn get_server(&self, name: &str) -> Option<&McpServerConfig> {
        self.servers.iter().find(|s| match s {
            McpServerConfig::Stdio { name: n, .. } => n == name,
            McpServerConfig::Http { name: n, .. } => n == name,
        })
    }

    /// Get all STDIO server configs
    pub fn stdio_servers(&self) -> Vec<&McpServerConfig> {
        self.servers
            .iter()
            .filter(|s| matches!(s, McpServerConfig::Stdio { .. }))
            .collect()
    }

    /// Get all HTTP server configs
    pub fn http_servers(&self) -> Vec<&McpServerConfig> {
        self.servers
            .iter()
            .filter(|s| matches!(s, McpServerConfig::Http { .. }))
            .collect()
    }
}

/// Errors that can occur when loading MCP configuration
#[derive(Debug, thiserror::Error)]
pub enum McpConfigError {
    #[error("Failed to read file {0}: {1}")]
    IoError(String, std::io::Error),

    #[error("Failed to parse TOML from {0}: {1}")]
    ParseToml(String),

    #[error("Failed to parse JSON from {0}: {1}")]
    ParseJson(String),

    #[error("Failed to parse YAML from {0}: {1}")]
    ParseYaml(String),

    #[error("Failed to parse config from {0}: {1}")]
    ParseError(String, String),

    #[error("Unknown config format: {0}")]
    UnknownFormat(String),
}

impl From<McpConfigError> for crate::error::ToolError {
    fn from(e: McpConfigError) -> Self {
        crate::error::ToolError::ExecutionFailed(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_toml_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("mcp.toml");
        let content = r#"
[[servers]]
type = "stdio"
name = "test-stdio"
command = "echo"
args = ["hello"]

[[servers]]
type = "http"
name = "test-http"
url = "http://localhost:8080/mcp"
"#;
        fs::write(&path, content).unwrap();

        let config = McpConfig::load_from_file(&path).unwrap();
        assert_eq!(config.servers.len(), 2);

        let stdio = config.get_server("test-stdio").unwrap();
        assert!(
            matches!(stdio, McpServerConfig::Stdio { name, command, .. } if name == "test-stdio" && command == "echo")
        );

        let http = config.get_server("test-http").unwrap();
        assert!(
            matches!(http, McpServerConfig::Http { name, url, .. } if name == "test-http" && url == "http://localhost:8080/mcp")
        );
    }

    #[test]
    fn test_load_json_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("mcp.json");
        let content = r#"{
  "servers": [
    {
      "type": "stdio",
      "name": "json-stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem"]
    }
  ]
}"#;
        fs::write(&path, content).unwrap();

        let config = McpConfig::load_from_file(&path).unwrap();
        assert_eq!(config.servers.len(), 1);
    }

    #[test]
    fn test_empty_config() {
        let config = McpConfig::default();
        assert!(config.servers.is_empty());
        assert!(config.get_server("nonexistent").is_none());
    }

    #[test]
    fn test_stdio_http_filters() {
        let config = McpConfig {
            servers: vec![
                McpServerConfig::Stdio {
                    name: "stdio1".into(),
                    command: "cmd1".into(),
                    args: vec![],
                    cwd: None,
                    env: Default::default(),
                },
                McpServerConfig::Http {
                    name: "http1".into(),
                    url: "http://localhost".into(),
                    headers: Default::default(),
                },
                McpServerConfig::Stdio {
                    name: "stdio2".into(),
                    command: "cmd2".into(),
                    args: vec![],
                    cwd: None,
                    env: Default::default(),
                },
            ],
        };

        assert_eq!(config.stdio_servers().len(), 2);
        assert_eq!(config.http_servers().len(), 1);
    }
}
