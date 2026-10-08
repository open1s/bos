//! MCP server connections — the GUI's pluggable-tool ("plugin") layer.
//!
//! Each [`McpServerEntry`] in settings is one external tool provider. The
//! async [`preflight`] handshake (spawn/connect + `initialize` +
//! `list_tools`) must run **outside** the state mutex: its network awaits
//! cannot hold the non-`Send` `std::sync::Mutex` guard. The caller then
//! attaches the listed tools to an agent clone with
//! [`agent::Agent::add_mcp_tool`] under the lock.
//!
//! Tools are wrapped in [`crate::approval::GatedAsyncTool`] whenever
//! `require_approval` is on, because MCP servers are arbitrary external
//! processes. The registered adapters hold `Arc<McpClient>`, which keeps each
//! connected server alive for as long as its tools are in the registry.

use std::sync::Arc;
use std::time::Duration;

use agent::mcp::{McpClient, McpToolAdapter, ToolDefinition};
use agent::tools::AsyncTool;
use serde::Serialize;

use crate::approval::{ApprovalBroker, GatedAsyncTool};
use crate::settings::McpServerEntry;

/// Cap on the whole handshake (spawn + initialize + list_tools) per server.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Connection outcome for one configured server, surfaced to the frontend.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct McpStatus {
    /// Server name (namespace) as configured.
    pub(crate) name: String,
    /// Transport: `"stdio"` or `"http"`.
    pub(crate) transport: String,
    /// Whether the server is enabled in settings.
    pub(crate) enabled: bool,
    /// Whether the handshake completed and tools were listed.
    pub(crate) connected: bool,
    /// Number of tools attached to the agent.
    pub(crate) tools: usize,
    /// Failure detail when an attempt was made but `connected` is false.
    pub(crate) error: Option<String>,
}

/// A successful handshake: the live client plus the tools it advertised.
pub(crate) struct Handshake {
    /// Client kept alive by the tool adapters registered from it.
    pub(crate) client: Arc<McpClient>,
    /// Tool definitions returned by `list_tools`.
    pub(crate) tools: Vec<ToolDefinition>,
}

/// Connect one server and list its tools, bounded by [`CONNECT_TIMEOUT`].
///
/// Validation runs first so a malformed entry fails fast without spawning
/// anything or opening a socket.
pub(crate) async fn preflight(entry: &McpServerEntry) -> Result<Handshake, String> {
    entry.validate()?;
    let handshake = async {
        let client: Arc<McpClient> = match entry.transport.as_str() {
            "http" => Arc::new(McpClient::connect_http(entry.url.trim().to_string())),
            _ => {
                let args: Vec<&str> = entry.args.iter().map(String::as_str).collect();
                Arc::new(
                    McpClient::spawn(entry.command.trim(), &args)
                        .await
                        .map_err(|e| e.to_string())?,
                )
            }
        };
        if client.get_capabilities().await.is_none() {
            client.initialize().await.map_err(|e| e.to_string())?;
        }
        let tools = client.list_tools().await.map_err(|e| e.to_string())?;
        Ok::<Handshake, String>(Handshake { client, tools })
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, handshake).await {
        Ok(result) => result,
        Err(_) => Err(format!("timed out after {}s", CONNECT_TIMEOUT.as_secs())),
    }
}

/// Build the registry adapter for one listed tool, gated when `broker` is set.
///
/// The adapter (and any wrapper around it) answers to `{namespace}_{tool}`,
/// which is what [`agent::Agent::add_mcp_tool`] keys the registry on.
pub(crate) fn tool_for(
    handshake: &Handshake,
    namespace: &str,
    def: &ToolDefinition,
    broker: Option<Arc<ApprovalBroker>>,
) -> Arc<dyn AsyncTool> {
    let namespaced = format!("{namespace}_{}", def.name);
    let adapter: Arc<dyn AsyncTool> = Arc::new(McpToolAdapter::new(
        handshake.client.clone(),
        namespaced,
        def.name.clone(),
        def.description.clone(),
        def.input_schema.clone(),
    ));
    match broker {
        Some(broker) => Arc::new(GatedAsyncTool::new(adapter, broker)),
        None => adapter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A malformed entry fails on validation, before anything is spawned.
    #[tokio::test]
    async fn preflight_rejects_invalid_entry_without_connecting() {
        let entry = McpServerEntry {
            name: "srv".into(),
            ..McpServerEntry::default()
        };
        let err = preflight(&entry)
            .await
            .err()
            .expect("stdio needs a command");
        assert!(err.contains("command"), "unexpected: {err}");

        let entry = McpServerEntry {
            name: "srv".into(),
            transport: "http".into(),
            url: "ftp://example.com".into(),
            ..McpServerEntry::default()
        };
        let err = preflight(&entry).await.err().expect("bad scheme");
        assert!(err.contains("http://"), "unexpected: {err}");
    }

    /// A stdio server whose binary does not exist fails fast with detail.
    #[tokio::test]
    async fn preflight_reports_spawn_failure() {
        let entry = McpServerEntry {
            name: "srv".into(),
            transport: "stdio".into(),
            command: "definitely-not-a-real-mcp-binary-9d1f".into(),
            ..McpServerEntry::default()
        };
        let err = preflight(&entry).await.err().expect("spawn must fail");
        assert!(!err.is_empty(), "error must carry detail");
    }

    /// Adapters are namespaced, and the gate wrapper keeps that identity.
    #[tokio::test]
    async fn tool_for_namespaces_and_gates() {
        let handshake = Handshake {
            client: Arc::new(McpClient::connect_http("http://127.0.0.1:9")),
            tools: Vec::new(),
        };
        let def = ToolDefinition {
            name: "read".to_string(),
            description: "read a file".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let gated = tool_for(
            &handshake,
            "srv",
            &def,
            Some(Arc::new(ApprovalBroker::default())),
        );
        assert_eq!(gated.name(), "srv_read");
        assert_eq!(gated.description(), "read a file");
        let plain = tool_for(&handshake, "srv", &def, None);
        assert_eq!(plain.name(), "srv_read");
    }
}
