//! Two-level composition: one spec, one resolver, two scopes.
//!
//! Local composition resolves a spec inside one Workspace against the local
//! catalogue. Federated composition resolves the same spec across autonomous
//! Nodes, and the only thing that changes is where a binding executes. The
//! rules that keep the levels apart are enforced here, not by convention:
//!
//! - a local capability the Workspace view does not admit is refused; it is
//!   never re-routed to a peer,
//! - a requirement that forbids remote binding never reaches a peer,
//! - a requirement that requires a named peer never silently falls back to a
//!   local implementation,
//! - every remote binding names the peer and the grant it runs under,
//! - the result has exactly one workspace id, however many peers it uses.

use crate::catalogue::Catalogue;
use crate::descriptor::CapabilityDescriptor;
use crate::id::{CapabilityId, NodeId, ProviderId, WorkspaceId};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

/// A composition's stable id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CompositionId(String);

impl CompositionId {
    /// Build a composition id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CompositionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A grant a peer relationship carries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GrantRef(String);

impl GrantRef {
    /// Build a grant reference.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The reference as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GrantRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One peer the composition may use, and the grant it runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerRef {
    /// The peer Node.
    pub node: NodeId,
    /// The grant authorizing this composition to use it. A peer without a
    /// grant can be listed but cannot satisfy a requirement.
    pub grant: Option<GrantRef>,
}

impl PeerRef {
    /// A peer relationship with no grant yet.
    pub fn new(node: NodeId) -> Self {
        Self { node, grant: None }
    }

    /// Attach the grant.
    pub fn with_grant(mut self, grant: GrantRef) -> Self {
        self.grant = Some(grant);
        self
    }
}

/// How one requirement may be satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemotePolicy {
    /// Local only. A peer is never consulted.
    Forbidden,
    /// Local first, then the allowlisted peers in order.
    Allowed,
    /// This named peer only. A local implementation is not a fallback.
    Required(NodeId),
}

/// One thing the composition needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequirement {
    /// The capability asked for.
    pub id: CapabilityId,
    /// The accepted version range.
    pub version: VersionReq,
    /// Whether the composition fails when it cannot be satisfied.
    pub required: bool,
    /// Where it may execute.
    pub remote: RemotePolicy,
}

impl CapabilityRequirement {
    /// A required, local-only requirement.
    pub fn new(id: CapabilityId, version: VersionReq) -> Self {
        Self {
            id,
            version,
            required: true,
            remote: RemotePolicy::Forbidden,
        }
    }

    /// Whether a missing capability fails the composition.
    pub fn with_required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Where the capability may execute.
    pub fn with_remote(mut self, remote: RemotePolicy) -> Self {
        self.remote = remote;
        self
    }
}

/// The budgets a workspace applies to the resolved plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    /// Most invocations in flight at once.
    pub max_concurrent_invocations: u16,
    /// Default wall-clock ceiling per invocation.
    pub default_timeout: Option<Duration>,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_concurrent_invocations: 8,
            default_timeout: None,
        }
    }
}

/// A declarative composition: what is needed, and where it may run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositionSpec {
    /// Stable id.
    pub id: CompositionId,
    /// Composition version.
    pub version: Version,
    /// What the composition is for.
    pub purpose: String,
    /// What it needs.
    pub requirements: Vec<CapabilityRequirement>,
    /// The peers it may use, in preference order.
    pub peers: Vec<PeerRef>,
    /// The budgets it applies.
    pub budgets: Budgets,
}

impl CompositionSpec {
    /// A spec with no requirements and no peers.
    pub fn new(id: impl Into<CompositionId>, version: Version, purpose: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version,
            purpose: purpose.into(),
            requirements: Vec::new(),
            peers: Vec::new(),
            budgets: Budgets::default(),
        }
    }

    /// Add a requirement.
    pub fn with_requirement(mut self, requirement: CapabilityRequirement) -> Self {
        self.requirements.push(requirement);
        self
    }

    /// Allow one peer.
    pub fn with_peer(mut self, peer: PeerRef) -> Self {
        self.peers.push(peer);
        self
    }
}

