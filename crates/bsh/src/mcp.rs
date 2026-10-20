//! Live MCP connections, shared by every agent build.
//!
//! An MCP server is a process or an endpoint, not a document: connecting it is
//! the expensive part, and a turn must not pay it again. The pool owns the
//! connections, reconciles them when the settings change, and hands the agent
//! the tools each server advertises. A server that cannot be reached is kept as
//! an error for the pane to show; it never blocks a turn.
//!
//! Installed extensions contribute servers too: a Codex-style `.mcp.json` (or
//! the `mcpServers` marker in `.codex-plugin/plugin.json`) is derived on each
//! reconcile, namespaced under the package id, and started with the package
//! root as its working directory. Disabling or uninstalling the package drops
//! the child on the next reconcile.
//!
//! TRUST IS THE GATE. A package's config names an arbitrary command, so
//! installing or enabling a stranger's package must not run it. Only an
//! installed, enabled AND trusted package contributes a server; an untrusted
//! one contributes nothing, and the pane reads the same declaration through
//! `declared_servers` to show what it WOULD run and ask for approval. Env
//! VALUES never leave the loader: the disclosure and the audit keep KEYS only.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
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

/// One MCP server declared by an installed extension package.
///
/// Extension servers are not settings entries: they appear and disappear with
/// the package's install/enable/trust state, so they are derived at each
/// reconcile and never written into the MCP pane's document.
#[derive(Clone)]
pub(crate) struct ExtensionMcp {
    /// Namespaced server name; tool names use it as the namespace.
    pub(crate) name: String,
    /// The server's own name in the package config, for the disclosure.
    pub(crate) server: String,
    /// How the child/endpoint is reached.
    pub(crate) transport: McpTransport,
    /// Plugin-specific environment, passed to the child only.
    pub(crate) env: Vec<(String, String)>,
    /// Working directory, usually the plugin package root.
    pub(crate) cwd: Option<PathBuf>,
}

/// What an extension server would run, in the shape the trust UI may show.
///
/// This is deliberately a SEPARATE type from [`ExtensionMcp`]: it can be
/// cloned into a dialog or an audit row without carrying the environment
/// VALUES the child is started with. Only the keys survive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpLaunch {
    /// The server's own name in the package config.
    pub(crate) server: String,
    /// The executable and its arguments, shell-quoted for display; for an
    /// HTTP server, `url <url>`.
    pub(crate) command: String,
    /// The working directory a child runs in, when it runs as a child.
    pub(crate) cwd: Option<String>,
    /// Environment variable KEYS only, sorted. Never a value.
    pub(crate) env_keys: Vec<String>,
}

impl ExtensionMcp {
    /// The launch description a disclosure or audit row may keep.
    pub(crate) fn launch(&self) -> McpLaunch {
        let command = match &self.transport {
            McpTransport::Stdio { command, args } => {
                let mut text = quote_arg(command);
                for arg in args {
                    text.push(' ');
                    text.push_str(&quote_arg(arg));
                }
                text
            }
            McpTransport::Http { url } => format!("url {url}"),
        };
        let env_keys = self
            .env
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        McpLaunch {
            server: self.server.clone(),
            command,
            cwd: self
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().to_string()),
            env_keys,
        }
    }
}

