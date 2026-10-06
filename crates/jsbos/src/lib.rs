//! Node.js bindings for the bos agent framework.
//!
//! Exposes the agent, bus, configuration, MCP, and plugin surfaces as napi
//! classes so JavaScript programs can build agents and message over the bus.
#![warn(missing_docs)]

use napi::threadsafe_function::ThreadsafeFunction;
use napi::Unknown;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// A lock-protected slot holding an optional JavaScript string handler.
pub(crate) type StringHandlerSlot =
  Arc<Mutex<Option<Arc<ThreadsafeFunction<String, Unknown<'static>>>>>>;

mod agent;
mod bus;
mod caller;
mod config;
mod hooks;
mod jsany;
mod llm_usage;
mod mcp;
mod perf;
mod plugin;
mod publisher;
mod query;
mod subscriber;

pub use agent::{Agent, AgentCallableServer, AgentConfig, AgentRpcClient};
pub use bus::{Bus, BusConfig, Session};
pub use caller::{Callable, Caller};
pub use config::ConfigLoader;
pub use hooks::{HookContextData, HookDecision, HookEvent, HookRegistry};
pub use llm_usage::{BudgetStatus, LlmUsage, PromptTokensDetails, TokenBudgetReport, TokenUsage};
pub use mcp::McpClient;
pub use plugin::{
  PluginLlmRequest, PluginLlmResponse, PluginRegistry, PluginStage, PluginToolCall,
  PluginToolResult,
};
pub use publisher::Publisher;
pub use query::{Query, Queryable};
pub use subscriber::Subscriber;

pub use perf::PerfSnapshot;

// Note: logging is a dependency but used in binaries

#[napi]
/// Return the binding version.
pub fn version() -> String {
  env!("CARGO_PKG_VERSION").to_string()
}

#[napi]
/// Initialize tracing for the process.
pub fn init_tracing() {
  logging::auto_init_tracing();
}

#[napi]
/// Emit a test log message.
pub fn log_test_message(message: String) {
  logging::log_test_message(&message);
}
