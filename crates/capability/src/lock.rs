//! Durable composition manifests and reproducible resolution locks.
//!
//! A spec alone is not reproducible: it names ranges and policy, while the
//! actual provider, version, peer, grant, and catalogue state live outside it.
//! A CompositionManifest is the versioned, serializable input; a
//! CompositionLock records exactly what one resolution selected, plus the
//! dependency order and the catalogue revisions the selection was made
//! against. Verifying a lock re-resolves and reports drift instead of silently
//! binding something new.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalogue::Catalogue;
use crate::composition::{
    resolve_host, Binding, Budgets, CapabilityRequirement, CompositionError, CompositionId,
    CompositionSpec, GrantRef, LocalScope, PeerCatalogue, ResolvedComposition,
};
use crate::id::{CapabilityId, NodeId, WorkspaceId};

/// The manifest schema this build writes and accepts.
pub const MANIFEST_SCHEMA: u32 = 1;
/// The lock schema this build writes and accepts.
pub const LOCK_SCHEMA: u32 = 1;

/// One explicit dependency between two declared requirements.
///
/// The dependent capability activates after the capability it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyEdge {
    /// The dependent capability.
    pub capability: CapabilityId,
    /// The capability it needs.
    pub depends_on: CapabilityId,
}

/// A versioned, serializable composition input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositionManifest {
    /// Schema version; only MANIFEST_SCHEMA is accepted.
    pub schema: u32,
    /// The composition specification.
    pub spec: CompositionSpec,
    /// Explicit dependencies between declared requirements.
    #[serde(default)]
    pub dependencies: Vec<DependencyEdge>,
}

impl CompositionManifest {
    /// Wrap a spec at the current schema with no explicit edges.
    pub fn new(spec: CompositionSpec) -> Self {
        Self {
            schema: MANIFEST_SCHEMA,
            spec,
            dependencies: Vec::new(),
        }
    }

    /// Add one dependency edge.
    pub fn with_dependency(mut self, capability: CapabilityId, depends_on: CapabilityId) -> Self {
        self.dependencies.push(DependencyEdge {
            capability,
            depends_on,
        });
        self
    }

    /// Refuse a schema this build does not understand.
    pub fn validate(&self) -> Result<(), LockError> {
        if self.schema == MANIFEST_SCHEMA {
            Ok(())
        } else {
            Err(LockError::ManifestSchema(self.schema))
        }
    }

    /// Serialize the manifest as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a serializable manifest")
    }

    /// Parse and validate a JSON manifest.
    pub fn from_json(text: &str) -> Result<Self, LockError> {
        let manifest: Self =
            serde_json::from_str(text).map_err(|error| LockError::Json(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// The canonical fingerprint of this manifest.
    ///
    /// Requirement, peer, and edge order are not semantic, so the fingerprint
    /// sorts them first; semantically identical inputs therefore hash equally.
    pub fn fingerprint(&self) -> String {
        digest_json(&self.canonical())
    }

    fn canonical(&self) -> Self {
        let mut manifest = self.clone();
        manifest
            .spec
            .requirements
            .sort_by(|left, right| left.id.cmp(&right.id));
        manifest
            .spec
            .peers
            .sort_by(|left, right| left.node.cmp(&right.node));
        manifest.dependencies.sort_by(|left, right| {
            (&left.capability, &left.depends_on).cmp(&(&right.capability, &right.depends_on))
        });
        manifest
    }
}

/// Why a manifest, graph, or lock was rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LockError {
    /// The manifest schema is not supported.
    #[error("unsupported manifest schema {0}")]
    ManifestSchema(u32),
    /// The lock schema is not supported.
    #[error("unsupported lock schema {0}")]
    LockSchema(u32),
    /// The JSON is malformed.
    #[error("invalid JSON: {0}")]
    Json(String),
    /// A dependency names a capability no requirement declares.
    #[error("dependency edge {capability} -> {depends_on} names an unknown capability")]
    UnknownDependency {
        /// The dependent capability.
        capability: CapabilityId,
        /// The unknown dependency.
        depends_on: CapabilityId,
    },
    /// A node was asked about that the graph does not hold.
    #[error("capability {0} is not a graph node")]
    UnknownNode(CapabilityId),
    /// The dependency graph contains a cycle.
    #[error("dependency cycle among {capabilities:?}")]
    Cycle {
        /// The nodes left after topological sorting, sorted.
        capabilities: Vec<CapabilityId>,
    },
    /// A bound capability depends on one that was not bound.
    #[error("bound capability {capability} depends on unbound {depends_on}")]
    UnsatisfiedDependency {
        /// The bound capability.
        capability: CapabilityId,
        /// The missing dependency.
        depends_on: CapabilityId,
    },
    /// Re-resolution produced different bindings.
    #[error("resolution drifted from the lock: {differences:?}")]
    Drift {
        /// Every observed difference, in a deterministic order.
        differences: Vec<Drift>,
    },
    /// The stored fingerprint does not match the lock content.
    #[error("the lock fingerprint does not match its content")]
    Fingerprint,
    /// The composition itself could not be resolved.
    #[error(transparent)]
    Composition(#[from] CompositionError),
}

/// One deterministic difference between two locks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Drift {
    /// The manifest the lock was made from changed.
    Manifest {
        /// Previous manifest fingerprint.
        was: String,
        /// Current manifest fingerprint.
        now: String,
    },
    /// The local catalogue revision changed.
    Catalogue {
        /// Previous revision.
        was: String,
        /// Current revision.
        now: String,
    },
    /// One peer's identity, grant, or catalogue changed.
    Peer {
        /// The peer.
        node: NodeId,
        /// Previous record.
        was: Option<LockedPeer>,
        /// Current record.
        now: Option<LockedPeer>,
    },
    /// One capability's binding or version changed.
    Binding {
        /// The capability.
        capability: CapabilityId,
        /// Previous record.
        was: Option<LockedBinding>,
        /// Current record.
        now: Option<LockedBinding>,
    },
    /// The deterministic activation order changed.
    Activation {
        /// Previous order.
        was: Vec<CapabilityId>,
        /// Current order.
        now: Vec<CapabilityId>,
    },
    /// The skipped set changed.
    Skipped {
        /// Previous skipped set.
        was: Vec<CapabilityId>,
        /// Current skipped set.
        now: Vec<CapabilityId>,
    },
    /// The budgets changed.
    Budgets {
        /// Previous budgets.
        was: Budgets,
        /// Current budgets.
        now: Budgets,
    },
}

/// One declared peer and the catalogue revision it showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPeer {
    /// The peer's identity.
    pub node: NodeId,
    /// The grant the manifest selected.
    pub grant: Option<GrantRef>,
    /// The catalogue revision this peer published, when known.
    pub catalogue: Option<String>,
}