/// Quote one argument the way a shell would, for a readable command line.
fn quote_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '\\' | '$'));
    if !needs_quotes {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// The pool's unified launch description for a settings or extension server.
struct Launch {
    name: String,
    transport: McpTransport,
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
}

impl Launch {
    fn from_server(server: &McpServer) -> Self {
        Self {
            name: server.name.clone(),
            transport: server.transport.clone(),
            env: Vec::new(),
            cwd: None,
        }
    }

    fn from_extension(server: &ExtensionMcp) -> Self {
        Self {
            name: server.name.clone(),
            transport: server.transport.clone(),
            env: server.env.clone(),
            cwd: server.cwd.clone(),
        }
    }

    /// Identity of the CONNECTION. Environment values are hashed rather than
    /// embedded, so a changed secret still respawns the child without the
    /// signature carrying the secret in memory it does not already need.
    fn signature(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.env.hash(&mut hasher);
        format!(
            "{}|{:?}|{}|{:?}",
            self.name,
            self.transport,
            hasher.finish(),
            self.cwd
        )
    }
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
    /// Reconcile settings servers plus the servers installed extensions
    /// declare. The extension entries are derived from package files, not the
    /// settings MCP list, so disabling the plugin removes them here.
    pub(crate) fn sync_all(
        &mut self,
        rt: &tokio::runtime::Runtime,
        desired: &[McpServer],
        extensions: &[ExtensionMcp],
    ) -> Vec<String> {
        let mut launches: Vec<Launch> = desired
            .iter()
            .filter(|server| server.enabled)
            .map(Launch::from_server)
            .collect();
        launches.extend(extensions.iter().map(Launch::from_extension));
        self.sync_launches(rt, launches)
    }

    fn sync_launches(
        &mut self,
        rt: &tokio::runtime::Runtime,
        launches: Vec<Launch>,
    ) -> Vec<String> {
        let mut previous = std::mem::take(&mut self.servers);
        self.errors.clear();
        let mut failed = Vec::new();
        for launch in launches {
            let signature = launch.signature();
            if let Some(connected) = previous.remove(&launch.name) {
                if connected.signature == signature {
                    self.servers.insert(launch.name.clone(), connected);
                    continue;
                }
            }
            let name = launch.name.clone();
            match connect(rt, &launch) {
                Ok((client, tools)) => {
                    self.servers.insert(
                        name,
                        Connected {
                            signature,
                            client,
                            tools,
                        },
                    );
                }
                Err(error) => {
                    failed.push(name.clone());
                    self.errors.insert(name, error);
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

/// MCP servers this build may start: installed, enabled and trusted packages.
///
/// This is the gate. A package's config names an arbitrary command, so merely
/// installing or enabling it must not execute anything: an untrusted package
/// contributes no server at all, which means the pool never sees it and no
/// child is spawned. The same declaration is readable through
/// [`declared_servers`] so the pane can show what would run and ask for
/// approval before this function starts returning it.
///
/// Codex packages name a config path (or an inline object) under
/// `mcpServers` in `.codex-plugin/plugin.json`; the config itself is the
/// usual `{ "mcpServers": { name: { command, args, env } } }`. A package with
/// no manifest marker but a root `.mcp.json` is read too, so a DSH package can
/// ship the same file. Relative commands and args run with the package root as
/// their working directory, and `${CODEX_PLUGIN_ROOT}` /
/// `${CLAUDE_PLUGIN_ROOT}` / `${PLUGIN_ROOT}` expand to that root.
pub(crate) fn extension_servers(settings: &crate::settings::Settings) -> Vec<ExtensionMcp> {
    let disabled = &settings.workspace.disabled.extensions;
    let mut servers = Vec::new();
    for entry in &settings.global.extensions {
        if disabled.iter().any(|id| id == &entry.id) {
            continue;
        }
        // The trust gate. Uninstalled is the absent path below; enabled is the
        // disabled list above; trusted is this. All three must hold.
        if !entry.is_trusted() {
            continue;
        }
        let Some(root) = entry.path_root() else {
            continue;
        };
        servers.extend(declared_servers(&root, &entry.id));
    }
    servers
}

/// Every MCP server a package DECLARES, trusted or not.
///
/// The pane reads this to show what a package would run before anyone approves
/// it; [`extension_servers`] is the same parse with the trust gate applied. It
/// is a file read, so callers cache it and never run it per frame.
pub(crate) fn declared_servers(root: &std::path::Path, plugin_id: &str) -> Vec<ExtensionMcp> {
    package_servers(root, plugin_id)
}

fn package_servers(root: &std::path::Path, plugin_id: &str) -> Vec<ExtensionMcp> {
    let manifest_path = root.join(".codex-plugin").join("plugin.json");
    let mut configs: Vec<(std::path::PathBuf, serde_json::Value)> = Vec::new();
    match std::fs::read_to_string(&manifest_path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(manifest) => {
                if let Some(declared) = manifest.get("mcpServers") {
                    match declared {
                        serde_json::Value::String(relative) => {
                            let path = root.join(relative);
                            match read_json_file(&path) {
                                Ok(value) => configs.push((path, value)),
                                Err(error) => log::warn!(
                                    "extension '{plugin_id}' MCP config {}: {error}",
                                    path.display()
                                ),
                            }
                        }
                        serde_json::Value::Object(_) => {
                            configs.push((manifest_path.clone(), declared.clone()));
                        }
                        _ => log::warn!(
                            "extension '{plugin_id}' mcpServers is neither a path nor an object"
                        ),
                    }
                }
            }
            Err(error) => log::warn!(
                "extension '{plugin_id}' manifest {}: {error}",
                manifest_path.display()
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "extension '{plugin_id}' manifest {}: {error}",
            manifest_path.display()
        ),
    }
    let fallback = root.join(".mcp.json");
    if configs.is_empty() && fallback.is_file() {
        match read_json_file(&fallback) {
            Ok(value) => configs.push((fallback, value)),
            Err(error) => log::warn!(
                "extension '{plugin_id}' MCP config {}: {error}",
                fallback.display()
            ),
        }
    }
    let mut servers = Vec::new();
    for (path, value) in configs {
        let Some(map) = value
            .get("mcpServers")
            .and_then(serde_json::Value::as_object)
            .or_else(|| value.as_object())
        else {
            log::warn!(
                "extension '{plugin_id}' MCP config {} has no mcpServers object",
                path.display()
            );
            continue;
        };
        for (name, config) in map {
            match extension_server(root, plugin_id, name, config) {
                Some(server) => servers.push(server),
                None => log::warn!(
                    "extension '{plugin_id}' MCP server '{name}' has no usable command or url"
                ),
            }
        }
    }
    servers
}

fn read_json_file(path: &std::path::Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn extension_server(
    root: &std::path::Path,
    plugin_id: &str,
    server_name: &str,
    config: &serde_json::Value,
) -> Option<ExtensionMcp> {
    let namespace = format!(
        "plugin_{}__{}",
        sanitize_component(plugin_id),
        sanitize_component(server_name)
    );
    let root_text = root.to_string_lossy().to_string();
    let expand = |text: &str| {
        text.replace("${CODEX_PLUGIN_ROOT}", &root_text)
            .replace("${CLAUDE_PLUGIN_ROOT}", &root_text)
            .replace("${PLUGIN_ROOT}", &root_text)
    };
    let transport = if let Some(url) = config
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        McpTransport::Http { url: expand(url) }
    } else {
        let command = config
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())?;
        let args = config
            .get("args")
            .and_then(serde_json::Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(&expand)
                    .collect()
            })
            .unwrap_or_default();
        McpTransport::Stdio {
            command: expand(command),
            args,
        }
    };
    let env = config
        .get("env")
        .and_then(serde_json::Value::as_object)
        .map(|env| {
            let mut values: Vec<(String, String)> = env
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), expand(value))))
                .collect();
            values.sort();
            values
        })
        .unwrap_or_default();
    let cwd = config
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(|cwd| {
            let path = std::path::Path::new(cwd);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            }
        })
        .or_else(|| Some(root.to_path_buf()));
    Some(ExtensionMcp {
        name: namespace,
        server: server_name.to_string(),
        transport,
        env,
        cwd,
    })
}