impl From<String> for CompositionId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for CompositionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// One peer's published catalogue.
#[derive(Debug, Clone)]
pub struct PeerCatalogue {
    /// The peer these capabilities belong to.
    pub node: NodeId,
    /// What the peer published.
    pub catalogue: Catalogue,
}

impl PeerCatalogue {
    /// Pair a peer with its catalogue.
    pub fn new(node: NodeId, catalogue: Catalogue) -> Self {
        Self { node, catalogue }
    }
}

/// The authorized subset of the catalogue for one workspace.
#[derive(Debug, Clone)]
pub struct LocalScope {
    /// The workspace this view belongs to.
    pub workspace: WorkspaceId,
    admitted: BTreeSet<CapabilityId>,
}

impl LocalScope {
    /// An empty view: nothing is admitted until it is named.
    pub fn new(workspace: WorkspaceId) -> Self {
        Self {
            workspace,
            admitted: BTreeSet::new(),
        }
    }

    /// A view that admits every capability in one catalogue.
    pub fn admitting(workspace: WorkspaceId, catalogue: &Catalogue) -> Self {
        Self {
            workspace,
            admitted: catalogue.ids().cloned().collect(),
        }
    }

    /// Admit one capability.
    pub fn admit(&mut self, id: CapabilityId) {
        self.admitted.insert(id);
    }

    /// Whether the view admits a capability.
    pub fn admits(&self, id: &CapabilityId) -> bool {
        self.admitted.contains(id)
    }

    /// Every admitted capability.
    pub fn admitted(&self) -> impl Iterator<Item = &CapabilityId> {
        self.admitted.iter()
    }
}

/// Where a resolved capability executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Binding {
    /// In this Node, through the named provider.
    Local {
        /// The provider that implements it here.
        provider: ProviderId,
    },
    /// On a peer, under a grant. This is a proxy, never a local provider.
    Remote {
        /// The peer that executes it.
        node: NodeId,
        /// The grant it runs under.
        grant: GrantRef,
    },
}

impl Binding {
    /// Whether the binding executes on another Node.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }
}

/// One requirement, resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCapability {
    /// The descriptor that satisfied the requirement.
    pub descriptor: CapabilityDescriptor,
    /// Where it will execute.
    pub binding: Binding,
}

/// A validated, resolved composition.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComposition {
    /// The composition this plan belongs to.
    pub id: CompositionId,
    /// The composition version that was resolved.
    pub version: Version,
    /// The one workspace this plan instantiates. However many peers it uses,
    /// a workspace never spans Nodes.
    pub workspace: WorkspaceId,
    /// The resolved capabilities.
    pub capabilities: Vec<ResolvedCapability>,
    /// Optional requirements that could not be satisfied.
    pub skipped: Vec<CapabilityId>,
    /// The effective execution budgets this plan was resolved with.
    pub budgets: Budgets,
}

impl ResolvedComposition {
    /// Whether every binding executes locally.
    pub fn is_local_only(&self) -> bool {
        self.capabilities
            .iter()
            .all(|capability| !capability.binding.is_remote())
    }

    /// Every peer this plan uses.
    pub fn remote_nodes(&self) -> BTreeSet<&NodeId> {
        self.capabilities
            .iter()
            .filter_map(|capability| match &capability.binding {
                Binding::Remote { node, .. } => Some(node),
                Binding::Local { .. } => None,
            })
            .collect()
    }

    /// One resolved descriptor by id.
    pub fn descriptor(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.capabilities
            .iter()
            .find(|capability| &capability.descriptor.id == id)
            .map(|capability| &capability.descriptor)
    }
}