/// One exact resolved binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedBinding {
    /// The capability.
    pub capability: CapabilityId,
    /// The exact descriptor version bound.
    pub version: Version,
    /// Where it executes.
    pub binding: Binding,
    /// The direct dependencies of this binding, sorted.
    pub dependencies: Vec<CapabilityId>,
}

/// A durable record of one resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositionLock {
    /// Schema version; only LOCK_SCHEMA is accepted.
    pub schema: u32,
    /// The manifest fingerprint this lock was resolved from.
    pub manifest: String,
    /// The composition id.
    pub composition: CompositionId,
    /// The composition version.
    pub version: Version,
    /// The workspace the plan instantiates.
    pub workspace: WorkspaceId,
    /// The local catalogue revision used.
    pub catalogue: String,
    /// Every declared peer and the catalogue revision it showed.
    pub peers: Vec<LockedPeer>,
    /// Every exact binding, sorted by capability.
    pub bindings: Vec<LockedBinding>,
    /// Dependency-first activation order.
    pub activation_order: Vec<CapabilityId>,
    /// Reverse teardown order.
    pub teardown_order: Vec<CapabilityId>,
    /// Optional requirements that were skipped, sorted.
    pub skipped: Vec<CapabilityId>,
    /// The effective budgets.
    pub budgets: Budgets,
    /// The canonical fingerprint of all other fields.
    pub fingerprint: String,
}

