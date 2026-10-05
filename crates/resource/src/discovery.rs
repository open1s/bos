//! Node discovery protocol for bus-based auto-clustering.
//!
//! Each `rex serve` node publishes a [`NodeAnnounce`] on startup to a
//! well-known bus topic (`bos/discovery/{node_id}`) and subscribes to
//! `bos/discovery/*` to learn about peers. After receiving an announcement,
//! the node constructs a super vnode aggregating all known peers.
//!
//! # Security
//!
//! Announcements can be optionally signed with HMAC-SHA256 using a shared
//! secret. When `shared_secret` is configured, peers verify the signature
//! before accepting an announcement, preventing bus spoofing.

use rkyv::{Archive, Deserialize, Serialize};

use crate::meta::{ResourceInfo, ResourceStateLabel, ResourceType};

/// Well-known bus topic prefix for node discovery.
pub const DISCOVERY_TOPIC: &str = "bos/discovery";

/// Build the discovery topic for a specific node.
pub fn discovery_topic(node_id: &str) -> String {
    format!("{DISCOVERY_TOPIC}/{node_id}")
}

/// A snapshot of a node's current load, kept to primitives so the bus codec
/// needs no serde. All counters are best-effort and informational.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
pub struct LoadSnapshot {
    /// Number of resources the node hosts (registered + auto-bound).
    pub num_resources: u32,
    /// Number of running child processes (proc manager).
    pub num_procs: u32,
    /// Seconds since the node process started.
    pub uptime_secs: u64,
}

impl LoadSnapshot {
    pub fn zero() -> Self {
        Self {
            num_resources: 0,
            num_procs: 0,
            uptime_secs: 0,
        }
    }
}

/// A node's announcement message, published on the bus when the node starts.
/// Uses only rkyv-compatible primitives (strings + tuples) so it can be
/// serialized by the bus codec without serde.
#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct NodeAnnounce {
    /// Unique node identifier (e.g. "nodeA").
    pub node_id: String,
    /// QUIC address reachable from other nodes (e.g. "10.0.0.1:4433").
    pub quic_addr: String,
    /// Server name (SNI / cert CN) that clients must use to connect to this node.
    pub server_name: String,
    /// Resources hosted by this node, as `(uri, kind, state, owner)` tuples.
    pub resources: Vec<(String, String, String, String)>,
    /// PEM-encoded CA certificate, so peers can verify this node's client certs.
    pub ca_cert: String,
    /// Feature tags this node supports (e.g. "stream", "proc", "supervisor",
    /// "relay", "vnode"). Directory consumers match on these instead of
    /// assuming every node has every subsystem.
    pub capabilities: Vec<String>,
    /// Current load snapshot (informational; refreshed on re-announce).
    pub load: LoadSnapshot,
    /// HMAC-SHA256 signature of `(node_id || quic_addr || server_name)`,
    /// or empty if unsigned.
    pub signature: Vec<u8>,
}

impl NodeAnnounce {
    /// Build an announce from typed fields, converting to string tuples.
    pub fn new(
        node_id: String,
        quic_addr: String,
        server_name: String,
        resources: Vec<ResourceInfo>,
        ca_cert: String,
    ) -> Self {
        Self {
            node_id,
            quic_addr,
            server_name,
            resources: resources
                .into_iter()
                .map(|r| (r.uri, r.kind.to_string(), format!("{:?}", r.state), r.owner))
                .collect(),
            ca_cert,
            capabilities: Vec::new(),
            load: LoadSnapshot::zero(),
            signature: Vec::new(),
        }
    }

    /// Set capability tags (replaces any prior set).
    pub fn with_capabilities(mut self, caps: impl IntoIterator<Item = String>) -> Self {
        self.capabilities = caps.into_iter().collect();
        self
    }

    /// Record a load snapshot.
    pub fn with_load(mut self, load: LoadSnapshot) -> Self {
        self.load = load;
        self
    }

