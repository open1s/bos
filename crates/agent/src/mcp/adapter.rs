use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use react::tool::registry::AsyncTool;
use react::tool::{Tool, ToolError, ToolMetadata};

use super::client::McpClient;
use super::policy::{McpDecision, McpPolicy};

/// The default per-call bound for an MCP invocation (issue #331).
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// One governance decision, recorded for the audit trail (issue #330).
#[derive(Debug, Clone)]
pub struct McpCallRecord {
    /// The MCP server name.
    pub server: String,
    /// The remote tool name.
    pub tool: String,
    /// The policy decision: allow, deny or ask.
    pub decision: &'static str,
    /// Whether an Ask decision was approved by the user.
    pub approved: bool,
}

/// The approval flow for an Ask decision: (server, tool) -> approved.
pub type McpApproval = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// The audit sink every governance decision is recorded through.
pub type McpAudit = Arc<dyn Fn(&McpCallRecord) + Send + Sync>;

/// The per-server/per-tool governance enforced BEFORE an MCP request is sent
/// (issue #330). Both the direct adapter and the gateway route through
/// McpGovernance::check, so no dispatch path bypasses it.
#[derive(Clone)]
pub struct McpGovernance {
    /// The allow/deny/ask policy.
    pub policy: McpPolicy,
    /// The approval flow for an Ask decision. None means Ask refuses: an
    /// unattended call is never silently allowed.
    pub approve: Option<McpApproval>,
    /// The audit sink for every decision.
    pub audit: Option<McpAudit>,
}

impl std::fmt::Debug for McpGovernance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpGovernance")
            .field("policy", &self.policy)
            .field("approve", &self.approve.is_some())
            .field("audit", &self.audit.is_some())
            .finish()
    }
}

impl McpGovernance {
    /// The legacy ungoverned default: allow everything, no audit. Kept so the
    /// original constructor stays source-compatible; a host that exposes MCP
    /// tools to a model should install a real policy with [`Self::check`].
    pub fn permissive() -> Self {
        Self {
            policy: McpPolicy::with_default(McpDecision::Allow),
            approve: None,
            audit: None,
        }
    }

    /// Enforce the policy for one server/tool pair.
    ///
    /// Deny refuses before any request. Ask routes through the approval flow
    /// and refuses when there is none or it declines. Every outcome is
    /// recorded in the audit sink.
    pub fn check(&self, server: &str, tool: &str) -> Result<(), ToolError> {
        let decision = self.policy.decide(server, tool);
        let (label, approved) = match decision {
            McpDecision::Allow => ("allow", true),
            McpDecision::Deny => ("deny", false),
            McpDecision::Ask => {
                let approved = self
                    .approve
                    .as_ref()
                    .map(|approve| approve(server, tool))
                    .unwrap_or(false);
                ("ask", approved)
            }
        };
        if let Some(audit) = &self.audit {
            audit(&McpCallRecord {
                server: server.to_string(),
                tool: tool.to_string(),
                decision: label,
                approved,
            });
        }
        match decision {
            McpDecision::Allow => Ok(()),
            McpDecision::Deny => Err(ToolError::Failed(format!(
                "MCP call {server}/{tool} denied by policy"
            ))),
            McpDecision::Ask if approved => Ok(()),
            McpDecision::Ask => Err(ToolError::Failed(format!(
                "MCP call {server}/{tool} requires approval that was not granted"
            ))),
        }
    }
}

/// Adapts a remote MCP tool to the local [`Tool`] trait.
pub struct McpToolAdapter {
    client: Arc<McpClient>,
    registry_name: String,
    mcp_tool_name: String,
    server_name: String,
    description: String,
    input_schema: serde_json::Value,
    metadata: ToolMetadata,
    governance: Arc<McpGovernance>,
    call_timeout: Duration,
}

impl McpToolAdapter {
    /// Wrap `mcp_tool_name` from `client`, exposed locally as `registry_name`.
    ///
    /// Uses the permissive governance default; a host wires a real policy
    /// with [`Self::with_governance`].
    pub fn new(
        client: Arc<McpClient>,
        registry_name: String,
        mcp_tool_name: String,
        description: String,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            client,
            registry_name,
            mcp_tool_name,
            server_name: "mcp".to_string(),
            description,
            input_schema,
            metadata: mcp_tool_metadata(),
            governance: Arc::new(McpGovernance::permissive()),
            call_timeout: DEFAULT_CALL_TIMEOUT,
        }
    }

    /// Name the MCP server this tool belongs to, for governance and audit.
    pub fn with_server(mut self, server: impl Into<String>) -> Self {
        self.server_name = server.into();
        self
    }

    /// Install the governance enforced before every invocation.
    pub fn with_governance(mut self, governance: Arc<McpGovernance>) -> Self {
        self.governance = governance;
        self
    }

    /// Set the per-call bound (issue #331).
    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }
}

/// What an MCP tool declares about itself.
///
/// An MCP tool always reaches the network; the server decides what it does
/// there. It carries no version of its own, so the catalogue records
/// `Unknown` rather than inventing one. Risk and permissions are conservative
/// because the declaration cannot be more specific than "a remote party runs
/// this".
pub fn mcp_tool_metadata() -> ToolMetadata {
    ToolMetadata::new()
        .with_risk("medium")
        .with_side_effects(["network"])
        .with_permissions(["net.egress"])
}

impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn metadata(&self) -> Option<&ToolMetadata> {
        Some(&self.metadata)
    }

    fn run(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.governance
            .check(&self.server_name, &self.mcp_tool_name)?;
        let client = self.client.clone();
        let tool_name = self.mcp_tool_name.clone();
        let args_clone = args.clone();

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("Failed to create runtime");
            let result = rt.block_on(client.call_tool(&tool_name, args_clone));
            let _ = tx.send(result);
        });

        // The sync path is bounded too: the client enforces its own deadline,
        // and this outer bound covers the channel handoff (issue #331).
        rx.recv_timeout(self.call_timeout + Duration::from_secs(5))
            .map_err(|_| {
                ToolError::Timeout(format!(
                    "MCP call {} exceeded {}s",
                    self.mcp_tool_name,
                    self.call_timeout.as_secs()
                ))
            })?
            .map_err(|e| ToolError::Failed(e.to_string()))
    }
}

// AsyncTool implementation - proper async execution
#[async_trait]
impl AsyncTool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn metadata(&self) -> Option<&ToolMetadata> {
        Some(&self.metadata)
    }

    fn json_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    fn to_openai_definition(&self) -> react::tool::ToolDefinition {
        use react::tool::{ToolDefinition, ToolParameterProperty, ToolParameters};
        use std::collections::HashMap;

        let parameters = if let Ok(params) =
            serde_json::from_value::<ToolParameters>(self.input_schema.clone())
        {
            params
        } else {
            let mut tool_params = ToolParameters {
                param_type: "object".to_string(),
                properties: Some(HashMap::new()),
                required: None,
            };

            if let Some(obj) = self.input_schema.as_object() {
                if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                    tool_params.param_type = t.to_string();
                }
                if let Some(arr) = obj.get("required").and_then(|v| v.as_array()) {
                    tool_params.required = Some(
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                    );
                }
                if let Some(props) = obj.get("properties").and_then(|v| v.as_object()) {
                    let mut properties = HashMap::new();
                    for (key, val) in props {
                        let param_prop = ToolParameterProperty {
                            param_type: val
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("string")
                                .to_string(),
                            description: val
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            enum_values: None,
                        };
                        properties.insert(key.clone(), param_prop);
                    }
                    tool_params.properties = Some(properties);
                }
            }
            tool_params
        };

        ToolDefinition::new(&self.registry_name, &self.description).with_parameters(parameters)
    }

    async fn run(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.governance
            .check(&self.server_name, &self.mcp_tool_name)?;
        self.client
            .call_tool(&self.mcp_tool_name, args.clone())
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))
    }
}

#[cfg(test)]
mod governance_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tool() -> McpToolAdapter {
        // A client that is never called: governance refuses first.
        let client = Arc::new(McpClient::connect_http("http://127.0.0.1:1"));
        McpToolAdapter::new(
            client,
            "srv_tool".to_string(),
            "tool".to_string(),
            "desc".to_string(),
            serde_json::json!({}),
        )
        .with_server("srv")
    }

    #[test]
    fn deny_refuses_before_any_request() {
        let mut policy = McpPolicy::new();
        policy.deny("srv", "tool");
        let governance = McpGovernance {
            policy,
            approve: None,
            audit: None,
        };
        let error = governance.check("srv", "tool").unwrap_err();
        assert!(error.to_string().contains("denied"), "{error}");
    }

    #[test]
    fn allow_reaches_the_call() {
        let mut policy = McpPolicy::new();
        policy.allow("srv", "tool");
        let governance = McpGovernance {
            policy,
            approve: None,
            audit: None,
        };
        assert!(governance.check("srv", "tool").is_ok());
    }

    #[test]
    fn ask_without_approval_refuses() {
        let governance = McpGovernance {
            policy: McpPolicy::new(),
            approve: None,
            audit: None,
        };
        let error = governance.check("srv", "tool").unwrap_err();
        assert!(error.to_string().contains("approval"), "{error}");
    }

    #[test]
    fn ask_with_approval_permits_and_audits() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(std::sync::Mutex::new(Vec::<McpCallRecord>::new()));
        let audit_calls = calls.clone();
        let audit_seen = seen.clone();
        let governance = McpGovernance {
            policy: McpPolicy::new(),
            approve: Some(Arc::new(|server, tool| server == "srv" && tool == "tool")),
            audit: Some(Arc::new(move |record: &McpCallRecord| {
                audit_calls.fetch_add(1, Ordering::SeqCst);
                audit_seen.lock().unwrap().push(record.clone());
            })),
        };
        assert!(governance.check("srv", "tool").is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].decision, "ask");
        assert!(seen[0].approved);
    }

    #[test]
    fn the_adapter_enforces_governance_on_both_paths() {
        let mut policy = McpPolicy::new();
        policy.deny("srv", "tool");
        let adapter = tool().with_governance(Arc::new(McpGovernance {
            policy,
            approve: None,
            audit: None,
        }));
        // Sync path refuses without touching the (unreachable) server.
        let error = Tool::run(&adapter, &serde_json::json!({})).unwrap_err();
        assert!(error.to_string().contains("denied"), "{error}");
        // Async path shares the same enforcement point.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let error = rt
            .block_on(AsyncTool::run(&adapter, &serde_json::json!({})))
            .unwrap_err();
        assert!(error.to_string().contains("denied"), "{error}");
    }
}
