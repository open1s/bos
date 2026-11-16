//! Deterministic validation of model-authored compositions.
//!
//! A host that lets a model propose a composition must not let the proposal
//! become authority. This module is the gate: a proposal is checked against the
//! host's approved capability set, a risk ceiling, and the remote policy BEFORE
//! the ordinary resolver runs. The checks are data-in, verdict-out: no model,
//! no clock, and no I/O participate.

use std::collections::BTreeSet;

use crate::catalogue::Catalogue;
use crate::composition::{
    resolve_host, CapabilityRequirement, CompositionError, CompositionSpec, LocalScope,
    PeerCatalogue, RemotePolicy,
};
use crate::descriptor::Risk;
use crate::id::CapabilityId;

/// What the host will let a proposal contain.
#[derive(Debug)]
pub struct ProposalPolicy<'a> {
    /// Capability ids the host approved for this turn.
    pub allowed: &'a BTreeSet<CapabilityId>,
    /// The most risk any proposed capability may carry.
    pub max_risk: Risk,
    /// Whether a proposal may name peers or allow remote execution.
    pub allow_remote: bool,
}

/// Why a proposal was refused before resolution.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProposalError {
    /// A capability the host never approved.
    #[error("the proposal asks for {capability}, which the host did not approve")]
    NotApproved {
        /// The refused capability.
        capability: CapabilityId,
    },
    /// A capability above the host's risk ceiling.
    #[error("the proposal asks for {capability} at {risk:?} risk, above the {ceiling:?} ceiling")]
    Risk {
        /// The capability.
        capability: CapabilityId,
        /// What the catalogue declares.
        risk: Risk,
        /// The host's ceiling.
        ceiling: Risk,
    },
    /// A remote binding the host forbids.
    #[error("the proposal allows remote execution of {capability}, which the host forbids")]
    Remote {
        /// The capability.
        capability: CapabilityId,
    },
    /// A capability no provider offers.
    #[error("the proposal asks for {capability}, which no provider offers")]
    Unknown {
        /// The capability.
        capability: CapabilityId,
    },
    /// A remote-only capability whose allowlisted providers hold no grant.
    #[error("the proposal asks for {capability}, which no allowlisted peer is granted to provide")]
    Ungranted {
        /// The capability.
        capability: CapabilityId,
    },
}

