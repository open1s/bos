//! Manifests, dependency graphs, and lock reproducibility.

use capability::composition::{
    resolve_host, Binding, Budgets, CapabilityRequirement, CompositionSpec, GrantRef, LocalScope,
    PeerCatalogue, PeerRef, RemotePolicy,
};
use capability::descriptor::{CapabilityDescriptor, OutputContract};
use capability::id::{
    CapabilityId, CapabilityKind, NodeId, SurfaceName, VersionSource, WorkspaceId,
};
use capability::lock::{
    lock_resolution, resolve_locked, CompositionLock, CompositionManifest, DependencyEdge,
    DependencyGraph, LockError,
};
use capability::Catalogue;
use semver::{Version, VersionReq};

fn cap(value: &str) -> CapabilityId {
    value.parse().expect("a capability id")
}

fn node(value: &str) -> NodeId {
    NodeId::new(value).expect("a node id")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:lock").expect("a workspace")
}

fn descriptor(value: &str, version: &str) -> CapabilityDescriptor {
    let id = cap(value);
    let surface = SurfaceName::new(id.name().to_string());
    CapabilityDescriptor::new(
        id,
        CapabilityKind::Tool,
        Version::parse(version).expect("a version"),
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
    CapabilityRequirement::new(cap(value), VersionReq::parse(range).expect("a range"))
}

fn spec(requirements: &[CapabilityRequirement]) -> CompositionSpec {
    let mut spec = CompositionSpec::new(
        "compose:lock",
        Version::new(1, 0, 0),
        "a reproducible composition",
    );
    for requirement in requirements {
        spec = spec.with_requirement(requirement.clone());
    }
    spec
}

fn graph(
    requirements: &[CapabilityRequirement],
    edges: &[(&str, &str)],
) -> Result<DependencyGraph, LockError> {
    let dependencies: Vec<DependencyEdge> = edges
        .iter()
        .map(|(capability, depends_on)| DependencyEdge {
            capability: cap(capability),
            depends_on: cap(depends_on),
        })
        .collect();
    DependencyGraph::build(requirements, &dependencies)
}

#[test]
fn a_manifest_round_trips_and_its_fingerprint_ignores_order() {
    let mut spec = spec(&[
        requirement("builtin/read_file", "^1"),
        requirement("builtin/build", "^1"),
    ]);
    spec.peers
        .push(PeerRef::new(node("node-b")).with_grant(GrantRef::new("grant:b")));
    spec.budgets = Budgets {
        max_concurrent_invocations: 2,
        default_timeout: Some(std::time::Duration::from_millis(500)),
    };
    let manifest = CompositionManifest::new(spec)
        .with_dependency(cap("builtin/build"), cap("builtin/read_file"));
    let text = manifest.to_json();
    let back = CompositionManifest::from_json(&text).expect("round-trip");
    assert_eq!(back, manifest);
    assert_eq!(back.fingerprint(), manifest.fingerprint());

    let mut reordered = manifest.clone();
    reordered.spec.requirements.reverse();
    reordered.spec.peers.reverse();
    reordered.dependencies.reverse();
    assert_eq!(
        reordered.fingerprint(),
        manifest.fingerprint(),
        "semantically identical orderings hash equally"
    );
}

#[test]
fn an_unknown_schema_is_refused() {
    let mut manifest = CompositionManifest::new(spec(&[requirement("builtin/read_file", "^1")]));
    manifest.schema = 99;
    assert!(matches!(
        CompositionManifest::from_json(&manifest.to_json()),
        Err(LockError::ManifestSchema(99))
    ));
}

#[test]
fn the_graph_orders_dependencies_first_and_teardown_in_reverse() {
    let requirements = [
        requirement("builtin/a", "^1"),
        requirement("builtin/b", "^1"),
        requirement("builtin/c", "^1"),
    ];
    let graph = graph(
        &requirements,
        &[("builtin/a", "builtin/b"), ("builtin/b", "builtin/c")],
    )
    .expect("a graph");

    assert_eq!(
        graph.activation_order().expect("order"),
        vec![cap("builtin/c"), cap("builtin/b"), cap("builtin/a")]
    );
    assert_eq!(
        graph.teardown_order().expect("teardown"),
        vec![cap("builtin/a"), cap("builtin/b"), cap("builtin/c")]
    );
    assert_eq!(
        graph
            .transitive_dependencies(&cap("builtin/a"))
            .expect("closure"),
        vec![cap("builtin/b"), cap("builtin/c")]
    );
    assert_eq!(
        graph.direct_dependencies(&cap("builtin/a")),
        vec![cap("builtin/b")]
    );
}

#[test]
fn the_graph_is_deterministic_for_independent_nodes() {
    let requirements = [
        requirement("builtin/a", "^1"),
        requirement("builtin/b", "^1"),
        requirement("builtin/c", "^1"),
    ];
    let first = graph(&requirements, &[("builtin/c", "builtin/a")]).expect("a graph");
    let second = graph(&requirements, &[("builtin/c", "builtin/a")]).expect("the same graph");
    assert_eq!(
        first.activation_order().expect("order"),
        second.activation_order().expect("order")
    );
    assert_eq!(
        first.activation_order().expect("order"),
        vec![cap("builtin/a"), cap("builtin/b"), cap("builtin/c")]
    );
}

#[test]
fn a_cycle_is_reported_with_its_nodes() {
    let requirements = [
        requirement("builtin/a", "^1"),
        requirement("builtin/b", "^1"),
    ];
    let graph = graph(
        &requirements,
        &[("builtin/a", "builtin/b"), ("builtin/b", "builtin/a")],
    )
    .expect("a graph");

    assert_eq!(
        graph.activation_order().unwrap_err(),
        LockError::Cycle {
            capabilities: vec![cap("builtin/a"), cap("builtin/b")],
        }
    );
}

#[test]
fn an_edge_to_an_undeclared_capability_is_refused() {
    let requirements = [requirement("builtin/a", "^1")];
    assert!(matches!(
        graph(&requirements, &[("builtin/a", "builtin/ghost")]),
        Err(LockError::UnknownDependency { .. })
    ));
}

#[test]
fn a_lock_records_exact_bindings_and_reproduces() {
    let local = catalogue(&[("builtin/read_file", "1.2.0")]);
    let peers = [PeerCatalogue::new(
        node("node-b"),
        catalogue(&[("builtin/build", "2.1.0")]),
    )];
    let mut spec = spec(&[
        requirement("builtin/read_file", "^1"),
        requirement("builtin/build", "^2").with_remote(RemotePolicy::Required(node("node-b"))),
    ]);
    spec.peers
        .push(PeerRef::new(node("node-b")).with_grant(GrantRef::new("grant:b")));
    let manifest = CompositionManifest::new(spec)
        .with_dependency(cap("builtin/read_file"), cap("builtin/build"));
    let scope = LocalScope::admitting(workspace(), &local);

    let (resolved, lock) = resolve_locked(&manifest, &local, &peers, &scope).expect("locked");
    assert_eq!(lock.bindings.len(), 2);
    let binding = lock
        .bindings
        .iter()
        .find(|binding| binding.capability == cap("builtin/build"))
        .expect("the remote binding");
    assert_eq!(binding.version, Version::new(2, 1, 0));
    assert!(matches!(binding.binding, Binding::Remote { .. }));
    assert!(lock.catalogue.len() == 64, "a sha-256 catalogue revision");
    assert!(lock.peers[0]
        .catalogue
        .as_ref()
        .is_some_and(|rev| rev.len() == 64));
    assert_eq!(
        lock.activation_order,
        vec![cap("builtin/build"), cap("builtin/read_file")]
    );
    assert_eq!(
        lock.teardown_order,
        vec![cap("builtin/read_file"), cap("builtin/build")]
    );
    assert_eq!(lock.fingerprint, lock.recompute_fingerprint());
    assert!(!lock.fingerprint.is_empty());

    let decoded = CompositionLock::from_json(&lock.to_json()).expect("lock round-trip");
    assert_eq!(decoded, lock);
    let (_again, second) = resolve_locked(&manifest, &local, &peers, &scope).expect("stable");
    assert_eq!(second.fingerprint, lock.fingerprint);
    assert_eq!(second, lock);
    lock.verify(&manifest, &local, &peers, &scope)
        .expect("the lock matches a fresh resolution");
    let _ = resolved;
}

#[test]
fn re_resolution_reports_drift_instead_of_changing_bindings() {
    let old = catalogue(&[("builtin/read_file", "1.0.0")]);
    let new = catalogue(&[("builtin/read_file", "1.1.0")]);
    let manifest = CompositionManifest::new(spec(&[requirement("builtin/read_file", "*")]));
    let scope_old = LocalScope::admitting(workspace(), &old);
    let scope_new = LocalScope::admitting(workspace(), &new);

    let (_resolved, lock) = resolve_locked(&manifest, &old, &[], &scope_old).expect("old lock");
    let error = lock
        .verify(&manifest, &new, &[], &scope_new)
        .expect_err("the catalogue changed");
    match error {
        LockError::Drift { differences } => {
            assert!(differences
                .iter()
                .any(|drift| matches!(drift, capability::lock::Drift::Catalogue { .. })));
            assert!(differences
                .iter()
                .any(|drift| matches!(drift, capability::lock::Drift::Binding { .. })));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn a_peer_catalogue_change_is_drift() {
    let local = catalogue(&[]);
    let manifest = CompositionManifest::new({
        let mut spec =
            spec(&[requirement("builtin/build", "^1")
                .with_remote(RemotePolicy::Required(node("node-b")))]);
        spec.peers
            .push(PeerRef::new(node("node-b")).with_grant(GrantRef::new("grant:b")));
        spec
    });
    let first = [PeerCatalogue::new(
        node("node-b"),
        catalogue(&[("builtin/build", "1.0.0")]),
    )];
    let second = [PeerCatalogue::new(
        node("node-b"),
        catalogue(&[("builtin/build", "1.1.0")]),
    )];
    let scope = LocalScope::admitting(workspace(), &local);

    let (_resolved, lock) = resolve_locked(&manifest, &local, &first, &scope).expect("a lock");
    let error = lock
        .verify(&manifest, &local, &second, &scope)
        .expect_err("the peer catalogue changed");
    match error {
        LockError::Drift { differences } => assert!(differences
            .iter()
            .any(|drift| matches!(drift, capability::lock::Drift::Peer { .. }))),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn a_bound_capability_cannot_depend_on_a_skipped_one() {
    let local = catalogue(&[("builtin/a", "1.0.0")]);
    let manifest = CompositionManifest::new(spec(&[
        requirement("builtin/a", "^1"),
        requirement("builtin/optional", "^1").with_required(false),
    ]))
    .with_dependency(cap("builtin/a"), cap("builtin/optional"));
    let scope = LocalScope::admitting(workspace(), &local);
    let resolved = resolve_host(&manifest.spec, &local, &[], &scope).expect("a plan");

    assert!(matches!(
        lock_resolution(&manifest, &resolved, &local, &[]),
        Err(LockError::UnsatisfiedDependency { .. })
    ));
}

#[test]
fn a_tampered_lock_is_refused() {
    let local = catalogue(&[("builtin/read_file", "1.0.0")]);
    let manifest = CompositionManifest::new(spec(&[requirement("builtin/read_file", "^1")]));
    let scope = LocalScope::admitting(workspace(), &local);
    let (_resolved, lock) = resolve_locked(&manifest, &local, &[], &scope).expect("a lock");

    let mut value: serde_json::Value = serde_json::from_str(&lock.to_json()).expect("json");
    value["bindings"][0]["version"] = serde_json::json!("9.9.9");
    let text = serde_json::to_string(&value).expect("json");
    assert_eq!(
        CompositionLock::from_json(&text),
        Err(LockError::Fingerprint)
    );
}

#[test]
fn budgets_and_manifest_fingerprint_are_part_of_the_lock() {
    let local = catalogue(&[("builtin/read_file", "1.0.0")]);
    let mut spec = spec(&[requirement("builtin/read_file", "^1")]);
    spec.budgets = Budgets {
        max_concurrent_invocations: 3,
        default_timeout: None,
    };
    let manifest = CompositionManifest::new(spec);
    let other = CompositionManifest::new({
        let mut spec = manifest.spec.clone();
        spec.budgets.max_concurrent_invocations = 4;
        spec
    });
    let scope = LocalScope::admitting(workspace(), &local);
    let (_resolved, lock) = resolve_locked(&manifest, &local, &[], &scope).expect("a lock");

    assert_eq!(lock.budgets.max_concurrent_invocations, 3);
    let error = lock
        .verify(&other, &local, &[], &scope)
        .expect_err("a different manifest");
    match error {
        LockError::Drift { differences } => assert!(differences
            .iter()
            .any(|drift| matches!(drift, capability::lock::Drift::Manifest { .. }))),
        other => panic!("unexpected error: {other:?}"),
    }
}
