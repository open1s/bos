//! Resource metadata: identity, kind, state, serializable info.

use rkyv::{Archive, Deserialize, Serialize};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

/// The six classes of manageable computer resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SerdeSerialize, SerdeDeserialize)]
pub enum ResourceType {
    /// Storage: files, directories, block devices, object stores.
    Storage,
    /// Network: sockets, endpoints, message buses, streams.
    Network,
    /// Compute: processes, threads, containers, tasks.
    Compute,
    /// System: env, signals, pipes, shared memory, devices.
    System,
    /// Abstract/virtual: databases, KV stores, agent memory, remote RPC.
    Abstract,
    /// Combine: a virtual node that aggregates multiple child nodes into a single super virtual node.
    Combine,
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ResourceType::Storage => "storage",
            ResourceType::Network => "network",
            ResourceType::Compute => "compute",
            ResourceType::System => "system",
            ResourceType::Abstract => "abstract",
            ResourceType::Combine => "combine",
        };
        f.write_str(s)
    }
}

/// Serializable projection of a resource's runtime state for agents/policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SerdeSerialize, SerdeDeserialize, Archive, Serialize, Deserialize)]
#[rkyv(derive(Debug))]
pub enum ResourceStateLabel {
    Provisioning,
    Open,
    Closed,
    Error,
}

/// Stable, serializable metadata describing a registered resource.
///
/// `uri` is the canonical address (e.g. `file:///tmp/x`, `proc://1234`).
#[derive(Debug, Clone, SerdeSerialize, SerdeDeserialize)]
pub struct ResourceMeta {
    /// Canonical URI used for addressing and routing.
    pub uri: String,
    /// The resource class.
    pub kind: ResourceType,
    /// Current runtime state.
    pub state: ResourceStateLabel,
    /// Owning agent identity (cert CN / handle-bound id).
    pub owner: String,
    /// Optional free-form metadata.
    pub metadata: Option<serde_json::Value>,
}

/// Lightweight snapshot for discovery (`list`/`resolve`).
#[derive(Debug, Clone, SerdeSerialize, SerdeDeserialize)]
pub struct ResourceInfo {
    pub uri: String,
    pub kind: ResourceType,
    pub state: ResourceStateLabel,
    pub owner: String,
}