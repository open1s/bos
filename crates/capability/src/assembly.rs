//! The Assembly service: task intent to an approved, reproducible Workspace plan.
//!
//! Model output is only a recommendation. This service discovers candidates,
//! ranks them deterministically, records why each was selected or rejected, and
//! requires host policy again at approval time. It never grants a permission:
//! activation is a separate operation through WorkspaceLifecycle.

use std::collections::BTreeSet;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalogue::Catalogue;
use crate::composition::{
    resolve_host, Binding, Budgets, CapabilityRequirement, CompositionError, CompositionSpec,
    LocalScope, PeerCatalogue, PeerRef, RemotePolicy, ResolvedComposition,
};
use crate::descriptor::Risk;
use crate::id::{CapabilityId, NodeId, WorkspaceId};
use crate::lock::{
    catalogue_revision, lock_resolution, CompositionLock, CompositionManifest, DependencyEdge,
    LockError,
};
use crate::proposal::{validate_proposal, ProposalError, ProposalPolicy};
use crate::provider::CapabilityProvider;
use crate::workspace::{WorkspaceError, WorkspaceLifecycle};
use std::sync::Arc;

/// One task the host asks the Assembly service to fulfil.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssemblyRequest {
    /// The user task, kept for the rationale and audit trail.
    pub task: String,
    /// The workspace the plan will instantiate.
    pub workspace: WorkspaceId,
    /// The capabilities the model asks for.
    pub requirements: Vec<CapabilityRequirement>,
    /// Explicit dependencies between those requirements.
    pub dependencies: Vec<DependencyEdge>,
    /// Peers the model proposed; each must also be trusted by the host.
    pub peers: Vec<PeerRef>,
    /// Execution budgets.
    pub budgets: Budgets,
}

impl AssemblyRequest {
    /// A request with no requirements, peers, or dependencies.
    pub fn new(task: impl Into<String>, workspace: WorkspaceId) -> Self {
        Self {
            task: task.into(),
            workspace,
            requirements: Vec::new(),
            dependencies: Vec::new(),
            peers: Vec::new(),
            budgets: Budgets::default(),
        }
    }

    /// Add one requirement.
    pub fn with_requirement(mut self, requirement: CapabilityRequirement) -> Self {
        self.requirements.push(requirement);
        self
    }

    /// Add one peer proposal.
    pub fn with_peer(mut self, peer: PeerRef) -> Self {
        self.peers.push(peer);
        self
    }
}

/// One candidate the discovery step considered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The requirement this candidate could satisfy.
    pub requirement: CapabilityId,
    /// The exact version offered.
    pub version: Version,
    /// Where it would execute, when the candidate is viable.
    pub binding: Option<Binding>,
    /// The declared risk.
    pub risk: Risk,
    /// Whether the resolver selected it.
    pub selected: bool,
    /// Why it was selected or excluded.
    pub rationale: String,
}

/// One candidate that could not be selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedCandidate {
    /// The requirement.
    pub requirement: CapabilityId,
    /// The peer offering it, if any.
    pub node: Option<NodeId>,
    /// The version offered, when known.
    pub version: Option<Version>,
    /// Why it was rejected.
    pub reason: String,
}

/// An explainable, not-yet-approved proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssemblyProposal {
    /// The task.
    pub task: String,
    /// The workspace the plan will instantiate.
    pub workspace: WorkspaceId,
    /// The manifest that would be resolved.
    pub manifest: CompositionManifest,
    /// Every considered candidate.
    pub candidates: Vec<Candidate>,
    /// Every rejected alternative.
    pub rejected: Vec<RejectedCandidate>,
    /// Explanations for the selected plan.
    pub rationale: Vec<String>,
    /// The policy fingerprint used during assembly.
    pub policy: String,
    /// The catalogue revision used during assembly.
    pub catalogue: String,
    /// The peer revisions used during assembly.
    pub peers: Vec<(NodeId, String)>,
    /// Canonical fingerprint of the proposal.
    pub fingerprint: String,
}

impl AssemblyProposal {
    fn recompute_fingerprint(&self) -> String {
        let mut copy = self.clone();
        copy.fingerprint.clear();
        digest_json(&copy)
    }

    /// The selected candidates, in requirement order.
    pub fn selected(&self) -> Vec<&Candidate> {
        self.candidates
            .iter()
            .filter(|candidate| candidate.selected)
            .collect()
    }
}

