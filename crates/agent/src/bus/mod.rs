//! Agent-level RPC over the bus, split by seam:
//! wire format, transport, client, server, and the agent-as-tool adapter.

mod client;
mod server;
mod tool;
mod transport;
mod wire;

pub use client::AgentRpcClient;
pub use react::ToolError;
pub use server::AgentCallableServer;
pub use tool::AgentCallerTool;
