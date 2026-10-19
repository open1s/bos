//! Live MCP connections, shared by every agent build.
//!
//! An MCP server is a process or an endpoint, not a document: connecting it is
//! the expensive part, and a turn must not pay it again. The pool owns the
//! connections, reconciles them when the settings change, and hands the agent
//! the tools each server advertises. A server that cannot be reached is kept as
//! an error for the pane to show; it never blocks a turn.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent::mcp::{McpClient, McpToolAdapter};
use react::tool::registry::AsyncTool;

use crate::settings::mcp::{McpServer, McpTransport};

/// Longest one connection attempt may take before it is reported as failed.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// One tool a connected server advertises, ready to register.
pub(crate) struct McpTool {
    /// The server's namespace; tool names register as `{namespace}_{name}`.
    pub(crate) namespace: String,
    /// The tool's own name on the server.
    pub(crate) name: String,
    /// The adapter that forwards a call over the connection.
    pub(crate) adapter: Arc<dyn AsyncTool>,
}

/// One live connection and the tools it offers.
struct Connected {
    /// What it was connected from, so an unchanged entry is not respawned.
    signature: String,
    /// Keeps the process alive even when the server advertises no tools; the
    /// adapters hold their own clones for the calls themselves.
    #[allow(dead_code)]
    client: Arc<McpClient>,
    tools: Vec<McpTool>,
}

/// The connected MCP servers, keyed by name.
#[derive(Default)]
pub(crate) struct McpPool {
    servers: HashMap<String, Connected>,
    errors: HashMap<String, String>,
}

impl McpPool {
    /// Reconcile the pool with the servers the settings now name.
    ///
    /// A server whose connection signature is unchanged keeps its live process;
    /// one that was removed, renamed or reconfigured is dropped here - its child
    /// is killed on drop - and reconnected. Returns the names that failed, so a
    /// caller can report them.
    pub(crate) fn sync(
        &mut self,
        rt: &tokio::runtime::Runtime,
        desired: &[McpServer],
    ) -> Vec<String> {
        let mut previous = std::mem::take(&mut self.servers);
        self.errors.clear();
        let mut failed = Vec::new();
        for server in desired.iter().filter(|server| server.enabled) {
            let signature = server.signature();
            if let Some(connected) = previous.remove(&server.name) {
                if connected.signature == signature {
                    self.servers.insert(server.name.clone(), connected);
                    continue;
                }
            }
            match connect(rt, server) {
                Ok((client, tools)) => {
                    self.servers.insert(
                        server.name.clone(),
                        Connected {
                            signature,
                            client,
                            tools,
                        },
                    );
                }
                Err(error) => {
                    failed.push(server.name.clone());
                    self.errors.insert(server.name.clone(), error);
                }
            }
        }
        failed
    }

    /// Every tool a connected server advertises.
    pub(crate) fn tools(&self) -> impl Iterator<Item = &McpTool> {
        self.servers
            .values()
            .flat_map(|connected| connected.tools.iter())
    }

    /// How many tools one server contributes, when it is connected.
    pub(crate) fn tool_count(&self, name: &str) -> Option<usize> {
        self.servers
            .get(name)
            .map(|connected| connected.tools.len())
    }

    /// The exact names one server's tools register under, when it is
    /// connected.
    ///
    /// These are the names the model sees in its tool list and calls back, so
    /// the pane can show what is actually on offer instead of only a count.
    pub(crate) fn tool_names(&self, name: &str) -> Option<Vec<String>> {
        self.servers.get(name).map(|connected| {
            connected
                .tools
                .iter()
                .map(|tool| format!("{}_{}", tool.namespace, tool.name))
                .collect()
        })
    }

    /// Why one server is not connected, when it is not.
    pub(crate) fn error(&self, name: &str) -> Option<&str> {
        self.errors.get(name).map(String::as_str)
    }

    /// Build a pool around already-made tools, without opening a connection.
    #[cfg(test)]
    pub(crate) fn with_tools(tools: Vec<McpTool>) -> Self {
        let mut pool = Self::default();
        pool.servers.insert(
            "test".to_string(),
            Connected {
                signature: "test".to_string(),
                client: Arc::new(McpClient::connect_http("http://127.0.0.1:1")),
                tools,
            },
        );
        pool
    }
}

/// A tool's input schema as the model request needs it: an OBJECT schema.
///
/// A server is allowed to omit `inputSchema`, and the protocol type then
/// deserializes to `null`. Passing that through would put `"parameters": null`
/// on the wire, which some providers reject for the WHOLE request - the tool
/// would not merely be uncallable, every turn would fail. An object with no
/// properties is the schema that says "this tool takes no arguments".
fn schema_or_empty(schema: &serde_json::Value) -> serde_json::Value {
    if schema.is_object() {
        schema.clone()
    } else {
        serde_json::json!({ "type": "object", "properties": {} })
    }
}

