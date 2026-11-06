//! The producer the proposal gate needed: prompt, strict parse, approved plan.

use std::collections::BTreeSet;

use agent::composition::{compose, parse, prompt, ComposeError};
use capability::composition::LocalScope;
use capability::descriptor::OutputContract;
use capability::proposal::{ProposalError, ProposalPolicy};
use capability::{
    CapabilityDescriptor, CapabilityId, CapabilityKind, Catalogue, Risk, VersionSource,
};
use semver::Version;
use serde_json::json;

type WorkspaceId = capability::WorkspaceId;

fn descriptor(name: &str, risk: Risk) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        format!("builtin/{name}").parse().expect("an id"),
        CapabilityKind::Tool,
        Version::new(1, 2, 0),
        VersionSource::Declared,
    )
    .with_risk(risk)
    .with_output(OutputContract::json())
}

fn catalogue() -> Catalogue {
    let mut catalogue = Catalogue::new();
    for (name, risk) in [("read_file", Risk::Low), ("write_file", Risk::High)] {
        catalogue.insert(descriptor(name, risk)).expect("unique");
    }
    catalogue
}

fn scope(catalogue: &Catalogue) -> LocalScope {
    LocalScope::admitting(
        WorkspaceId::new("ws:model").expect("a workspace"),
        catalogue,
    )
}

fn policy(allowed: &BTreeSet<CapabilityId>) -> ProposalPolicy<'_> {
    ProposalPolicy {
        allowed,
        max_risk: Risk::Medium,
        allow_remote: false,
    }
}

fn approved(name: &str) -> BTreeSet<CapabilityId> {
    [format!("builtin/{name}").parse().expect("an id")]
        .into_iter()
        .collect()
}

#[test]
fn the_prompt_lists_exactly_the_catalogue() {
    let catalogue = catalogue();
    let text = prompt(&catalogue, "read one file");

    assert!(text.contains("builtin/read_file"), "{text}");
    assert!(text.contains("builtin/write_file"), "{text}");
    assert!(text.contains("version 1.2.0"), "{text}");
    assert!(text.contains("do not invent"), "{text}");
    assert!(text.contains("\"requirements\""), "{text}");
}

#[test]
fn a_conforming_proposal_resolves_at_the_catalogue_version() {
    let catalogue = catalogue();
    let allowed = approved("read_file");
    let proposal = json!({
        "requirements": [{ "capability": "builtin/read_file", "version": "^1.0", "remote": "forbidden" }]
    });

    let plan =
        compose(&proposal, &catalogue, &scope(&catalogue), &policy(&allowed)).expect("a plan");

    assert_eq!(plan.capabilities.len(), 1);
    assert_eq!(
        plan.capabilities[0].descriptor.version,
        Version::new(1, 2, 0)
    );
}

#[test]
fn an_unknown_capability_is_refused_before_validation() {
    let catalogue = catalogue();
    let proposal = json!({
        "requirements": [{ "capability": "builtin/ghost" }]
    });

    let error = parse(&proposal, &catalogue).unwrap_err();

    assert!(matches!(error, ComposeError::UnknownCapability(ref name) if name == "builtin/ghost"));
}

#[test]
fn an_unknown_field_is_refused() {
    let catalogue = catalogue();
    let proposal = json!({
        "requirements": [{ "capability": "builtin/read_file", "permissions": ["net.egress"] }]
    });

    let error = parse(&proposal, &catalogue).unwrap_err();

    assert!(matches!(error, ComposeError::Json(_)), "{error:?}");
}

#[test]
fn an_empty_requirement_list_is_refused() {
    let catalogue = catalogue();
    let error = parse(&json!({ "requirements": [] }), &catalogue).unwrap_err();
    assert!(matches!(error, ComposeError::Empty));
}

#[test]
fn a_bad_version_range_is_refused() {
    let catalogue = catalogue();
    let proposal = json!({
        "requirements": [{ "capability": "builtin/read_file", "version": "not-a-range" }]
    });

    let error = parse(&proposal, &catalogue).unwrap_err();

    assert!(
        matches!(error, ComposeError::InvalidVersion { .. }),
        "{error:?}"
    );
}

#[test]
fn a_risk_above_the_ceiling_is_refused_by_the_gate() {
    let catalogue = catalogue();
    let allowed = approved("write_file");
    let proposal = json!({
        "requirements": [{ "capability": "builtin/write_file" }]
    });

    let error = compose(&proposal, &catalogue, &scope(&catalogue), &policy(&allowed))
        .expect_err("too risky");

    assert!(matches!(
        error,
        ComposeError::Refused(ProposalError::Risk { .. })
    ));
}

#[test]
fn a_remote_binding_is_refused_when_the_host_forbids_it() {
    let catalogue = catalogue();
    let allowed = approved("read_file");
    let proposal = json!({
        "requirements": [{ "capability": "builtin/read_file", "remote": "allowed" }]
    });

    let error = compose(&proposal, &catalogue, &scope(&catalogue), &policy(&allowed))
        .expect_err("remote is forbidden");

    assert!(matches!(
        error,
        ComposeError::Refused(ProposalError::Remote { .. })
    ));
}

#[test]
fn a_peer_binding_parses_to_a_required_remote() {
    let catalogue = catalogue();
    let proposal = json!({
        "requirements": [{ "capability": "builtin/read_file", "remote": "peer:peer-a" }]
    });

    let spec = parse(&proposal, &catalogue).expect("a spec");

    assert!(matches!(
        spec.requirements[0].remote,
        capability::composition::RemotePolicy::Required(_)
    ));
}
