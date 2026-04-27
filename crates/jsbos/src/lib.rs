use napi_derive::napi;

mod agent;
mod bus;
mod caller;
mod config;
mod hooks;
mod jsany;
mod llm_usage;
mod mcp;
mod plugin;
mod publisher;
mod query;
mod subscriber;
mod utils;

pub use agent::{Agent, AgentCallableServer, AgentConfig, AgentRpcClient};
pub use bus::{Bus, BusConfig, Session};
pub use caller::{Callable, Caller};
pub use config::ConfigLoader;
pub use hooks::{HookContextData, HookDecision, HookEvent, HookRegistry};
pub use llm_usage::{LlmUsage, PromptTokensDetails};
pub use mcp::McpClient;
pub use plugin::{
  PluginLlmRequest, PluginLlmResponse, PluginRegistry, PluginStage, PluginToolCall,
  PluginToolResult,
};
pub use publisher::Publisher;
pub use query::{Query, Queryable};
pub use subscriber::Subscriber;

// Note: logging is a dependency but used in binaries

#[napi]
pub fn version() -> String {
  env!("CARGO_PKG_VERSION").to_string()
}

#[napi]
pub fn init_tracing() {
  logging::auto_init_tracing();
}

#[napi]
pub fn log_test_message(message: String) {
  logging::log_test_message(&message);
}