    /// Sign this announcement with HMAC-SHA256 using the shared secret.
    /// The signature covers `(node_id || quic_addr || server_name)`.
    pub fn sign(&mut self, secret: &[u8]) {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
        mac.update(self.node_id.as_bytes());
        mac.update(self.quic_addr.as_bytes());
        mac.update(self.server_name.as_bytes());
        self.signature = mac.finalize().into_bytes().to_vec();
    }

    /// Verify the HMAC signature. Returns `true` only if the signature is
    /// valid. Empty/missing signatures are **never** valid — nodes that
    /// configure a shared_secret reject unsigned announcements.
    pub fn verify(&self, secret: &[u8]) -> bool {
        if self.signature.is_empty() {
            return false; // unsigned — reject when secret is configured
        }
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
        mac.update(self.node_id.as_bytes());
        mac.update(self.quic_addr.as_bytes());
        mac.update(self.server_name.as_bytes());
        mac.verify_slice(&self.signature).is_ok()
    }

    /// Convert string tuples back to typed [`ResourceInfo`].
    pub fn to_resources(&self) -> Vec<ResourceInfo> {
        self.resources
            .iter()
            .map(|(uri, kind, state, owner)| ResourceInfo {
                uri: uri.clone(),
                kind: parse_kind(kind),
                state: parse_state(state),
                owner: owner.clone(),
            })
            .collect()
    }
}

fn parse_kind(s: &str) -> ResourceType {
    match s {
        "storage" => ResourceType::Storage,
        "network" => ResourceType::Network,
        "compute" => ResourceType::Compute,
        "system" => ResourceType::System,
        "abstract" => ResourceType::Abstract,
        "combine" => ResourceType::Combine,
        _ => ResourceType::Abstract,
    }
}

fn parse_state(s: &str) -> ResourceStateLabel {
    match s {
        "Open" => ResourceStateLabel::Open,
        "Closed" => ResourceStateLabel::Closed,
        "Error" => ResourceStateLabel::Error,
        _ => ResourceStateLabel::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_announce() {
        let announce = NodeAnnounce::new(
            "nodeA".into(),
            "10.0.0.1:4433".into(),
            "myserver".into(),
            vec![ResourceInfo {
                uri: "mem://store".into(),
                kind: ResourceType::Abstract,
                state: ResourceStateLabel::Open,
                owner: "agent1".into(),
            }],
            String::new(),
        );

        let resources = announce.to_resources();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].uri, "mem://store");
        assert_eq!(resources[0].kind, ResourceType::Abstract);
        assert_eq!(resources[0].state, ResourceStateLabel::Open);
        assert_eq!(resources[0].owner, "agent1");
    }

    #[test]
    fn discovery_topic_format() {
        assert_eq!(discovery_topic("nodeA"), "bos/discovery/nodeA");
    }

    #[test]
    fn rich_directory_fields_roundtrip() {
        let a = NodeAnnounce::new(
            "nodeX".into(),
            "10.0.0.9:4433".into(),
            "xserver".into(),
            vec![],
            String::new(),
        )
        .with_capabilities(vec!["stream".into(), "proc".into()])
        .with_load(LoadSnapshot {
            num_resources: 12,
            num_procs: 3,
            uptime_secs: 99,
        });
        assert_eq!(a.capabilities, vec!["stream", "proc"]);
        assert_eq!(a.load.num_resources, 12);
        assert_eq!(a.load.num_procs, 3);
        assert_eq!(a.load.uptime_secs, 99);
    }

    #[test]
    fn sign_and_verify() {
        let secret = b"my-shared-secret";
        let mut announce = NodeAnnounce::new(
            "nodeA".into(),
            "10.0.0.1:4433".into(),
            "myserver".into(),
            vec![ResourceInfo {
                uri: "mem://store".into(),
                kind: ResourceType::Abstract,
                state: ResourceStateLabel::Open,
                owner: "agent1".into(),
            }],
            String::new(),
        );

        // Unsigned: verify returns false (reject when secret is configured).
        assert!(!announce.verify(secret));

        // Sign and verify.
        announce.sign(secret);
        assert!(!announce.signature.is_empty());
        assert!(announce.verify(secret));

        // Wrong secret fails.
        assert!(!announce.verify(b"wrong-secret"));
    }
}