/// An approved, locked, executable plan. Activation is a separate call.
#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyPlan {
    /// The approved manifest.
    pub manifest: CompositionManifest,
    /// The resolved composition.
    pub resolved: ResolvedComposition,
    /// The reproducible lock.
    pub lock: CompositionLock,
    /// The rationale preserved from the proposal.
    pub rationale: Vec<String>,
}

impl AssemblyPlan {
    /// Hand the plan to the lifecycle coordinator and activate it.
    ///
    /// A failure returns no lifecycle: the coordinator has already rolled back
    /// the providers it activated, so no partially active Workspace escapes.
    pub async fn activate(
        &self,
        providers: &[Arc<dyn CapabilityProvider>],
    ) -> Result<WorkspaceLifecycle, AssemblyError> {
        let mut lifecycle = WorkspaceLifecycle::with_order(
            self.resolved.clone(),
            providers,
            &self.lock.activation_order,
        )
        .map_err(|error| AssemblyError::Activation {
            message: error.to_string(),
        })?;
        lifecycle
            .activate()
            .await
            .map_err(|error| AssemblyError::Activation {
                message: error.to_string(),
            })?;
        Ok(lifecycle)
    }
}

/// Why assembly or approval failed.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum AssemblyError {
    /// The model proposed no requirements.
    #[error("the assembly request has no requirements")]
    Empty,
    /// A peer the model proposed is not trusted by the host.
    #[error("peer {node} is not trusted by the host")]
    UntrustedPeer {
        /// The refused peer.
        node: NodeId,
    },
    /// The request repeats a requirement.
    #[error("requirement {capability} is duplicated")]
    Duplicate {
        /// The repeated capability.
        capability: CapabilityId,
    },
    /// Host policy refused the proposal.
    #[error(transparent)]
    Policy(#[from] ProposalError),
    /// The plan could not be resolved.
    #[error(transparent)]
    Unresolvable(#[from] CompositionError),
    /// The lock could not be built.
    #[error(transparent)]
    Lock(#[from] LockError),
    /// The policy changed between assembly and approval.
    #[error("host policy changed between assembly and approval")]
    PolicyChanged,
    /// The catalogue or a peer catalogue changed between assembly and approval.
    #[error("catalogue state changed between assembly and approval")]
    CatalogueChanged,
    /// Re-resolution selected different bindings.
    #[error("the approved bindings no longer resolve to the same plan")]
    Drift,
    /// Activation failed and was rolled back.
    #[error("workspace activation failed: {message}")]
    Activation {
        /// The failure.
        message: String,
    },
    /// The lifecycle rejected the plan.
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
}

/// The host-facing Assembly service.
pub struct AssemblyEngine<'a> {
    local: &'a Catalogue,
    peers: &'a [PeerCatalogue],
    trusted: BTreeSet<NodeId>,
}

impl<'a> AssemblyEngine<'a> {
    /// Build an engine over the local catalogue and the explicitly trusted peers.
    pub fn new(
        local: &'a Catalogue,
        peers: &'a [PeerCatalogue],
        trusted: impl IntoIterator<Item = NodeId>,
    ) -> Self {
        Self {
            local,
            peers,
            trusted: trusted.into_iter().collect(),
        }
    }

    /// Discover, rank, and validate a proposal. Nothing is approved yet.
    pub fn assemble(
        &self,
        request: &AssemblyRequest,
        policy: &ProposalPolicy<'_>,
    ) -> Result<AssemblyProposal, AssemblyError> {
        if request.requirements.is_empty() {
            return Err(AssemblyError::Empty);
        }
        let mut seen = BTreeSet::new();
        for requirement in &request.requirements {
            if !seen.insert(requirement.id.clone()) {
                return Err(AssemblyError::Duplicate {
                    capability: requirement.id.clone(),
                });
            }
            if !matches!(requirement.remote, RemotePolicy::Forbidden) {
                for peer in &request.peers {
                    if !self.trusted.contains(&peer.node) {
                        return Err(AssemblyError::UntrustedPeer {
                            node: peer.node.clone(),
                        });
                    }
                }
            }
        }
        // A peer the model names must be trusted even if no requirement is remote.
        for peer in &request.peers {
            if !self.trusted.contains(&peer.node) {
                return Err(AssemblyError::UntrustedPeer {
                    node: peer.node.clone(),
                });
            }
        }

        let mut spec = CompositionSpec::new(
            format!("assembly:{}", short_hash(&request.task)),
            Version::new(1, 0, 0),
            request.task.clone(),
        );
        spec.requirements = request.requirements.clone();
        spec.peers = request.peers.clone();
        spec.budgets = request.budgets.clone();
        validate_proposal(&spec, self.local, self.peers, policy)?;

        let scope = LocalScope::admitting(request.workspace.clone(), self.local);
        let resolved = resolve_host(&spec, self.local, self.peers, &scope)?;
        let manifest = CompositionManifest {
            schema: crate::lock::MANIFEST_SCHEMA,
            spec,
            dependencies: request.dependencies.clone(),
        };

        let mut candidates = Vec::new();
        let mut rejected = Vec::new();
        let mut rationale = Vec::new();
        for requirement in &manifest.spec.requirements {
            let Some(selected) = resolved
                .capabilities
                .iter()
                .find(|capability| capability.descriptor.id == requirement.id)
            else {
                rejected.push(RejectedCandidate {
                    requirement: requirement.id.clone(),
                    node: None,
                    version: None,
                    reason: "no provider satisfied the requirement".to_string(),
                });
                rationale.push(format!(
                    "{}: no provider was available; the optional requirement was skipped",
                    requirement.id
                ));
                continue;
            };
            let mut local_seen = false;
            if let Some(descriptor) = self.local.get(&requirement.id) {
                local_seen = true;
                let viable = requirement.version.matches(&descriptor.version)
                    && !matches!(requirement.remote, RemotePolicy::Required(_));
                let local_binding = if viable && matches!(selected.binding, Binding::Local { .. }) {
                    Some(selected.binding.clone())
                } else {
                    None
                };
                candidates.push(Candidate {
                    requirement: requirement.id.clone(),
                    version: descriptor.version.clone(),
                    binding: local_binding,
                    risk: descriptor.risk,
                    selected: viable && matches!(selected.binding, Binding::Local { .. }),
                    rationale: if viable {
                        "local provider, preferred by the resolver".to_string()
                    } else {
                        "local provider could not satisfy the version or remote policy".to_string()
                    },
                });
                if !viable {
                    rejected.push(RejectedCandidate {
                        requirement: requirement.id.clone(),
                        node: None,
                        version: Some(descriptor.version.clone()),
                        reason: "local version or remote policy mismatch".to_string(),
                    });
                }
            }
            for peer in &manifest.spec.peers {
                let Some(catalogue) = self
                    .peers
                    .iter()
                    .find(|candidate| candidate.node == peer.node)
                else {
                    rejected.push(RejectedCandidate {
                        requirement: requirement.id.clone(),
                        node: Some(peer.node.clone()),
                        version: None,
                        reason: "peer catalogue is unavailable".to_string(),
                    });
                    continue;
                };
                let Some(descriptor) = catalogue.catalogue.get(&requirement.id) else {
                    continue;
                };
                let version_ok = requirement.version.matches(&descriptor.version);
                let grant_ok = peer.grant.is_some();
                let selected_here = matches!(
                    &selected.binding,
                    Binding::Remote { node, .. } if node == &peer.node
                );
                let remote_binding = if version_ok {
                    peer.grant.clone().map(|grant| Binding::Remote {
                        node: peer.node.clone(),
                        grant,
                    })
                } else {
                    None
                };
                candidates.push(Candidate {
                    requirement: requirement.id.clone(),
                    version: descriptor.version.clone(),
                    binding: remote_binding,
                    risk: descriptor.risk,
                    selected: selected_here && version_ok && grant_ok,
                    rationale: if selected_here && version_ok && grant_ok {
                        "peer selected after local and earlier peers were unusable".to_string()
                    } else if !version_ok {
                        "peer version does not satisfy the requirement".to_string()
                    } else if !grant_ok {
                        "peer has no grant".to_string()
                    } else {
                        "peer was a lower-priority alternative".to_string()
                    },
                });
                if !version_ok {
                    rejected.push(RejectedCandidate {
                        requirement: requirement.id.clone(),
                        node: Some(peer.node.clone()),
                        version: Some(descriptor.version.clone()),
                        reason: "version mismatch".to_string(),
                    });
                } else if !grant_ok {
                    rejected.push(RejectedCandidate {
                        requirement: requirement.id.clone(),
                        node: Some(peer.node.clone()),
                        version: Some(descriptor.version.clone()),
                        reason: "missing grant".to_string(),
                    });
                }
            }
            if !local_seen && candidates.iter().all(|c| c.requirement != requirement.id) {
                rejected.push(RejectedCandidate {
                    requirement: requirement.id.clone(),
                    node: None,
                    version: None,
                    reason: "no candidate was discovered".to_string(),
                });
            }
            rationale.push(format!(
                "{}@{} resolved to {:?}",
                requirement.id, selected.descriptor.version, selected.binding
            ));
        }
        // Deterministic order independent of discovery order.
        candidates.sort_by(|left, right| {
            (
                &left.requirement,
                &left.version,
                format!("{:?}", left.binding),
            )
                .cmp(&(
                    &right.requirement,
                    &right.version,
                    format!("{:?}", right.binding),
                ))
        });

        let mut proposal = AssemblyProposal {
            task: request.task.clone(),
            workspace: request.workspace.clone(),
            manifest,
            candidates,
            rejected,
            rationale,
            policy: policy_fingerprint(policy),
            catalogue: catalogue_revision(self.local),
            peers: manifest_peer_revisions(&request.peers, self.peers),
            fingerprint: String::new(),
        };
        proposal.fingerprint = proposal.recompute_fingerprint();
        Ok(proposal)
    }

    /// Re-validate host policy, detect drift, and produce the locked plan.
    ///
    /// The model has no path here that skips validation.
    pub fn approve(
        &self,
        proposal: &AssemblyProposal,
        policy: &ProposalPolicy<'_>,
    ) -> Result<AssemblyPlan, AssemblyError> {
        if proposal.policy != policy_fingerprint(policy) {
            return Err(AssemblyError::PolicyChanged);
        }
        if proposal.catalogue != catalogue_revision(self.local)
            || proposal.peers != manifest_peer_revisions(&proposal.manifest.spec.peers, self.peers)
        {
            return Err(AssemblyError::CatalogueChanged);
        }
        validate_proposal(&proposal.manifest.spec, self.local, self.peers, policy)?;
        let scope = LocalScope::admitting(proposal.workspace.clone(), self.local);
        let resolved = resolve_host(&proposal.manifest.spec, self.local, self.peers, &scope)?;
        // The selected bindings must not have moved since assembly.
        for selected in proposal.selected() {
            let Some(capability) = resolved
                .capabilities
                .iter()
                .find(|capability| capability.descriptor.id == selected.requirement)
            else {
                return Err(AssemblyError::Drift);
            };
            let Some(binding) = &selected.binding else {
                continue;
            };
            if &capability.binding != binding || capability.descriptor.version != selected.version {
                return Err(AssemblyError::Drift);
            }
        }
        let lock = lock_resolution(&proposal.manifest, &resolved, self.local, self.peers)?;
        Ok(AssemblyPlan {
            manifest: proposal.manifest.clone(),
            resolved,
            lock,
            rationale: proposal.rationale.clone(),
        })
    }
}

fn manifest_peer_revisions(
    peers: &[PeerRef],
    catalogues: &[PeerCatalogue],
) -> Vec<(NodeId, String)> {
    let mut revisions: Vec<(NodeId, String)> = peers
        .iter()
        .map(|peer| {
            (
                peer.node.clone(),
                catalogues
                    .iter()
                    .find(|candidate| candidate.node == peer.node)
                    .map(|candidate| catalogue_revision(&candidate.catalogue))
                    .unwrap_or_default(),
            )
        })
        .collect();
    revisions.sort_by(|left, right| left.0.cmp(&right.0));
    revisions
}

/// A canonical fingerprint of the host policy snapshot.
pub fn policy_fingerprint(policy: &ProposalPolicy<'_>) -> String {
    #[derive(Serialize)]
    struct Snapshot<'a> {
        allowed: Vec<&'a CapabilityId>,
        max_risk: Risk,
        allow_remote: bool,
    }
    let mut allowed: Vec<&CapabilityId> = policy.allowed.iter().collect();
    allowed.sort();
    digest_json(&Snapshot {
        allowed,
        max_risk: policy.max_risk,
        allow_remote: policy.allow_remote,
    })
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("a serializable value");
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