impl CompositionLock {
    /// Serialize the lock as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a serializable lock")
    }

    /// Parse a lock, checking its schema and fingerprint.
    pub fn from_json(text: &str) -> Result<Self, LockError> {
        let lock: Self =
            serde_json::from_str(text).map_err(|error| LockError::Json(error.to_string()))?;
        if lock.schema != LOCK_SCHEMA {
            return Err(LockError::LockSchema(lock.schema));
        }
        if lock.recompute_fingerprint() != lock.fingerprint {
            return Err(LockError::Fingerprint);
        }
        Ok(lock)
    }

    /// Recompute the fingerprint of the current content.
    pub fn recompute_fingerprint(&self) -> String {
        let mut copy = self.clone();
        copy.fingerprint.clear();
        digest_json(&copy)
    }

    /// Every difference between this lock and the other, deterministically.
    pub fn diff(&self, other: &Self) -> Vec<Drift> {
        let mut drift = Vec::new();
        if self.manifest != other.manifest {
            drift.push(Drift::Manifest {
                was: self.manifest.clone(),
                now: other.manifest.clone(),
            });
        }
        if self.catalogue != other.catalogue {
            drift.push(Drift::Catalogue {
                was: self.catalogue.clone(),
                now: other.catalogue.clone(),
            });
        }
        let peer_nodes: BTreeSet<&NodeId> = self
            .peers
            .iter()
            .chain(other.peers.iter())
            .map(|peer| &peer.node)
            .collect();
        for node in peer_nodes {
            let was = self.peers.iter().find(|peer| &peer.node == node).cloned();
            let now = other.peers.iter().find(|peer| &peer.node == node).cloned();
            if was != now {
                drift.push(Drift::Peer {
                    node: node.clone(),
                    was,
                    now,
                });
            }
        }
        let capabilities: BTreeSet<&CapabilityId> = self
            .bindings
            .iter()
            .chain(other.bindings.iter())
            .map(|binding| &binding.capability)
            .collect();
        for capability in capabilities {
            let was = self
                .bindings
                .iter()
                .find(|binding| &binding.capability == capability)
                .cloned();
            let now = other
                .bindings
                .iter()
                .find(|binding| &binding.capability == capability)
                .cloned();
            if was != now {
                drift.push(Drift::Binding {
                    capability: capability.clone(),
                    was,
                    now,
                });
            }
        }
        if self.activation_order != other.activation_order {
            drift.push(Drift::Activation {
                was: self.activation_order.clone(),
                now: other.activation_order.clone(),
            });
        }
        if self.skipped != other.skipped {
            drift.push(Drift::Skipped {
                was: self.skipped.clone(),
                now: other.skipped.clone(),
            });
        }
        if self.budgets != other.budgets {
            drift.push(Drift::Budgets {
                was: self.budgets.clone(),
                now: other.budgets.clone(),
            });
        }
        drift
    }

    /// Re-resolve the manifest and require the lock to still describe it.
    pub fn verify(
        &self,
        manifest: &CompositionManifest,
        local: &Catalogue,
        peers: &[PeerCatalogue],
        scope: &LocalScope,
    ) -> Result<ResolvedComposition, LockError> {
        if manifest.fingerprint() != self.manifest {
            return Err(LockError::Drift {
                differences: vec![Drift::Manifest {
                    was: self.manifest.clone(),
                    now: manifest.fingerprint(),
                }],
            });
        }
        let resolved = resolve_host(&manifest.spec, local, peers, scope)?;
        let current = lock_resolution(manifest, &resolved, local, peers)?;
        let differences = self.diff(&current);
        if differences.is_empty() {
            Ok(resolved)
        } else {
            Err(LockError::Drift { differences })
        }
    }
}

/// Resolve a manifest and produce its lock.
pub fn resolve_locked(
    manifest: &CompositionManifest,
    local: &Catalogue,
    peers: &[PeerCatalogue],
    scope: &LocalScope,
) -> Result<(ResolvedComposition, CompositionLock), LockError> {
    manifest.validate()?;
    let resolved = resolve_host(&manifest.spec, local, peers, scope)?;
    let lock = lock_resolution(manifest, &resolved, local, peers)?;
    Ok((resolved, lock))
}

/// Build the lock for an already-resolved plan.
pub fn lock_resolution(
    manifest: &CompositionManifest,
    resolved: &ResolvedComposition,
    local: &Catalogue,
    peers: &[PeerCatalogue],
) -> Result<CompositionLock, LockError> {
    manifest.validate()?;
    let graph = DependencyGraph::build(&manifest.spec.requirements, &manifest.dependencies)?;
    let full_order = graph.activation_order()?;
    let bound: BTreeSet<CapabilityId> = resolved
        .capabilities
        .iter()
        .map(|capability| capability.descriptor.id.clone())
        .collect();
    for edge in &manifest.dependencies {
        if bound.contains(&edge.capability) && !bound.contains(&edge.depends_on) {
            return Err(LockError::UnsatisfiedDependency {
                capability: edge.capability.clone(),
                depends_on: edge.depends_on.clone(),
            });
        }
    }
    let activation_order: Vec<CapabilityId> = full_order
        .into_iter()
        .filter(|capability| bound.contains(capability))
        .collect();
    let teardown_order = activation_order.iter().rev().cloned().collect();

    let mut bindings: Vec<LockedBinding> = resolved
        .capabilities
        .iter()
        .map(|capability| LockedBinding {
            capability: capability.descriptor.id.clone(),
            version: capability.descriptor.version.clone(),
            binding: capability.binding.clone(),
            dependencies: graph.direct_dependencies(&capability.descriptor.id),
        })
        .collect();
    bindings.sort_by(|left, right| left.capability.cmp(&right.capability));

    let mut locked_peers: Vec<LockedPeer> = manifest
        .spec
        .peers
        .iter()
        .map(|peer| LockedPeer {
            node: peer.node.clone(),
            grant: peer.grant.clone(),
            catalogue: peers
                .iter()
                .find(|candidate| candidate.node == peer.node)
                .map(|candidate| catalogue_revision(&candidate.catalogue)),
        })
        .collect();
    locked_peers.sort_by(|left, right| left.node.cmp(&right.node));

    let mut skipped = resolved.skipped.clone();
    skipped.sort();

    let mut lock = CompositionLock {
        schema: LOCK_SCHEMA,
        manifest: manifest.fingerprint(),
        composition: resolved.id.clone(),
        version: resolved.version.clone(),
        workspace: resolved.workspace.clone(),
        catalogue: catalogue_revision(local),
        peers: locked_peers,
        bindings,
        activation_order,
        teardown_order,
        skipped,
        budgets: resolved.budgets.clone(),
        fingerprint: String::new(),
    };
    lock.fingerprint = lock.recompute_fingerprint();
    Ok(lock)
}

