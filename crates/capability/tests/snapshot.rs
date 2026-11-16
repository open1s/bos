//! A snapshot records identity and origin, never a secret.

use capability::composition::{CapabilityRequirement, CompositionSpec, LocalScope};
use capability::descriptor::{CapabilityDescriptor, OutputContract};
use capability::id::{CapabilityId, CapabilityKind, NodeId, VersionSource, WorkspaceId};
use capability::{resolve_host, Catalogue, CompositionSnapshot, SnapshotOrigin};
use semver::{Version, VersionReq};

fn descriptor(id: &str, version: &str) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        id.parse().expect("a valid id"),
        CapabilityKind::Tool,
        Version::parse(version).expect("a valid version"),
        VersionSource::Declared,
    )
    .with_output(OutputContract::json())
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:test").expect("a valid workspace id")
}

#[test]
fn a_local_plan_snapshots_versions_and_providers() {
    let mut catalogue = Catalogue::new();
    catalogue
        .insert(descriptor("builtin/read_file", "1.2.0"))
        .unwrap();
    catalogue
        .insert(descriptor("builtin/grep", "1.0.0"))
        .unwrap();
    let scope = LocalScope::admitting(workspace(), &catalogue);
    let spec = CompositionSpec::new("compose:test", Version::new(1, 0, 0), "snapshot")
        .with_requirement(CapabilityRequirement::new(
            "builtin/read_file".parse().unwrap(),
            VersionReq::STAR,
        ))
        .with_requirement(CapabilityRequirement::new(
            "builtin/grep".parse().unwrap(),
            VersionReq::STAR,
        ));
    let resolved = resolve_host(&spec, &catalogue, &[], &scope).expect("resolved");

    let snapshot = CompositionSnapshot::from_resolved(&resolved, Some("policy:turn".into()), 42);

    assert_eq!(snapshot.composition, "compose:test");
    assert_eq!(snapshot.workspace, "ws:test");
    assert_eq!(snapshot.created_at_ms, 42);
    assert_eq!(snapshot.policy.as_deref(), Some("policy:turn"));
    assert!(snapshot.is_local_only());
    assert_eq!(snapshot.capabilities.len(), 2);
    let read = snapshot
        .capabilities
        .iter()
        .find(|capability| capability.id == "builtin/read_file")
        .expect("the file tool");
    assert_eq!(read.version, "1.2.0");
    assert_eq!(read.provider, "builtin");
    assert_eq!(
        read.origin,
        SnapshotOrigin::Local {
            provider: "builtin".to_string()
        }
    );
}

#[test]
fn a_remote_plan_records_the_peer_and_the_grant() {
    let local = Catalogue::new();
    let peer = capability::PeerCatalogue::new(NodeId::new("node-b").expect("a valid node"), {
        let mut catalogue = Catalogue::new();
        catalogue
            .insert(descriptor("builtin/build", "1.0.0"))
            .unwrap();
        catalogue
    });
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = CompositionSpec::new("compose:fed", Version::new(1, 0, 0), "federated")
        .with_peer(
            capability::PeerRef::new(NodeId::new("node-b").unwrap())
                .with_grant(capability::GrantRef::new("grant:builder")),
        )
        .with_requirement(
            CapabilityRequirement::new("builtin/build".parse().unwrap(), VersionReq::STAR)
                .with_remote(capability::RemotePolicy::Allowed),
        );
    let resolved = resolve_host(&spec, &local, &[peer], &scope).expect("resolved");

    let snapshot = CompositionSnapshot::from_resolved(&resolved, None, 7);

    assert!(!snapshot.is_local_only());
    assert_eq!(snapshot.remote_capabilities().count(), 1);
    assert_eq!(
        snapshot.capabilities[0].origin,
        SnapshotOrigin::Remote {
            node: "node-b".to_string(),
            grant: "grant:builder".to_string(),
        }
    );
}

#[test]
fn a_snapshot_serializes_without_secret_material() {
    let mut catalogue = Catalogue::new();
    catalogue
        .insert(descriptor("builtin/read_file", "1.0.0"))
        .unwrap();
    let scope = LocalScope::admitting(workspace(), &catalogue);
    let spec =
        CompositionSpec::new("compose:test", Version::new(1, 0, 0), "snapshot").with_requirement(
            CapabilityRequirement::new("builtin/read_file".parse().unwrap(), VersionReq::STAR),
        );
    let resolved = resolve_host(&spec, &catalogue, &[], &scope).expect("resolved");
    let snapshot = CompositionSnapshot::from_resolved(&resolved, None, 1);

    let json = serde_json::to_string(&snapshot).expect("serializes");
    for forbidden in ["api_key", "secret", "token", "password", "credential"] {
        assert!(!json.contains(forbidden), "{json}");
    }
    let back: CompositionSnapshot = serde_json::from_str(&json).expect("round trips");
    assert_eq!(back, snapshot);
    assert_eq!(
        snapshot.capabilities[0].id,
        CapabilityId::new("builtin", "read_file")
            .unwrap()
            .to_string()
    );
}
