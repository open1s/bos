//! Demo showing agent hooks and plugins
//!
//! Run with: cargo run -p agent --example demo_hooks_and_plugins

use agent::agent::agentic::{Agent, AgentConfig, LlmProvider};
use agent::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent};
use agent::agent::plugin::{AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, ToolCallWrapper, ToolResultWrapper};
use async_trait::async_trait;
use config::ConfigLoader;
use react::llm::vendor::{NvidiaVendor, OpenRouterVendor};
use std::sync::{Arc, Mutex};

/// Test hook that tracks events and prints them
#[derive(Debug, Clone)]
struct TestHook {
    events: Arc<Mutex<Vec<HookEvent>>>,
}

impl TestHook {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl AgentHook for TestHook {
    async fn on_event(&self, event: HookEvent, context: &HookContext) -> HookDecision {
        let mut events = self.events.lock().unwrap();
        events.push(event.clone());
        println!("  [Hook fired] {:?}", event);
        if let Some(tool_name) = context.get("tool_name") {
            println!("    tool_name: {}", tool_name);
        }
        if let Some(model) = context.get("model") {
            println!("    model: {}", model);
        }
        HookDecision::Continue
    }
}

/// Test plugin that tracks calls and prints them
#[derive(Debug, Clone)]
struct TestPlugin {
    calls: Arc<Mutex<Vec<String>>>,
}

impl TestPlugin {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl AgentPlugin for TestPlugin {
    fn name(&self) -> &str {
        "TestPlugin"
    }

    async fn on_llm_request(&self, request: LlmRequestWrapper) -> Option<LlmRequestWrapper> {
        let mut calls = self.calls.lock().unwrap();
        calls.push("on_llm_request".to_string());
        println!("  [Plugin:on_llm_request] model={}", request.model);
        println!("    input: {}", &request.input[..request.input.len().min(100)]);
        // Return Some to indicate we want to pass the request through
        // Return None to intercept (but we want to pass through for demo)
        Some(request)
    }

    async fn on_llm_response(&self, response: LlmResponseWrapper) -> Option<LlmResponseWrapper> {
        let mut calls = self.calls.lock().unwrap();
        calls.push("on_llm_response".to_string());
        println!("  [Plugin:on_llm_response] called");
        Some(response)
    }

    async fn on_tool_call(&self, tool_call: ToolCallWrapper) -> Option<ToolCallWrapper> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(format!("on_tool_call:{}", tool_call.name));
        println!("  [Plugin:on_tool_call] name={}", tool_call.name);
        Some(tool_call)
    }

    async fn on_tool_result(&self, tool_result: ToolResultWrapper) -> Option<ToolResultWrapper> {
        let mut calls = self.calls.lock().unwrap();
        calls.push("on_tool_result".to_string());
        println!("  [Plugin:on_tool_result] success={}", tool_result.success);
        Some(tool_result)
    }
}

struct VendorConfig {
    model: String,
    base_url: String,
    api_key: String,
}

impl VendorConfig {
    fn from_nvidia(config: &serde_json::Value) -> Option<Self> {
        let nvidia = config.get("llm")?.get("nvidia")?;
        Some(Self {
            model: nvidia.get("model")?.as_str()?.to_string(),
            base_url: nvidia.get("base_url")?.as_str()?.to_string(),
            api_key: nvidia.get("api_key")?.as_str()?.to_string(),
        })
    }
    fn from_openrouter(config: &serde_json::Value) -> Option<Self> {
        let or = config.get("llm")?.get("openrouter")?;
        Some(Self {
            model: or.get("model")?.as_str()?.to_string(),
            base_url: or.get("base_url")?.as_str()?.to_string(),
            api_key: or.get("api_key")?.as_str()?.to_string(),
        })
    }
}

fn build_llm_provider(config: &serde_json::Value) -> LlmProvider {
    let mut provider = LlmProvider::new();

    if let Some(cfg) = VendorConfig::from_nvidia(config) {
        let model = cfg.model.strip_prefix("nvidia/").unwrap_or(&cfg.model);
        println!("Registering nvidia: {} @ {}", model, cfg.base_url);
        let v = NvidiaVendor::new(cfg.base_url.clone(), model.to_string(), cfg.api_key.clone());
        provider.register_vendor("nvidia".into(), Box::new(v));
    }

    if let Some(cfg) = VendorConfig::from_openrouter(config) {
        let model = cfg.model.strip_prefix("openrouter/").unwrap_or(&cfg.model);
        println!("Registering openrouter: {} @ {}", model, cfg.base_url);
        let v = OpenRouterVendor::new(cfg.base_url, model.to_string(), cfg.api_key);
        provider.register_vendor("openrouter".into(), Box::new(v));
    }

    provider
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Agent Hooks and Plugins Demo ===\n");

    // Load config
    let mut loader = ConfigLoader::new().discover();
    if loader.sources().is_empty() {
        return Err("No config. Create ~/.bos/conf/config.toml with [llm.nvidia]".into());
    }
    let config = loader.load_sync().map_err(|e| e.to_string())?;

    let provider = build_llm_provider(&config);

    let nvidia_cfg = VendorConfig::from_nvidia(&config)
        .ok_or("no llm.nvidia config")?;

    // Create agent config
    let mut agent_config = AgentConfig::default();
    agent_config.model = nvidia_cfg.model;
    agent_config.timeout_secs = 120;

    // Create agent
    let agent = Agent::new(agent_config, Arc::new(provider));

    // Create and register test hook
    let test_hook = Arc::new(TestHook::new());
    println!("Registering test hook for BeforeLlmCall, AfterLlmCall, BeforeToolCall, AfterToolCall...");
    agent.hooks().register_blocking(HookEvent::BeforeLlmCall, test_hook.clone());
    agent.hooks().register_blocking(HookEvent::AfterLlmCall, test_hook.clone());
    agent.hooks().register_blocking(HookEvent::BeforeToolCall, test_hook.clone());
    agent.hooks().register_blocking(HookEvent::AfterToolCall, test_hook.clone());
    println!("Hook registered.\n");

    // Create and register test plugin
    let test_plugin = Arc::new(TestPlugin::new());
    println!("Registering test plugin (on_llm_request, on_llm_response, on_tool_call, on_tool_result)...");
    agent.plugins().register_blocking(test_plugin.clone());
    println!("Plugin registered.\n");

    // Run a simple task
    println!("=== Running agent.run_simple('What is 2+2?') ===\n");
    match agent.run_simple("What is 2+2?").await {
        Ok(response) => println!("\nResponse: {}\n", response),
        Err(e) => println!("\nError: {}\n", e),
    }

    // Check what was called
    let hook_events = test_hook.events.lock().unwrap();
    let plugin_calls = test_plugin.calls.lock().unwrap();

    println!("=== Summary ===");
    println!("Hook events fired ({}): {:?}", hook_events.len(), *hook_events);
    println!("Plugin methods called ({}): {:?}", plugin_calls.len(), *plugin_calls);

    if hook_events.is_empty() {
        println!("\n[WARNING] No hooks were fired! Something is wrong.");
    } else {
        println!("\n[OK] Hooks are working correctly.");
    }

    if plugin_calls.is_empty() {
        println!("[BUG] No plugin methods were called!");
        println!("       The PluginRegistry exists but plugins.on_llm_request etc. are NEVER INVOKED.");
        println!("       This is the bug - plugins need to be called from the agent/reactor code.");
    } else {
        println!("[OK] Plugins are working correctly.");
    }

    println!("\n=== Done ===");
    Ok(())
}