/// Why a composition could not be resolved.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompositionError {
    /// The spec contradicts itself.
    #[error("invalid composition: {message}")]
    Invalid {
        /// What is wrong.
        message: String,
    },
    /// A required capability is offered nowhere.
    #[error("required capability is missing: {capability}")]
    Missing {
        /// What was missing.
        capability: CapabilityId,
    },
    /// A candidate exists but not in the accepted range.
    #[error("version mismatch for {capability}: required {required}, available {available}")]
    VersionMismatch {
        /// The capability.
        capability: CapabilityId,
        /// The accepted range.
        required: String,
        /// What the candidate offers.
        available: String,
    },
    /// The workspace view does not admit the capability.
    #[error("the workspace view does not admit {capability}")]
    NotAdmitted {
        /// The refused capability.
        capability: CapabilityId,
    },
    /// A peer was named that the spec does not allow.
    #[error("peer {node} is not in the composition allowlist")]
    PeerNotAllowed {
        /// The refused peer.
        node: NodeId,
    },
    /// A required peer does not offer the capability.
    #[error("peer {node} does not offer {capability}")]
    PeerCapabilityMissing {
        /// The peer.
        node: NodeId,
        /// The capability.
        capability: CapabilityId,
    },
    /// A peer relationship carries no grant.
    #[error("peer {node} has no grant for {capability}")]
    MissingGrant {
        /// The peer.
        node: NodeId,
        /// The capability.
        capability: CapabilityId,
    },
}

/// Resolve a composition against a local catalogue and, when the spec allows
/// it, the catalogues of the peers it names.
///
/// # Host-authored specs only
///
/// A specification that came from a model must pass
/// `proposal::approve`/`proposal::resolve_approved` instead: the token makes
/// the approval impossible to skip by accident.
pub fn resolve_host(
    spec: &CompositionSpec,
    local: &Catalogue,
    peers: &[PeerCatalogue],
    scope: &LocalScope,
) -> Result<ResolvedComposition, CompositionError> {
    validate(spec)?;

    let mut capabilities = Vec::new();
    let mut skipped = Vec::new();
    for requirement in &spec.requirements {
        match resolve_one(requirement, spec, local, peers, scope)? {
            Some(resolved) => capabilities.push(resolved),
            None => skipped.push(requirement.id.clone()),
        }
    }

    Ok(ResolvedComposition {
        id: spec.id.clone(),
        version: spec.version.clone(),
        workspace: scope.workspace.clone(),
        capabilities,
        skipped,
        budgets: spec.budgets.clone(),
    })
}

fn validate(spec: &CompositionSpec) -> Result<(), CompositionError> {
    if spec.budgets.max_concurrent_invocations == 0 {
        return Err(CompositionError::Invalid {
            message: "max_concurrent_invocations must be at least 1".to_string(),
        });
    }
    if spec
        .budgets
        .default_timeout
        .is_some_and(|timeout| timeout.is_zero())
    {
        return Err(CompositionError::Invalid {
            message: "default_timeout must be greater than zero".to_string(),
        });
    }
    let mut ids = BTreeSet::new();
    for requirement in &spec.requirements {
        if !ids.insert(requirement.id.clone()) {
            return Err(CompositionError::Invalid {
                message: format!("duplicate requirement: {}", requirement.id),
            });
        }
    }
    let mut nodes = BTreeSet::new();
    for peer in &spec.peers {
        if !nodes.insert(&peer.node) {
            return Err(CompositionError::Invalid {
                message: format!("duplicate peer: {}", peer.node),
            });
        }
    }
    Ok(())
}

