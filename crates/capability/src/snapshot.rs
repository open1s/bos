//! Immutable composition snapshots.
//!
//! A snapshot answers "what did this workspace actually get, at which
//! versions, from which providers, and under which policy" after the fact
//! (RFC section 8.3). It is deliberately made of identifiers, versions, and
//! origins: it can be serialized and shared without carrying a credential, a
//! secret, or an argument.

use serde::{Deserialize, Serialize};

use crate::composition::{Binding, ResolvedComposition};

/// One capability as resolved into a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    /// The capability id, in its provider-qualified form.
    pub id: String,
    /// The exact version that was resolved.
    pub version: String,
    /// The provider that offers it.
    pub provider: String,
    /// Where it executes.
    pub origin: SnapshotOrigin,
}

/// Where a snapshotted capability executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnapshotOrigin {
    /// This Node, through the named provider.
    Local {
        /// The provider id.
        provider: String,
    },
    /// A peer, under a grant.
    Remote {
        /// The peer's Node id.
        node: String,
        /// The grant the binding runs under.
        grant: String,
    },
}

/// An immutable record of one resolved composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionSnapshot {
    /// The composition this plan belongs to.
    pub composition: String,
    /// The composition version that was resolved.
    pub version: String,
    /// The workspace it instantiated.
    pub workspace: String,
    /// When it was taken, in milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    /// Every capability, in resolution order.
    pub capabilities: Vec<CapabilitySnapshot>,
    /// A reference to the policy decision that admitted the plan, never the
    /// policy body.
    pub policy: Option<String>,
}

impl CompositionSnapshot {
    /// Record a resolved composition.
    pub fn from_resolved(
        resolved: &ResolvedComposition,
        policy: Option<String>,
        created_at_ms: u64,
    ) -> Self {
        Self {
            composition: resolved.id.to_string(),
            version: resolved.version.to_string(),
            workspace: resolved.workspace.to_string(),
            created_at_ms,
            capabilities: resolved
                .capabilities
                .iter()
                .map(|capability| CapabilitySnapshot {
                    id: capability.descriptor.id.to_string(),
                    version: capability.descriptor.version.to_string(),
                    provider: capability.descriptor.id.provider.to_string(),
                    origin: match &capability.binding {
                        Binding::Local { provider } => SnapshotOrigin::Local {
                            provider: provider.to_string(),
                        },
                        Binding::Remote { node, grant } => SnapshotOrigin::Remote {
                            node: node.to_string(),
                            grant: grant.as_str().to_string(),
                        },
                    },
                })
                .collect(),
            policy,
        }
    }

    /// Whether every capability executes on this Node.
    pub fn is_local_only(&self) -> bool {
        self.capabilities
            .iter()
            .all(|capability| matches!(capability.origin, SnapshotOrigin::Local { .. }))
    }

    /// The capabilities bound to a peer.
    pub fn remote_capabilities(&self) -> impl Iterator<Item = &CapabilitySnapshot> {
        self.capabilities
            .iter()
            .filter(|capability| matches!(capability.origin, SnapshotOrigin::Remote { .. }))
    }
}
