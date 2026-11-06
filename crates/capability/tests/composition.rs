//! One resolver, two composition scopes.
//!
//! Every case here is a rule from ADR-008: the layers share a capability model
//! and separate their authority.

use capability::catalogue::Catalogue;
use capability::composition::{
    resolve, Binding, CapabilityRequirement, CompositionError, CompositionSpec, GrantRef,
    LocalScope, PeerCatalogue, PeerRef, RemotePolicy,
};
use capability::descriptor::{CapabilityDescriptor, OutputContract};
use capability::id::{
    CapabilityId, CapabilityKind, NodeId, SurfaceName, VersionSource, WorkspaceId,
};
use semver::{Version, VersionReq};

fn cap(value: &str) -> CapabilityId {
    value.parse().expect("a valid capability id")
}

fn node(value: &str) -> NodeId {
    NodeId::new(value).expect("a valid node id")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:test").expect("a valid workspace id")
}

fn descriptor(value: &str, version: &str) -> CapabilityDescriptor {
    let id = cap(value);
    let surface = SurfaceName::new(id.name().to_string());
    CapabilityDescriptor::new(
        id,
        CapabilityKind::Tool,
        Version::parse(version).expect("a valid version"),
        VersionSource::Declared,
    )
    .with_display_name(value)
    .with_description("a test capability")
    .with_input_schema(serde_json::json!({ "type": "object" }))
    .with_output(OutputContract::json())
    .with_surface(surface)
}

fn catalogue(entries: &[(&str, &str)]) -> Catalogue {
    let mut catalogue = Catalogue::new();
    for (id, version) in entries {
        catalogue
            .insert(descriptor(id, version))
            .expect("a unique descriptor");
    }
    catalogue
}

fn requirement(value: &str, range: &str) -> CapabilityRequirement {
    CapabilityRequirement::new(cap(value), VersionReq::parse(range).expect("a valid range"))
}

fn spec() -> CompositionSpec {
    CompositionSpec::new(
        "compose:test",
        Version::parse("1.0.0").expect("a valid version"),
        "a test composition",
    )
}

fn granted(node_id: &str) -> PeerRef {
    PeerRef::new(node(node_id)).with_grant(GrantRef::new(format!("grant:{node_id}")))
}

#[test]
fn a_local_spec_resolves_entirely_inside_one_workspace() {
    let local = catalogue(&[("builtin/read_file", "1.2.0"), ("builtin/grep", "1.0.0")]);
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec()
        .with_requirement(requirement("builtin/read_file", "^1"))
        .with_requirement(requirement("builtin/grep", "^1"));

    let resolved = resolve(&spec, &local, &[], &scope).expect("a resolvable composition");

    assert_eq!(resolved.capabilities.len(), 2);
    assert!(resolved.is_local_only());
    assert_eq!(resolved.remote_nodes().len(), 0);
    assert_eq!(resolved.workspace, workspace());
    assert!(matches!(
        resolved.capabilities[0].binding,
        Binding::Local { .. }
    ));
}

#[test]
fn an_optional_capability_that_is_absent_is_skipped() {
    let local = catalogue(&[("builtin/read_file", "1.0.0")]);
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec()
        .with_requirement(requirement("builtin/read_file", "^1"))
        .with_requirement(requirement("builtin/github_issues", "^1").with_required(false));

    let resolved = resolve(&spec, &local, &[], &scope).expect("a resolvable composition");

    assert_eq!(resolved.capabilities.len(), 1);
    assert_eq!(resolved.skipped, vec![cap("builtin/github_issues")]);
}

#[test]
fn a_missing_required_capability_fails_without_activation() {
    let local = catalogue(&[]);
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec().with_requirement(requirement("builtin/read_file", "^1"));

    let error = resolve(&spec, &local, &[], &scope).expect_err("the spec cannot be satisfied");

    assert!(matches!(
        error,
        CompositionError::Missing { capability } if capability == cap("builtin/read_file")
    ));
}

#[test]
fn a_version_outside_the_accepted_range_fails() {
    let local = catalogue(&[("builtin/read_file", "1.4.0")]);
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec().with_requirement(requirement("builtin/read_file", "^2"));

    let error = resolve(&spec, &local, &[], &scope).expect_err("the range excludes the candidate");

    assert!(matches!(
        error,
        CompositionError::VersionMismatch { capability, .. } if capability == cap("builtin/read_file")
    ));
}