/// A proposal that failed either the host gate or the resolver.
#[derive(Debug, thiserror::Error)]
pub enum ProposalRejection {
    /// The host refused the proposal.
    #[error(transparent)]
    Refused(#[from] ProposalError),
    /// The host accepted it but the resolver could not build a plan.
    #[error(transparent)]
    Unresolvable(#[from] CompositionError),
}

/// Check a proposal against the host's policy, without resolving it.
pub fn validate_proposal(
    spec: &CompositionSpec,
    catalogue: &Catalogue,
    peers: &[PeerCatalogue],
    policy: &ProposalPolicy<'_>,
) -> Result<(), ProposalError> {
    if !policy.allow_remote && !spec.peers.is_empty() {
        return Err(ProposalError::Remote {
            capability: spec.requirements.first().map_or_else(
                || CapabilityId::new("proposal", "peers").expect("a valid id"),
                |requirement| requirement.id.clone(),
            ),
        });
    }
    for requirement in &spec.requirements {
        if !policy.allowed.contains(&requirement.id) {
            return Err(ProposalError::NotApproved {
                capability: requirement.id.clone(),
            });
        }
        if !policy.allow_remote && requirement.remote != RemotePolicy::Forbidden {
            return Err(ProposalError::Remote {
                capability: requirement.id.clone(),
            });
        }
        let descriptor = match proposed_descriptor(requirement, spec, catalogue, peers) {
            Ok(Some(descriptor)) => descriptor,
            Ok(None) if !requirement.required => continue,
            Ok(None) => {
                return Err(ProposalError::Unknown {
                    capability: requirement.id.clone(),
                })
            }
            Err(ProposalError::Ungranted { .. }) if !requirement.required => continue,
            Err(error) => return Err(error),
        };
        if descriptor.risk > policy.max_risk {
            return Err(ProposalError::Risk {
                capability: requirement.id.clone(),
                risk: descriptor.risk,
                ceiling: policy.max_risk,
            });
        }
    }
    Ok(())
}

/// The descriptor the resolver would use for this requirement, without
/// resolving: local first (unless the requirement names a peer), then the
/// allowlisted peers in spec order, skipping a version mismatch and a missing
/// grant. A peer outside the spec's peer list is never a candidate, so the
/// proposal cannot widen the allowlist.
fn proposed_descriptor<'a>(
    requirement: &CapabilityRequirement,
    spec: &CompositionSpec,
    catalogue: &'a Catalogue,
    peers: &'a [PeerCatalogue],
) -> Result<Option<&'a crate::descriptor::CapabilityDescriptor>, ProposalError> {
    if !matches!(requirement.remote, RemotePolicy::Required(_)) {
        if let Some(descriptor) = catalogue.get(&requirement.id) {
            return Ok(Some(descriptor));
        }
    }
    let mut saw_version_match = false;
    for peer in &spec.peers {
        let Some(candidate) = peers
            .iter()
            .find(|candidate| candidate.node == peer.node)
            .and_then(|candidate| candidate.catalogue.get(&requirement.id))
        else {
            continue;
        };
        if !requirement.version.matches(&candidate.version) {
            continue;
        }
        saw_version_match = true;
        if peer.grant.is_none() {
            continue;
        }
        return Ok(Some(candidate));
    }
    if saw_version_match {
        return Err(ProposalError::Ungranted {
            capability: requirement.id.clone(),
        });
    }
    Ok(None)
}

/// A proposal the host has approved.
///
/// The only constructor is [`approve`], so a resolver that requires a token
/// cannot be called on an unapproved specification by construction.
///
/// ```compile_fail
/// use capability::proposal::ApprovedComposition;
/// // No public constructor: only `approve` yields a token.
/// let _token = ApprovedComposition { spec: todo!() };
/// ```
#[derive(Debug)]
pub struct ApprovedComposition<'a> {
    spec: &'a CompositionSpec,
}

impl<'a> ApprovedComposition<'a> {
    /// The specification this token approves.
    pub fn spec(&self) -> &'a CompositionSpec {
        self.spec
    }
}

/// Check a proposal and, when it passes, return the token resolution requires.
pub fn approve<'a>(
    spec: &'a CompositionSpec,
    catalogue: &Catalogue,
    peers: &[PeerCatalogue],
    policy: &ProposalPolicy<'_>,
) -> Result<ApprovedComposition<'a>, ProposalError> {
    validate_proposal(spec, catalogue, peers, policy)?;
    Ok(ApprovedComposition { spec })
}

/// Resolve an approved proposal.
///
/// This is the only resolver a model-authored spec may take: it cannot be
/// called without a token, and a token cannot be built without [`approve`].
pub fn resolve_approved(
    approved: ApprovedComposition<'_>,
    local: &Catalogue,
    peers: &[PeerCatalogue],
    scope: &LocalScope,
) -> Result<crate::composition::ResolvedComposition, CompositionError> {
    resolve_host(approved.spec, local, peers, scope)
}
/// Validate, then resolve: the path a host takes for a model-authored spec.
pub fn resolve_proposal(
    spec: &CompositionSpec,
    local: &Catalogue,
    peers: &[PeerCatalogue],
    scope: &LocalScope,
    policy: &ProposalPolicy<'_>,
) -> Result<crate::composition::ResolvedComposition, ProposalRejection> {
    let approved = approve(spec, local, peers, policy)?;
    Ok(resolve_approved(approved, local, peers, scope)?)
}
