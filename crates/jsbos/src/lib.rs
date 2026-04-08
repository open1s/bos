use napi_derive::napi;

mod agent;
mod bus;
mod caller;
mod config;
mod jsany;
mod mcp;
mod publisher;
mod query;
mod subscriber;
mod utils;

pub use agent::{Agent, AgentCallableServer, AgentConfig, AgentRpcClient};
pub use bus::{Bus, BusConfig, Session};
pub use caller::{Callable, Caller};
pub use config::ConfigLoader;
pub use mcp::McpClient;
pub use publisher::Publisher;
pub use query::{Query, Queryable};
pub use subscriber::Subscriber;

// Note: logging is a dependency but used in binaries

#[napi]
pub fn version() -> String {
  env!("CARGO_PKG_VERSION").to_string()
}