fn sanitize_component(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    let output = output.trim_matches('_').to_string();
    if output.is_empty() {
        "server".to_string()
    } else {
        output
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
    launch: &Launch,
) -> Result<(Arc<McpClient>, Vec<McpTool>), String> {
    // The timeout must be CONSTRUCTED inside the runtime: a timer created
    // without a reactor panics before it can ever be awaited.
    let outcome = rt.block_on(async {
        tokio::time::timeout(CONNECT_TIMEOUT, async {
            let client = match &launch.transport {
                McpTransport::Stdio { command, args } => {
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    let env: Vec<(&str, &str)> = launch
                        .env
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str()))
                        .collect();
                    McpClient::spawn_with(command, &args, &env, launch.cwd.as_deref())
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
            let namespace = launch.name.clone();
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
        let failed = pool.sync_all(
            &rt,
            &[stdio("missing", "bsh-mcp-server-that-does-not-exist", true)],
            &[],
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

    /// A Codex package that declares a real MCP server: the pool connects it,
    /// exposes its tools, and the turn's agent can call them.
    #[test]
    fn an_extension_mcp_server_reaches_the_agent() {
        let root = std::env::temp_dir().join(format!(
            "bsh-extension-mcp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock")
                .as_nanos()
        ));
        let manifest = root.join(".codex-plugin");
        std::fs::create_dir_all(&manifest).expect("the manifest directory");
        std::fs::write(
            manifest.join("plugin.json"),
            serde_json::json!({
                "name": "mcp-fixture",
                "version": "1.0.0",
                "mcpServers": "./.mcp.json"
            })
            .to_string(),
        )
        .expect("the manifest");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/mock_mcp_server.py");
        std::fs::copy(&fixture, root.join("mock_mcp_server.py")).expect("the mock server");
        std::fs::write(
            root.join(".mcp.json"),
            serde_json::json!({
                "mcpServers": {
                    "echo": {
                        "command": "python3",
                        "args": ["./mock_mcp_server.py"],
                        "env": { "MCP_FIXTURE": "1" }
                    }
                }
            })
            .to_string(),
        )
        .expect("the MCP config");

        let mut settings = crate::settings::Settings::default();
        settings
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: "mcp-fixture".to_string(),
                path: Some(root.to_string_lossy().to_string()),
                // The premise of this test is an APPROVED package: trust is
                // what lets the declaration reach the pool at all.
                trusted: true,
                trusted_path: Some(root.to_string_lossy().to_string()),
                ..Default::default()
            });
        let servers = extension_servers(&settings);
        assert_eq!(servers.len(), 1, "one declared server");
        assert_eq!(servers[0].name, "plugin_mcp-fixture__echo");
        assert_eq!(
            servers[0].cwd.as_deref(),
            Some(root.as_path()),
            "the package root is the working directory"
        );

        let rt = runtime();
        let mut pool = McpPool::default();
        let failed = pool.sync_all(&rt, &[], &servers);
        assert!(
            failed.is_empty(),
            "the fixture MCP server connects: {failed:?}"
        );
        let names = pool
            .tool_names("plugin_mcp-fixture__echo")
            .expect("the extension server is connected");
        assert!(
            names.iter().any(|name| name.ends_with("_echo_tool")),
            "the server's tools are listed: {names:?}"
        );

        let baseline = crate::caps::build_agent(
            &crate::settings::Settings::default(),
            &crate::vault::Vault::disabled(),
            &crate::settings::resolve::SessionChoice::default(),
            &McpPool::default(),
        )
        .tools_schema_chars;
        let built = crate::caps::build_agent(
            &settings,
            &crate::vault::Vault::disabled(),
            &crate::settings::resolve::SessionChoice::default(),
            &pool,
        );
        assert!(
            built.tools_schema_chars > baseline,
            "the extension's MCP tools reach the request schema ({baseline} -> {})",
            built.tools_schema_chars
        );
        let registry = built.agent.registry().expect("the tool registry");
        let tool = registry
            .get_async("plugin_mcp-fixture__echo_echo_tool")
            .unwrap_or_else(|| {
                panic!(
                    "the extension MCP tool is in the agent's registry: {:?}",
                    registry.list()
                )
            });
        let output = rt
            .block_on(tool.run(&serde_json::json!({ "message": "hi" })))
            .expect("the MCP tool call succeeds");
        assert!(
            output.to_string().contains("echo: hi"),
            "the tool call round-trips through the server: {output}"
        );

        // Disabling the package is enough to drop its server before the next
        // turn; uninstall does the same through the same reconcile path.
        settings
            .workspace
            .disabled
            .extensions
            .push("mcp-fixture".to_string());
        let active = extension_servers(&settings);
        assert!(
            active.is_empty(),
            "a disabled package derives no MCP server"
        );
        pool.sync_all(&rt, &[], &active);
        assert_eq!(
            pool.tools().count(),
            0,
            "the disabled server's tools are gone from the pool"
        );

        drop(pool);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_disabled_extension_does_not_derive_mcp_servers() {
        let mut settings = crate::settings::Settings::default();
        settings
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: "mcp-fixture".to_string(),
                path: Some("/tmp/not-read-because-disabled".to_string()),
                ..Default::default()
            });
        settings
            .workspace
            .disabled
            .extensions
            .push("mcp-fixture".to_string());
        assert!(extension_servers(&settings).is_empty());
    }

    #[test]
    fn a_disabled_server_is_never_connected() {
        let rt = runtime();
        let mut pool = McpPool::default();
        let failed = pool.sync_all(
            &rt,
            &[stdio("off", "bsh-mcp-server-that-does-not-exist", false)],
            &[],
        );
        assert!(failed.is_empty(), "a disabled entry is not an error");
        assert!(pool.error("off").is_none());
        assert_eq!(pool.tools().count(), 0);
    }

    /// A scratch package directory, unique per test and process.
    fn scratch_root(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("bsh-trust-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).expect("the scratch package root");
        root
    }

    /// A Codex package declaring one server, with the manifest wiring the
    /// `.mcp.json` the crate reads.
    fn package_with(label: &str, server: serde_json::Value) -> std::path::PathBuf {
        let root = scratch_root(label);
        let manifest = root.join(".codex-plugin");
        std::fs::create_dir_all(&manifest).expect("the manifest directory");
        std::fs::write(
            manifest.join("plugin.json"),
            serde_json::json!({
                "name": format!("trust-{label}"),
                "version": "1.0.0",
                "mcpServers": "./.mcp.json"
            })
            .to_string(),
        )
        .expect("the manifest");
        std::fs::write(
            root.join(".mcp.json"),
            serde_json::json!({ "mcpServers": { "echo": server } }).to_string(),
        )
        .expect("the MCP config");
        root
    }

    fn record(id: &str, root: &std::path::Path) -> crate::settings::global::InstalledExtension {
        crate::settings::global::InstalledExtension {
            id: id.to_string(),
            path: Some(root.to_string_lossy().to_string()),
            ..Default::default()
        }
    }

    /// An app state whose inventory holds one package, backed by a scratch
    /// store so a persistence in the test never reaches the real settings.
    fn state_with(
        label: &str,
        record: crate::settings::global::InstalledExtension,
    ) -> crate::state::AppState {
        let dir =
            std::env::temp_dir().join(format!("bsh-trust-store-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = crate::state::AppState::with_store(crate::store::Store::open_at(dir));
        state.settings.global.extensions = vec![record];
        state.settings.workspace.disabled.extensions.clear();
        state
    }

    /// An untrusted package's declaration reaches the pane but never the pool:
    /// the gate returns no server, so no reconcile can spawn the command.
    #[test]
    fn an_untrusted_mcp_extension_derives_no_server_and_spawns_nothing() {
        let marker =
            std::env::temp_dir().join(format!("bsh-trust-marker-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let root = package_with(
            "untrusted",
            serde_json::json!({
                "command": "/bin/sh",
                "args": ["-c", format!("touch '{}'", marker.display())]
            }),
        );
        let mut settings = crate::settings::Settings::default();
        settings
            .global
            .extensions
            .push(record("trust-untrusted", &root));
        assert!(
            extension_servers(&settings).is_empty(),
            "an installed package nobody trusted derives no server"
        );

        let rt = runtime();
        let mut pool = McpPool::default();
        let failed = pool.sync_all(&rt, &[], &extension_servers(&settings));
        assert!(
            failed.is_empty(),
            "a gated server is neither connected nor reported as failed: {failed:?}"
        );
        assert_eq!(pool.tools().count(), 0);
        assert!(
            !marker.exists(),
            "the child command must not have run before trust"
        );

        drop(pool);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&marker);
    }

    /// The one handler the Trust and enable button calls is what turns the
    /// declaration into a live server; the fixture's own tool then answers.
    #[test]
    fn trusting_through_the_handler_derives_the_server_and_its_fixture_tool() {
        let root = package_with(
            "python",
            serde_json::json!({
                "command": "python3",
                "args": ["./mock_mcp_server.py"],
                "env": { "MCP_FIXTURE": "1" }
            }),
        );
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/mock_mcp_server.py");
        std::fs::copy(fixture, root.join("mock_mcp_server.py")).expect("the mock server");
        let id = "mcp-trust";
        let mut state = state_with("trust", record(id, &root));
        assert!(
            extension_servers(&state.settings).is_empty(),
            "the premise: installed but untrusted"
        );

        assert!(
            state.trust_extension(id),
            "the button's handler approves the package"
        );
        assert!(
            state.settings.global.extensions[0].is_trusted(),
            "the approval is written to the committed settings"
        );
        let servers = extension_servers(&state.settings);
        assert_eq!(servers.len(), 1, "the trusted package derives its server");
        assert_eq!(servers[0].name, "plugin_mcp-trust__echo");
        let names = state
            .mcp
            .tool_names("plugin_mcp-trust__echo")
            .expect("the fixture server connected through the handler");
        assert!(
            names.iter().any(|name| name.ends_with("_echo_tool")),
            "the declared tool is listed: {names:?}"
        );

        let built = crate::caps::build_agent(
            &state.settings,
            &crate::vault::Vault::disabled(),
            &crate::settings::resolve::SessionChoice::default(),
            &state.mcp,
        );
        let registry = built.agent.registry().expect("the tool registry");
        let tool = registry
            .get_async("plugin_mcp-trust__echo_echo_tool")
            .unwrap_or_else(|| {
                panic!(
                    "the trusted MCP tool is in the agent's registry: {:?}",
                    registry.list()
                )
            });
        let output = runtime()
            .block_on(tool.run(&serde_json::json!({ "message": "hi" })))
            .expect("the fixture tool call succeeds");
        assert!(
            output.to_string().contains("echo: hi"),
            "the tool call round-trips through the server: {output}"
        );

        // Disabling is not distrust: the record stays approved, but the
        // workspace's own list keeps the server out of the pool.
        state.toggle_plugin_off(id);
        state
            .persist_extension_state()
            .expect("the scratch extension state saves");
        assert!(
            state.draft().global.extensions[0].is_trusted(),
            "disabling leaves the package trusted"
        );
        assert!(
            state
                .settings
                .workspace
                .disabled
                .extensions
                .contains(&id.to_string()),
            "disabling is the workspace list"
        );
        assert!(
            extension_servers(&state.settings).is_empty(),
            "a trusted but disabled package derives no server"
        );
        assert_eq!(state.mcp.tools().count(), 0, "and its tools are gone");

        // Repointing the record is a different package to approve.
        state.draft_mut().workspace.disabled.extensions.clear();
        let moved = root.with_file_name("moved-elsewhere");
        state.draft_mut().global.extensions[0].path = Some(moved.to_string_lossy().to_string());
        assert!(
            !state.draft().global.extensions[0].is_trusted(),
            "a path change clears trust"
        );
        assert!(
            extension_servers(state.draft()).is_empty(),
            "and the moved record derives no server"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Toggling a package on must not run commands the reader never approved:
    /// the enabled list is one condition of the gate, not the gate.
    #[test]
    fn toggling_an_untrusted_mcp_plugin_spawns_nothing() {
        let marker =
            std::env::temp_dir().join(format!("bsh-trust-marker-toggle-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let root = package_with(
            "toggle",
            serde_json::json!({
                "command": "/bin/sh",
                "args": ["-c", format!("touch '{}'", marker.display())]
            }),
        );
        let id = "trust-toggle";
        let mut state = state_with("toggle", record(id, &root));
        state
            .settings
            .workspace
            .disabled
            .extensions
            .push(id.to_string());
        // The pair of writes the pane's toggle makes.
        state.toggle_plugin_off(id);
        state
            .persist_extension_state()
            .expect("the scratch extension state saves");
        assert!(
            !state
                .settings
                .workspace
                .disabled
                .extensions
                .contains(&id.to_string()),
            "the premise: the package is now enabled"
        );
        assert!(
            !marker.exists(),
            "enabling an untrusted package must not spawn its command"
        );
        assert!(extension_servers(&state.settings).is_empty());
        assert_eq!(state.mcp.tools().count(), 0);

        drop(state);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&marker);
    }

    /// The trust ring records the decision and the exact command line, and
    /// never the environment VALUE the package declares.
    #[test]
    fn a_trust_decision_is_audited_without_env_values() {
        let root = package_with(
            "audit",
            serde_json::json!({
                "command": "/bin/sh",
                "args": ["-c", "exit 0"],
                "env": { "FIXTURE_TOKEN": "sk-fixture-secret" }
            }),
        );
        let mut state = state_with("audit", record("trust-audit", &root));
        assert!(state.decline_trust("trust-audit"), "declining is recorded");
        let audit = state.trust_audit();
        assert_eq!(audit.len(), 1);
        assert!(!audit[0].allow, "a cancelled dialog is a refusal");
        assert_eq!(audit[0].extension_id, "trust-audit");
        assert_eq!(audit[0].path, root.to_string_lossy());
        assert!(
            audit[0]
                .commands
                .iter()
                .any(|command| command.contains("/bin/sh")),
            "the exact command line is kept: {:?}",
            audit[0].commands
        );
        assert!(
            !format!("{audit:?}").contains("sk-fixture-secret"),
            "an env value never reaches the audit: {audit:?}"
        );
        assert!(audit[0].at > 0, "the decision is stamped");

        assert!(
            state.trust_extension("trust-audit"),
            "the approval is recorded too"
        );
        let audit = state.trust_audit();
        assert_eq!(audit.len(), 2, "one row per decision");
        assert!(audit[1].allow);
        assert!(!format!("{audit:?}").contains("sk-fixture-secret"));

        drop(state);
        let _ = std::fs::remove_dir_all(&root);
    }
}
