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

fn remote_policy(allowed: &BTreeSet<CapabilityId>) -> capability::proposal::ProposalPolicy<'_> {
    capability::proposal::ProposalPolicy {
        allowed,
        max_risk: Risk::Medium,
        allow_remote: true,
    }
}

fn remote_peer(
    node_name: &str,
    name: &str,
    risk: Risk,
) -> (
    capability::composition::PeerRef,
    capability::composition::PeerCatalogue,
) {
    let mut catalogue = Catalogue::new();
    catalogue
        .insert(descriptor(name, risk))
        .expect("a unique descriptor");
    let peer =
        capability::composition::PeerRef::new(capability::NodeId::new(node_name).expect("a node"))
            .with_grant(capability::composition::GrantRef::new(format!(
                "grant:{node_name}"
            )));
    (
        peer,
        capability::composition::PeerCatalogue::new(
            capability::NodeId::new(node_name).expect("a node"),
            catalogue,
        ),
    )
}

#[test]
fn an_approved_remote_only_capability_passes_and_resolves() {
    let local = catalogue();
    let (peer, remote) = remote_peer("peer-a", "remote_search", Risk::Low);
    let node = capability::NodeId::new("peer-a").expect("a node");
    let allowed: BTreeSet<_> = [capability("remote_search")].into_iter().collect();
    let mut spec = spec(vec![CapabilityRequirement::new(
        capability("remote_search"),
        VersionReq::STAR,
    )
    .with_remote(RemotePolicy::Allowed)]);
    spec.peers.push(peer);

    let plan = resolve_proposal(
        &spec,
        &local,
        &[remote],
        &scope(&local),
        &remote_policy(&allowed),
    )
    .expect("the remote-only capability is approved and resolved");

    assert_eq!(
        plan.capabilities[0].binding,
        capability::composition::Binding::Remote {
            node,
            grant: capability::composition::GrantRef::new("grant:peer-a"),
        }
    );
}

#[test]
fn a_remote_only_capability_the_host_did_not_approve_is_refused() {
    let local = catalogue();
    let (peer, remote) = remote_peer("peer-a", "remote_search", Risk::Low);
    let allowed: BTreeSet<_> = [capability("read_file")].into_iter().collect();
    let mut spec = spec(vec![CapabilityRequirement::new(
        capability("remote_search"),
        VersionReq::STAR,
    )
    .with_remote(RemotePolicy::Allowed)]);
    spec.peers.push(peer);

    assert_eq!(
        validate_proposal(&spec, &local, &[remote], &remote_policy(&allowed)).unwrap_err(),
        ProposalError::NotApproved {
            capability: capability("remote_search"),
        }
    );
}

#[test]
fn a_remote_capability_above_the_risk_ceiling_is_refused() {
    let local = catalogue();
    let (peer, remote) = remote_peer("peer-a", "remote_search", Risk::High);
    let allowed: BTreeSet<_> = [capability("remote_search")].into_iter().collect();
    let mut spec = spec(vec![CapabilityRequirement::new(
        capability("remote_search"),
        VersionReq::STAR,
    )
    .with_remote(RemotePolicy::Allowed)]);
    spec.peers.push(peer);

    assert_eq!(
        validate_proposal(&spec, &local, &[remote], &remote_policy(&allowed)).unwrap_err(),
        ProposalError::Risk {
            capability: capability("remote_search"),
            risk: Risk::High,
            ceiling: Risk::Medium,
        }
    );
}

#[test]
fn a_remote_peer_without_a_grant_cannot_satisfy_the_proposal() {
    let local = catalogue();
    let mut remote_catalogue = Catalogue::new();
    remote_catalogue
        .insert(descriptor("remote_search", Risk::Low))
        .expect("a unique descriptor");
    let node = capability::NodeId::new("peer-a").expect("a node");
    let remote = capability::composition::PeerCatalogue::new(node.clone(), remote_catalogue);
    let allowed: BTreeSet<_> = [capability("remote_search")].into_iter().collect();
    let mut spec = spec(vec![CapabilityRequirement::new(
        capability("remote_search"),
        VersionReq::STAR,
    )
    .with_remote(RemotePolicy::Allowed)]);
    spec.peers.push(capability::composition::PeerRef::new(node));

    assert_eq!(
        validate_proposal(&spec, &local, &[remote], &remote_policy(&allowed)).unwrap_err(),
        ProposalError::Ungranted {
            capability: capability("remote_search"),
        }
    );
}

#[test]
fn a_peer_outside_the_spec_allowlist_cannot_satisfy_a_proposal() {
    let local = catalogue();
    let (_peer, remote) = remote_peer("peer-a", "remote_search", Risk::Low);
    let allowed: BTreeSet<_> = [capability("remote_search")].into_iter().collect();
    let spec = spec(vec![CapabilityRequirement::new(
        capability("remote_search"),
        VersionReq::STAR,
    )
    .with_remote(RemotePolicy::Allowed)]);

    assert_eq!(
        validate_proposal(&spec, &local, &[remote], &remote_policy(&allowed)).unwrap_err(),
        ProposalError::Unknown {
            capability: capability("remote_search"),
        }
    );
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

    let refused = validate_proposal(&spec, &catalogue, &[], &policy(&allowed)).unwrap_err();

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

    let refused = validate_proposal(&spec, &catalogue, &[], &policy(&allowed)).unwrap_err();

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
        validate_proposal(&spec, &catalogue, &[], &policy(&allowed)).unwrap_err(),
        ProposalError::Remote {
            capability: capability("read_file"),
        }
    );

    let mut with_peer = spec;
    with_peer.peers.push(PeerRef::new(
        capability::NodeId::new("peer-a").expect("a node"),
    ));
    assert!(matches!(
        validate_proposal(&with_peer, &catalogue, &[], &policy(&allowed)).unwrap_err(),
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
        validate_proposal(&spec, &catalogue, &[], &policy(&allowed)).unwrap_err(),
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
        capability::proposal::approve(&spec, &catalogue, &[], &policy(&allowed)).expect("approved");

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

    let refused = capability::proposal::approve(&spec, &catalogue, &[], &policy(&allowed));

    assert!(
        refused.is_err(),
        "no token is produced for a refused proposal"
    );
}
