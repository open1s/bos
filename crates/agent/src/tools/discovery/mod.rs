//! Discovery implementations for various tool sources

mod base;
mod zenoh_rpc;
mod mcp;
mod a2a;

pub use base::{ToolDiscovery, DiscoveredTool, ToolSource, LocalDiscovery};
pub use zenoh_rpc::ZenohRpcDiscovery;
pub use mcp::McpDiscovery;
pub use a2a::A2AToolDiscovery;