fn resolve_one(
    requirement: &CapabilityRequirement,
    spec: &CompositionSpec,
    local: &Catalogue,
    peers: &[PeerCatalogue],
    scope: &LocalScope,
) -> Result<Option<ResolvedCapability>, CompositionError> {
    // A local implementation decides the requirement. It is never bypassed by
    // a remote binding: refusing the local one and taking a peer's would let a
    // workspace view widen itself through the federation. The one exception is
    // the other direction: a requirement that names a peer is executed there,
    // so a local implementation is not a substitute for it.
    if !matches!(requirement.remote, RemotePolicy::Required(_)) {
        if let Some(descriptor) = local.get(&requirement.id) {
            if !scope.admits(&requirement.id) {
                return Err(CompositionError::NotAdmitted {
                    capability: requirement.id.clone(),
                });
            }
            check_version(requirement, descriptor)?;
            return Ok(Some(ResolvedCapability {
                descriptor: descriptor.clone(),
                binding: Binding::Local {
                    provider: descriptor.id.provider.clone(),
                },
            }));
        }
    }

    match &requirement.remote {
        RemotePolicy::Forbidden => missing(requirement),
        RemotePolicy::Allowed => {
            // Every allowlisted peer is a candidate. A peer that publishes the
            // capability but cannot satisfy the version, or holds no grant,
            // must not stop the scan: a later peer may be usable. The first
            // usable peer in spec order wins; authorization is never weakened.
            let mut version_error: Option<CompositionError> = None;
            let mut grant_error: Option<CompositionError> = None;
            for peer in &spec.peers {
                let Some(catalogue) = catalogue_of(peers, &peer.node) else {
                    continue;
                };
                let Some(descriptor) = catalogue.get(&requirement.id) else {
                    continue;
                };
                if let Err(error) = check_version(requirement, descriptor) {
                    version_error.get_or_insert(error);
                    continue;
                }
                let Some(grant) = peer.grant.clone() else {
                    // Authorization outranks a version mismatch in the report:
                    // a peer that holds no grant is a stronger refusal.
                    grant_error.get_or_insert_with(|| missing_grant(peer, requirement));
                    continue;
                };
                return Ok(Some(ResolvedCapability {
                    descriptor: descriptor.clone(),
                    binding: Binding::Remote {
                        node: peer.node.clone(),
                        grant,
                    },
                }));
            }
            if !requirement.required {
                return Ok(None);
            }
            Err(grant_error
                .or(version_error)
                .unwrap_or_else(|| CompositionError::Missing {
                    capability: requirement.id.clone(),
                }))
        }
        RemotePolicy::Required(node) => {
            let peer = spec
                .peers
                .iter()
                .find(|peer| &peer.node == node)
                .ok_or_else(|| CompositionError::PeerNotAllowed { node: node.clone() })?;
            let Some(catalogue) = catalogue_of(peers, node) else {
                return peer_missing(requirement, node);
            };
            let Some(descriptor) = catalogue.get(&requirement.id) else {
                return peer_missing(requirement, node);
            };
            check_version(requirement, descriptor)?;
            let grant = peer
                .grant
                .clone()
                .ok_or_else(|| missing_grant(peer, requirement))?;
            Ok(Some(ResolvedCapability {
                descriptor: descriptor.clone(),
                binding: Binding::Remote {
                    node: node.clone(),
                    grant,
                },
            }))
        }
    }
}

fn catalogue_of<'a>(peers: &'a [PeerCatalogue], node: &NodeId) -> Option<&'a Catalogue> {
    peers
        .iter()
        .find(|peer| &peer.node == node)
        .map(|peer| &peer.catalogue)
}

fn check_version(
    requirement: &CapabilityRequirement,
    descriptor: &CapabilityDescriptor,
) -> Result<(), CompositionError> {
    if requirement.version.matches(&descriptor.version) {
        Ok(())
    } else {
        Err(CompositionError::VersionMismatch {
            capability: requirement.id.clone(),
            required: requirement.version.to_string(),
            available: descriptor.version.to_string(),
        })
    }
}

fn missing(
    requirement: &CapabilityRequirement,
) -> Result<Option<ResolvedCapability>, CompositionError> {
    if requirement.required {
        Err(CompositionError::Missing {
            capability: requirement.id.clone(),
        })
    } else {
        Ok(None)
    }
}

fn peer_missing(
    requirement: &CapabilityRequirement,
    node: &NodeId,
) -> Result<Option<ResolvedCapability>, CompositionError> {
    if requirement.required {
        Err(CompositionError::PeerCapabilityMissing {
            node: node.clone(),
            capability: requirement.id.clone(),
        })
    } else {
        Ok(None)
    }
}

fn missing_grant(peer: &PeerRef, requirement: &CapabilityRequirement) -> CompositionError {
    CompositionError::MissingGrant {
        node: peer.node.clone(),
        capability: requirement.id.clone(),
    }
}