/// Open one connection and list its tools, on the runner's runtime.
fn connect(
    rt: &tokio::runtime::Runtime,
    server: &McpServer,
) -> Result<(Arc<McpClient>, Vec<McpTool>), String> {
    // The timeout must be CONSTRUCTED inside the runtime: a timer created
    // without a reactor panics before it can ever be awaited.
    let outcome = rt.block_on(async {
        tokio::time::timeout(CONNECT_TIMEOUT, async {
            let client = match &server.transport {
                McpTransport::Stdio { command, args } => {
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    McpClient::spawn(command, &args)
                        .await
                        .map_err(|error| error.to_string())?
                }
                McpTransport::Http { url } => McpClient::connect_http(url.clone()),
            };
            client
                .initialize()
                .await
                .map_err(|error| error.to_string())?;
            let listed = client
                .list_tools()
                .await
                .map_err(|error| error.to_string())?;
            let client = Arc::new(client);
            let namespace = server.namespace().to_string();
            let tools = listed
                .into_iter()
                .filter(|tool| !tool.name.trim().is_empty())
                .map(|tool| {
                    let name = tool.name.trim().to_string();
                    let adapter: Arc<dyn AsyncTool> = Arc::new(McpToolAdapter::new(
                        client.clone(),
                        format!("{namespace}_{name}"),
                        name.clone(),
                        tool.description.clone(),
                        schema_or_empty(&tool.input_schema),
                    ));
                    McpTool {
                        namespace: namespace.clone(),
                        name,
                        adapter,
                    }
                })
                .collect();
            Ok((client, tools))
        })
        .await
    });
    match outcome {
        Ok(result) => result,
        Err(_) => Err(format!(
            "connection timed out after {}s",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::mcp::McpScope;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime")
    }

    fn stdio(name: &str, command: &str, enabled: bool) -> McpServer {
        McpServer {
            name: name.to_string(),
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: Vec::new(),
            },
            enabled,
            scope: McpScope::Global,
        }
    }

    /// A server that cannot start is recorded, not fatal: the turn still runs
    /// without its tools and the pane can say why.
    #[test]
    fn an_unreachable_server_is_recorded_rather_than_fatal() {
        let rt = runtime();
        let mut pool = McpPool::default();
        let failed = pool.sync(
            &rt,
            &[stdio("missing", "bsh-mcp-server-that-does-not-exist", true)],
        );
        assert_eq!(failed, ["missing"]);
        assert!(pool.error("missing").is_some(), "the reason is kept");
        assert_eq!(pool.tools().count(), 0);
        assert_eq!(pool.tool_count("missing"), None);
    }

    /// The pane shows the exact names the model will call, not just a count.
    #[test]
    fn the_pool_names_the_tools_the_model_calls() {
        use agent::tools::AsyncFunctionTool;
        let fake: Arc<dyn AsyncTool> = Arc::new(AsyncFunctionTool::from_fn(
            "github_search",
            "Search GitHub",
            serde_json::json!({ "type": "object", "properties": {} }),
            |_args| async move { Ok(serde_json::json!({ "ok": true })) },
        ));
        let pool = McpPool::with_tools(vec![McpTool {
            namespace: "github".to_string(),
            name: "search".to_string(),
            adapter: fake,
        }]);
        assert_eq!(
            pool.tool_names("test"),
            Some(vec!["github_search".to_string()]),
            "the callable name is the registry name"
        );
        assert_eq!(pool.tool_count("test"), Some(1));
        assert_eq!(pool.tool_names("absent"), None);
    }

    /// A server that omits its input schema must not poison the request.
    #[test]
    fn a_missing_input_schema_becomes_an_empty_object_schema() {
        assert_eq!(
            schema_or_empty(&serde_json::Value::Null),
            serde_json::json!({ "type": "object", "properties": {} })
        );
        let declared = serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } }
        });
        assert_eq!(
            schema_or_empty(&declared),
            declared,
            "a real schema is kept"
        );
    }

    #[test]
    fn a_disabled_server_is_never_connected() {
        let rt = runtime();
        let mut pool = McpPool::default();
        let failed = pool.sync(
            &rt,
            &[stdio("off", "bsh-mcp-server-that-does-not-exist", false)],
        );
        assert!(failed.is_empty(), "a disabled entry is not an error");
        assert!(pool.error("off").is_none());
        assert_eq!(pool.tools().count(), 0);
    }
}