#[test]
fn the_workspace_view_refuses_a_capability_it_does_not_admit() {
    let local = catalogue(&[("builtin/read_file", "1.0.0")]);
    let scope = LocalScope::new(workspace());
    let spec = spec().with_requirement(requirement("builtin/read_file", "^1"));

    let error = resolve(&spec, &local, &[], &scope).expect_err("nothing is admitted");

    assert!(matches!(
        error,
        CompositionError::NotAdmitted { capability } if capability == cap("builtin/read_file")
    ));
}

#[test]
fn a_refused_local_capability_is_never_rerouted_to_a_peer() {
    let local = catalogue(&[("builtin/read_file", "1.0.0")]);
    let peer = PeerCatalogue::new(node("node-b"), catalogue(&[("builtin/read_file", "1.0.0")]));
    let scope = LocalScope::new(workspace());
    let spec = spec().with_peer(granted("node-b")).with_requirement(
        requirement("builtin/read_file", "^1").with_remote(RemotePolicy::Allowed),
    );

    let error = resolve(&spec, &local, &[peer], &scope)
        .expect_err("the view refusal must not be bypassed by a peer");

    assert!(matches!(
        error,
        CompositionError::NotAdmitted { capability } if capability == cap("builtin/read_file")
    ));
}

#[test]
fn an_allowed_remote_capability_binds_to_the_first_granted_peer() {
    let local = catalogue(&[]);
    let peer = PeerCatalogue::new(node("node-b"), catalogue(&[("builtin/build", "1.3.0")]));
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec()
        .with_peer(granted("node-b"))
        .with_requirement(requirement("builtin/build", "^1").with_remote(RemotePolicy::Allowed));

    let resolved = resolve(&spec, &local, &[peer], &scope).expect("the peer satisfies it");

    assert!(!resolved.is_local_only());
    assert_eq!(resolved.workspace, workspace());
    assert_eq!(resolved.remote_nodes().len(), 1);
    match &resolved.capabilities[0].binding {
        Binding::Remote { node: bound, grant } => {
            assert_eq!(bound, &node("node-b"));
            assert_eq!(grant.as_str(), "grant:node-b");
        }
        Binding::Local { .. } => panic!("the peer answered, so the binding is remote"),
    }
}

#[test]
fn a_forbidden_remote_capability_never_reaches_a_peer() {
    let local = catalogue(&[]);
    let peer = PeerCatalogue::new(node("node-b"), catalogue(&[("builtin/build", "1.0.0")]));
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec()
        .with_peer(granted("node-b"))
        .with_requirement(requirement("builtin/build", "^1"));

    let error = resolve(&spec, &local, &[peer], &scope).expect_err("remote is forbidden");

    assert!(matches!(
        error,
        CompositionError::Missing { capability } if capability == cap("builtin/build")
    ));
}

#[test]
fn a_required_peer_does_not_fall_back_to_a_local_implementation() {
    let local = catalogue(&[("builtin/build", "1.0.0")]);
    let peer = PeerCatalogue::new(node("node-b"), catalogue(&[]));
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec().with_peer(granted("node-b")).with_requirement(
        requirement("builtin/build", "^1").with_remote(RemotePolicy::Required(node("node-b"))),
    );

    let error = resolve(&spec, &local, &[peer], &scope)
        .expect_err("a local implementation is not a substitute for the named peer");

    assert!(matches!(
        error,
        CompositionError::PeerCapabilityMissing { node: peer, .. } if peer == node("node-b")
    ));
}

#[test]
fn a_required_peer_must_be_allowlisted() {
    let local = catalogue(&[]);
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec().with_requirement(
        requirement("builtin/build", "^1").with_remote(RemotePolicy::Required(node("node-c"))),
    );

    let error = resolve(&spec, &local, &[], &scope).expect_err("node-c is not allowed");

    assert!(matches!(
        error,
        CompositionError::PeerNotAllowed { node: refused } if refused == node("node-c")
    ));
}

