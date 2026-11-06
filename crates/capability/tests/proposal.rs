//! The proposal gate: a model-authored spec cannot widen what the host approved.

use std::collections::BTreeSet;

use capability::composition::{
    resolve_host, CapabilityRequirement, CompositionSpec, LocalScope, PeerRef, RemotePolicy,
};
use capability::id::{CapabilityId, WorkspaceId};
use capability::proposal::{resolve_proposal, validate_proposal, ProposalError, ProposalRejection};
use capability::Catalogue;
use capability::{CapabilityDescriptor, CapabilityKind, Risk, VersionSource};
use semver::{Version, VersionReq};

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

fn descriptor(name: &str, risk: Risk) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        capability(name),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
    .with_risk(risk)
}

fn catalogue() -> Catalogue {
    let mut catalogue = Catalogue::new();
    for (name, risk) in [
        ("read_file", Risk::Low),
        ("write_file", Risk::High),
        ("bash", Risk::High),
    ] {
        catalogue.insert(descriptor(name, risk)).expect("unique");
    }
    catalogue
}

fn spec(requirements: Vec<CapabilityRequirement>) -> CompositionSpec {
    let mut spec = CompositionSpec::new(
        "compose:proposal",
        Version::new(1, 0, 0),
        "a model proposal",
    );
    for requirement in requirements {
        spec = spec.with_requirement(requirement);
    }
    spec
}

fn policy(allowed: &BTreeSet<CapabilityId>) -> capability::proposal::ProposalPolicy<'_> {
    capability::proposal::ProposalPolicy {
        allowed,
        max_risk: Risk::Medium,
        allow_remote: false,
    }
}

fn scope(catalogue: &Catalogue) -> LocalScope {
    LocalScope::admitting(
        WorkspaceId::new("ws:proposal").expect("a workspace"),
        catalogue,
    )
}

#[test]
fn an_approved_local_requirement_resolves() {
    let catalogue = catalogue();
    let allowed: BTreeSet<_> = [capability("read_file")].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        capability("read_file"),
        VersionReq::STAR,
    )]);

    let plan = resolve_proposal(
        &spec,
        &catalogue,
        &[],
        &scope(&catalogue),
        &policy(&allowed),
    )
    .expect("approved");

    assert_eq!(plan.capabilities.len(), 1);
    assert_eq!(plan.capabilities[0].descriptor.id, capability("read_file"));
}

#[test]
fn a_capability_the_host_never_approved_is_refused() {
    let catalogue = catalogue();
    let allowed: BTreeSet<_> = [capability("read_file")].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        capability("bash"),
        VersionReq::STAR,
    )]);

    let refused = validate_proposal(&spec, &catalogue, &policy(&allowed)).unwrap_err();

    assert_eq!(
        refused,
        ProposalError::NotApproved {
            capability: capability("bash"),
        }
    );
}

#[test]
fn a_risk_above_the_ceiling_is_refused() {
    let catalogue = catalogue();
    let allowed: BTreeSet<_> = [capability("write_file")].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        capability("write_file"),
        VersionReq::STAR,
    )]);

    let refused = validate_proposal(&spec, &catalogue, &policy(&allowed)).unwrap_err();

    assert_eq!(
        refused,
        ProposalError::Risk {
            capability: capability("write_file"),
            risk: Risk::High,
            ceiling: Risk::Medium,
        }
    );
}

#[test]
fn a_remote_binding_is_refused_when_the_host_forbids_it() {
    let catalogue = catalogue();
    let allowed: BTreeSet<_> = [capability("read_file")].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        capability("read_file"),
        VersionReq::STAR,
    )
    .with_remote(RemotePolicy::Allowed)]);

    assert_eq!(
        validate_proposal(&spec, &catalogue, &policy(&allowed)).unwrap_err(),
        ProposalError::Remote {
            capability: capability("read_file"),
        }
    );

    let mut with_peer = spec;
    with_peer.peers.push(PeerRef::new(
        capability::NodeId::new("peer-a").expect("a node"),
    ));
    assert!(matches!(
        validate_proposal(&with_peer, &catalogue, &policy(&allowed)).unwrap_err(),
        ProposalError::Remote { .. }
    ));
}

#[test]
fn an_approved_name_no_provider_offers_is_refused() {
    let catalogue = catalogue();
    let approved = capability("ghost");
    let allowed: BTreeSet<_> = [approved.clone()].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        approved.clone(),
        VersionReq::STAR,
    )]);

    assert_eq!(
        validate_proposal(&spec, &catalogue, &policy(&allowed)).unwrap_err(),
        ProposalError::Unknown {
            capability: approved,
        }
    );
}

#[test]
fn the_gate_refuses_a_spec_the_resolver_alone_would_accept() {
    let catalogue = catalogue();
    let scope = scope(&catalogue);
    let spec = spec(vec![CapabilityRequirement::new(
        capability("bash"),
        VersionReq::STAR,
    )]);

    // The host scope admits bash, so the plain resolver accepts the plan.
    assert!(resolve_host(&spec, &catalogue, &[], &scope).is_ok());

    // The host's proposal policy does not, and the proposal path says so.
    let allowed: BTreeSet<_> = [capability("read_file")].into_iter().collect();
    let refused = resolve_proposal(&spec, &catalogue, &[], &scope, &policy(&allowed)).unwrap_err();
    assert!(matches!(
        &refused,
        ProposalRejection::Refused(ProposalError::NotApproved { .. })
    ));
}

#[test]
fn only_an_approved_token_resolves() {
    let catalogue = catalogue();
    let allowed: BTreeSet<_> = [capability("read_file")].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        capability("read_file"),
        VersionReq::STAR,
    )]);
    let token =
        capability::proposal::approve(&spec, &catalogue, &policy(&allowed)).expect("approved");

    let plan = capability::proposal::resolve_approved(token, &catalogue, &[], &scope(&catalogue))
        .expect("a plan");

    assert_eq!(plan.capabilities.len(), 1);
}

#[test]
fn a_refused_proposal_yields_no_token() {
    let catalogue = catalogue();
    let allowed: BTreeSet<_> = [capability("read_file")].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        capability("bash"),
        VersionReq::STAR,
    )]);

    let refused = capability::proposal::approve(&spec, &catalogue, &policy(&allowed));

    assert!(
        refused.is_err(),
        "no token is produced for a refused proposal"
    );
}
