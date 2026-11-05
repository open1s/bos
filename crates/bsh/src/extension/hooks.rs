//! Re-exported from ferris-runtime; BSH installs its agent hook adapter here.
use std::sync::Arc;

pub(crate) use ferris_sdk::host_runtime::hooks::*;

struct HostHookProvider;

impl ferris_sdk::host_runtime::hook::HookProvider for HostHookProvider {
    fn targets(&self, point: &str) -> Vec<ferris_sdk::host_runtime::hook::HookTarget> {
        super::host::hook_targets(point)
            .into_iter()
            .map(|target| ferris_sdk::host_runtime::hook::HookTarget {
                id: target.id,
                runtime: target.runtime,
            })
            .collect()
    }

    fn attached(&self) -> bool {
        super::host::hook_attached()
    }
}

/// Install the process-global hook provider once.
pub(crate) fn install_host_provider() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        ferris_sdk::host_runtime::hook::install(Arc::new(HostHookProvider));
    });
}

/// Install the process-global HTTP tap for the built-in LLM vendors.
pub(crate) fn install_http_tap() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        react::llm::vendor::set_http_tap(Some(Arc::new(
            |exchange: react::llm::vendor::HttpExchange| {
                if !ferris_sdk::host_runtime::hook::attached() {
                    return;
                }
                // The tap already masked credential header values; a body is
                // masked here because a prompt can carry a key of its own.
                let request_headers: Vec<(String, String)> = exchange
                    .request_headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let response_headers: Vec<(String, String)> = exchange
                    .response_headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let request_body = exchange
                    .request_body
                    .as_deref()
                    .map(ferris_sdk::host_runtime::hooks::redact_payload);
                let response_body = exchange
                    .response_body
                    .as_deref()
                    .map(ferris_sdk::host_runtime::hooks::redact_payload);
                let event = ferris_sdk::host_runtime::hooks::http_exchange_event(
                    ferris_sdk::host_runtime::hooks::HttpEvent {
                        source: exchange.label,
                        method: &exchange.method,
                        url: &exchange.url,
                        status: exchange.status,
                        request_id: exchange.request_id.as_deref(),
                        request_bytes: exchange.request_bytes,
                        response_bytes: exchange.response_bytes,
                        content_type: exchange.content_type.as_deref(),
                        ttfb_ms: exchange.ttfb_ms,
                        total_ms: exchange.total_ms,
                        error: exchange.error.as_deref(),
                        request_headers: &request_headers,
                        response_headers: &response_headers,
                        request_body: request_body.as_deref(),
                        response_body: response_body.as_deref(),
                        body_truncated: exchange.body_truncated,
                    },
                );
                ferris_sdk::host_runtime::hooks::observe("http.exchange", event);
            },
        )));
    });
}

/// Register the agent hooks that forward BOS hooks into extension hooks.
pub(crate) fn install_agent_hooks(agent: &mut agent::Agent) {
    if !ferris_sdk::host_runtime::hook::attached() {
        return;
    }
    let hook = Arc::new(ExtensionAgentHook);
    for event in [
        agent::HookEvent::BeforeLlmCall,
        agent::HookEvent::AfterLlmCall,
        agent::HookEvent::BeforeToolCall,
        agent::HookEvent::AfterToolCall,
    ] {
        agent.add_hook(event, Arc::clone(&hook) as Arc<dyn agent::AgentHook>);
    }
}

struct ExtensionAgentHook;

#[async_trait::async_trait]
impl agent::AgentHook for ExtensionAgentHook {
    async fn on_event(
        &self,
        event: agent::HookEvent,
        context: &agent::HookContext,
    ) -> agent::agent::hooks::HookDecision {
        let Some(point) = point_for(event) else {
            return agent::agent::hooks::HookDecision::Continue;
        };
        let mut data = serde_json::Map::new();
        for (key, value) in &context.data {
            data.insert(
                key.clone(),
                serde_json::Value::String(ferris_sdk::host_runtime::hooks::redact(value)),
            );
        }
        observe(
            point,
            serde_json::json!({ "agent": context.agent_id, "data": data }),
        );
        agent::agent::hooks::HookDecision::Continue
    }
}

fn point_for(event: agent::HookEvent) -> Option<&'static str> {
    use agent::HookEvent::*;
    match event {
        BeforeLlmCall => Some("model.request"),
        AfterLlmCall => Some("model.response"),
        BeforeToolCall => Some("tool.before"),
        AfterToolCall => Some("tool.after"),
        _ => None,
    }
}