#[test]
fn a_remote_binding_requires_a_grant() {
    let local = catalogue(&[]);
    let peer = PeerCatalogue::new(node("node-b"), catalogue(&[("builtin/build", "1.0.0")]));
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec()
        .with_peer(PeerRef::new(node("node-b")))
        .with_requirement(requirement("builtin/build", "^1").with_remote(RemotePolicy::Allowed));

    let error = resolve(&spec, &local, &[peer], &scope).expect_err("no grant, no binding");

    assert!(matches!(
        error,
        CompositionError::MissingGrant { node: refused, .. } if refused == node("node-b")
    ));
}

#[test]
fn the_first_allowed_peer_wins_and_the_order_is_the_spec_order() {
    let local = catalogue(&[]);
    let peers = [
        PeerCatalogue::new(node("node-b"), catalogue(&[("builtin/build", "1.0.0")])),
        PeerCatalogue::new(node("node-c"), catalogue(&[("builtin/build", "1.0.0")])),
    ];
    let scope = LocalScope::admitting(workspace(), &local);
    let requirement_ = requirement("builtin/build", "^1").with_remote(RemotePolicy::Allowed);

    let forwards = spec()
        .with_peer(granted("node-b"))
        .with_peer(granted("node-c"))
        .with_requirement(requirement_.clone());
    let resolved = resolve(&forwards, &local, &peers, &scope).expect("node-b answers first");
    assert_eq!(
        resolved.capabilities[0].binding,
        Binding::Remote {
            node: node("node-b"),
            grant: GrantRef::new("grant:node-b"),
        }
    );

    let backwards = spec()
        .with_peer(granted("node-c"))
        .with_peer(granted("node-b"))
        .with_requirement(requirement_);
    let resolved = resolve(&backwards, &local, &peers, &scope).expect("node-c answers first");
    assert_eq!(
        resolved.capabilities[0].binding,
        Binding::Remote {
            node: node("node-c"),
            grant: GrantRef::new("grant:node-c"),
        }
    );
}

#[test]
fn duplicate_requirements_are_refused() {
    let local = catalogue(&[("builtin/read_file", "1.0.0")]);
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec()
        .with_requirement(requirement("builtin/read_file", "^1"))
        .with_requirement(requirement("builtin/read_file", "^1"));

    let error = resolve(&spec, &local, &[], &scope).expect_err("the spec is ambiguous");

    assert!(matches!(error, CompositionError::Invalid { .. }));
}

#[test]
fn one_spec_resolves_locally_or_remotely_and_only_the_binding_changes() {
    let spec = spec()
        .with_peer(granted("node-b"))
        .with_requirement(requirement("builtin/build", "^1").with_remote(RemotePolicy::Allowed));
    let local_plan = catalogue(&[("builtin/build", "1.0.0")]);
    let local_scope = LocalScope::admitting(workspace(), &local_plan);
    let here = resolve(&spec, &local_plan, &[], &local_scope).expect("resolved locally");
    assert!(here.is_local_only());

    let empty = catalogue(&[]);
    let remote_scope = LocalScope::admitting(workspace(), &empty);
    let there = resolve(
        &spec,
        &empty,
        &[PeerCatalogue::new(
            node("node-b"),
            catalogue(&[("builtin/build", "1.0.0")]),
        )],
        &remote_scope,
    )
    .expect("resolved remotely");
    assert!(!there.is_local_only());

    let local_descriptor = &here.capabilities[0].descriptor;
    let remote_descriptor = &there.capabilities[0].descriptor;
    assert_eq!(local_descriptor.id, remote_descriptor.id);
    assert_eq!(local_descriptor.version, remote_descriptor.version);
    assert_eq!(local_descriptor.surface, remote_descriptor.surface);
}

#[test]
fn a_catalogue_refuses_a_duplicate_id_and_a_duplicate_surface() {
    let mut catalogue = catalogue(&[("builtin/read_file", "1.0.0")]);

    let duplicate = catalogue.insert(descriptor("builtin/read_file", "1.0.0"));
    assert!(duplicate.is_err());

    let shared_surface =
        descriptor("mcp:github/read_file", "1.0.0").with_surface(SurfaceName::new("read_file"));
    let error = catalogue.insert(shared_surface);
    assert!(error.is_err());
}