/// A deterministic revision for one catalogue.
pub fn catalogue_revision(catalogue: &Catalogue) -> String {
    let descriptors: Vec<&crate::descriptor::CapabilityDescriptor> = catalogue.iter().collect();
    digest_json(&descriptors)
}

/// The dependency graph over a manifest's declared requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyGraph {
    nodes: BTreeSet<CapabilityId>,
    edges: BTreeMap<CapabilityId, BTreeSet<CapabilityId>>,
}

impl DependencyGraph {
    /// Validate and build the graph.
    pub fn build(
        requirements: &[CapabilityRequirement],
        dependencies: &[DependencyEdge],
    ) -> Result<Self, LockError> {
        let nodes: BTreeSet<CapabilityId> = requirements.iter().map(|req| req.id.clone()).collect();
        let mut edges: BTreeMap<CapabilityId, BTreeSet<CapabilityId>> = BTreeMap::new();
        for edge in dependencies {
            if !nodes.contains(&edge.capability) || !nodes.contains(&edge.depends_on) {
                return Err(LockError::UnknownDependency {
                    capability: edge.capability.clone(),
                    depends_on: edge.depends_on.clone(),
                });
            }
            edges
                .entry(edge.capability.clone())
                .or_default()
                .insert(edge.depends_on.clone());
        }
        Ok(Self { nodes, edges })
    }

    /// The direct dependencies of one capability, sorted.
    pub fn direct_dependencies(&self, capability: &CapabilityId) -> Vec<CapabilityId> {
        self.edges
            .get(capability)
            .map(|dependencies| dependencies.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Every dependency reachable from one capability, sorted, excluding itself.
    pub fn transitive_dependencies(
        &self,
        capability: &CapabilityId,
    ) -> Result<Vec<CapabilityId>, LockError> {
        if !self.nodes.contains(capability) {
            return Err(LockError::UnknownNode(capability.clone()));
        }
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([capability.clone()]);
        while let Some(current) = queue.pop_front() {
            if let Some(dependencies) = self.edges.get(&current) {
                for dependency in dependencies {
                    if seen.insert(dependency.clone()) {
                        queue.push_back(dependency.clone());
                    }
                }
            }
        }
        Ok(seen.into_iter().collect())
    }

    /// Dependency-first activation order, deterministic by capability id.
    pub fn activation_order(&self) -> Result<Vec<CapabilityId>, LockError> {
        let mut pending: BTreeMap<CapabilityId, BTreeSet<CapabilityId>> = self
            .nodes
            .iter()
            .map(|node| {
                (
                    node.clone(),
                    self.edges.get(node).cloned().unwrap_or_default(),
                )
            })
            .collect();
        let mut ready: BTreeSet<CapabilityId> = pending
            .iter()
            .filter(|(_, dependencies)| dependencies.is_empty())
            .map(|(node, _)| node.clone())
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(node) = ready.pop_first() {
            for (candidate, dependencies) in pending.iter_mut() {
                if dependencies.remove(&node) && dependencies.is_empty() {
                    ready.insert(candidate.clone());
                }
            }
            pending.remove(&node);
            order.push(node);
        }
        if !pending.is_empty() {
            return Err(LockError::Cycle {
                capabilities: pending.into_keys().collect(),
            });
        }
        Ok(order)
    }

    /// Reverse activation order.
    pub fn teardown_order(&self) -> Result<Vec<CapabilityId>, LockError> {
        let mut order = self.activation_order()?;
        order.reverse();
        Ok(order)
    }
}

/// Render one drift record for diagnostics.
impl fmt::Display for Drift {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest { was, now } => write!(formatter, "manifest {was} -> {now}"),
            Self::Catalogue { was, now } => write!(formatter, "catalogue {was} -> {now}"),
            Self::Peer { node, .. } => write!(formatter, "peer {node} changed"),
            Self::Binding { capability, .. } => write!(formatter, "binding {capability} changed"),
            Self::Activation { .. } => formatter.write_str("activation order changed"),
            Self::Skipped { .. } => formatter.write_str("skipped set changed"),
            Self::Budgets { .. } => formatter.write_str("budgets changed"),
        }
    }
}

fn digest_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("a serializable value");
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
