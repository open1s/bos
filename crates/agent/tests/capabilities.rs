//! One model, two composition levels, over tools that really exist.
//!
//! The catalogue is built from the engine's own registry, so these cases pin
//! the connection between a registered tool and a resolved composition.

use std::sync::Arc;

use agent::capabilities::{catalogue_from_registry, NativeToolProvider};
use agent::tools::{FunctionTool, ToolMetadata, ToolRegistry};
use capability::composition::{
    resolve, Binding, CapabilityRequirement, CompositionSpec, GrantRef, LocalScope, PeerCatalogue,
    PeerRef, RemotePolicy,
};
use capability::id::{NodeId, WorkspaceId};
use capability::{
    CapabilityId, CapabilityProvider, InvocationContext, InvocationId, Principal, Risk, SideEffect,
    VersionSource, WorkspaceRuntime,
};
use semver::{Version, VersionReq};
use serde_json::json;

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "a": { "type": "number" },
            "b": { "type": "number" }
        },
        "required": ["a", "b"]
    })
}

fn registry_with_adder() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(FunctionTool::new(
            "echo_add",
            "Add two numbers",
            schema(),
            |args| {
                let a = args
                    .get("a")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                let b = args
                    .get("b")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                Ok(json!({ "sum": a + b }))
            },
        )))
        .expect("a unique tool");
    registry
}

fn adder_id() -> CapabilityId {
    CapabilityId::new("builtin", "echo_add").expect("a valid capability id")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:agent").expect("a valid workspace id")
}

fn spec_with(remote: RemotePolicy) -> CompositionSpec {
    CompositionSpec::new("compose:adder", Version::new(1, 0, 0), "add two numbers")
        .with_requirement(
            CapabilityRequirement::new(adder_id(), VersionReq::parse("^0.1").expect("a range"))
                .with_remote(remote),
        )
}

#[test]
fn a_registered_tool_becomes_exactly_one_descriptor() {
    let registry = registry_with_adder();
    let catalogue = catalogue_from_registry(&registry).expect("a catalogue");

    assert_eq!(catalogue.len(), 1);
    let descriptor = catalogue.get(&adder_id()).expect("the added tool");
    assert_eq!(descriptor.surface.as_str(), "echo_add");
    assert_eq!(descriptor.description, "Add two numbers");
    assert_eq!(descriptor.input_schema, schema());
    assert_eq!(descriptor.version_source, VersionSource::Derived);
}

#[test]
fn the_catalogue_and_the_registry_name_the_same_tools() {
    let registry = registry_with_adder();
    let catalogue = catalogue_from_registry(&registry).expect("a catalogue");

    let mut from_registry = registry.list();
    from_registry.sort();
    let mut from_catalogue: Vec<String> = catalogue
        .iter()
        .map(|descriptor| descriptor.surface.to_string())
        .collect();
    from_catalogue.sort();

    assert_eq!(from_registry, from_catalogue);
}

#[test]
fn a_tool_name_that_cannot_form_an_id_is_reported_not_dropped() {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(FunctionTool::new(
            "not/a/name",
            "a tool whose name is not a capability name",
            schema(),
            |_| Ok(json!({})),
        )))
        .expect("the registry accepts any unique name");

    let error = catalogue_from_registry(&registry).expect_err("the name cannot form an id");
    assert!(error.to_string().contains("invalid identity"), "{error}");
}

#[test]
fn a_local_composition_resolves_over_real_registered_tools() {
    let catalogue = catalogue_from_registry(&registry_with_adder()).expect("a catalogue");
    let scope = LocalScope::admitting(workspace(), &catalogue);

    let resolved = resolve(&spec_with(RemotePolicy::Forbidden), &catalogue, &[], &scope)
        .expect("resolved locally");

    assert!(resolved.is_local_only());
    assert_eq!(resolved.workspace, workspace());
    assert_eq!(
        resolved.capabilities[0].binding,
        Binding::Local {
            provider: capability::ProviderId::builtin(),
        }
    );
    assert_eq!(
        resolved
            .descriptor(&adder_id())
            .expect("a descriptor")
            .input_schema,
        schema()
    );
}

#[test]
fn the_same_spec_resolves_against_a_peer_when_the_tool_is_not_local() {
    let local = catalogue_from_registry(&ToolRegistry::new()).expect("an empty catalogue");
    let peer_node = NodeId::new("node:builder").expect("a valid node id");
    let peer = PeerCatalogue::new(
        peer_node.clone(),
        catalogue_from_registry(&registry_with_adder()).expect("the peer catalogue"),
    );
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = spec_with(RemotePolicy::Allowed)
        .with_peer(PeerRef::new(peer_node.clone()).with_grant(GrantRef::new("grant:builder")));

    let resolved = resolve(&spec, &local, &[peer], &scope).expect("resolved against the peer");

    assert!(!resolved.is_local_only());
    assert_eq!(resolved.workspace, workspace());
    match &resolved.capabilities[0].binding {
        Binding::Remote { node, grant } => {
            assert_eq!(node, &peer_node);
            assert_eq!(grant.as_str(), "grant:builder");
        }
        Binding::Local { .. } => panic!("nothing local implements the tool"),
    }
}

#[tokio::test]
async fn a_resolved_plan_executes_through_the_workspace_runtime() {
    let registry = registry_with_adder();
    let catalogue = catalogue_from_registry(&registry).expect("a catalogue");
    let scope = LocalScope::admitting(workspace(), &catalogue);
    let resolved = resolve(&spec_with(RemotePolicy::Forbidden), &catalogue, &[], &scope)
        .expect("resolved locally");

    let provider: Arc<dyn CapabilityProvider> =
        Arc::new(NativeToolProvider::from_registry(&registry));
    let runtime = WorkspaceRuntime::instantiate(resolved, &[provider]).expect("a runtime");
    let context = InvocationContext::local(
        InvocationId::new("inv:1"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    );

    let output = runtime
        .invoke(&adder_id(), json!({ "a": 2, "b": 5 }), &context)
        .await
        .expect("the real tool runs");

    assert_eq!(output.structured, Some(json!({ "sum": 7 })));
    assert_eq!(runtime.workspace(), &workspace());
}

#[test]
fn declared_metadata_reaches_the_descriptor() {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(
            FunctionTool::new("write_thing", "Writes a thing", schema(), |_| Ok(json!({})))
                .with_metadata(
                    ToolMetadata::new()
                        .with_version("2.1.0")
                        .with_risk("high")
                        .with_side_effects(["write", "network"])
                        .with_permissions(["fs.write:workspace"])
                        .with_timeout_ms(5_000)
                        .with_max_output_bytes(1_024),
                ),
        ))
        .expect("a unique tool");
    let catalogue = catalogue_from_registry(&registry).expect("a catalogue");
    let descriptor = catalogue
        .get(&CapabilityId::new("builtin", "write_thing").expect("a valid id"))
        .expect("the declared tool");

    assert_eq!(descriptor.version, Version::new(2, 1, 0));
    assert_eq!(descriptor.version_source, VersionSource::Declared);
    assert_eq!(descriptor.risk, Risk::High);
    assert_eq!(
        descriptor.side_effects,
        vec![SideEffect::Write, SideEffect::Network]
    );
    assert_eq!(descriptor.permissions[0].as_str(), "fs.write:workspace");
    assert_eq!(descriptor.limits.timeout_ms, Some(5_000));
    assert_eq!(descriptor.limits.max_output_bytes, Some(1_024));
}

#[test]
fn an_unknown_risk_class_is_an_error_not_a_default() {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(
            FunctionTool::new(
                "risky",
                "Declares a class that does not exist",
                schema(),
                |_| Ok(json!({})),
            )
            .with_metadata(ToolMetadata::new().with_risk("extreme")),
        ))
        .expect("a unique tool");

    let error = catalogue_from_registry(&registry).expect_err("the declaration is invalid");
    assert!(error.to_string().contains("extreme"), "{error}");
}

/// A declaration without a version is Unknown, not a fabricated version: an
/// MCP tool has no version of its own, and a range check against one would be a
/// promise the provider never made.
#[test]
fn metadata_without_a_version_is_unknown() {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(
            FunctionTool::new("remote_tool", "A remote tool", schema(), |_| Ok(json!({})))
                .with_metadata(
                    ToolMetadata::new()
                        .with_risk("medium")
                        .with_side_effects(["network"])
                        .with_permissions(["net.egress"]),
                ),
        ))
        .expect("a unique tool");

    let catalogue = catalogue_from_registry(&registry).expect("a catalogue");
    let descriptor = catalogue
        .get(&CapabilityId::new("builtin", "remote_tool").expect("an id"))
        .expect("the tool");

    assert_eq!(descriptor.version_source, VersionSource::Unknown);
    assert_eq!(descriptor.risk, Risk::Medium);
    assert_eq!(descriptor.side_effects, vec![SideEffect::Network]);
    assert_eq!(descriptor.permissions[0].as_str(), "net.egress");
}

/// The MCP adapter's own declaration: network egress, medium risk, no version.
#[test]
fn an_mcp_tool_declares_the_network_and_no_version() {
    let metadata = agent::mcp::adapter::mcp_tool_metadata();

    assert!(metadata.version.is_none());
    assert_eq!(metadata.risk.as_deref(), Some("medium"));
    assert!(metadata
        .side_effects
        .iter()
        .any(|effect| effect == "network"));
    assert!(metadata
        .permissions
        .iter()
        .any(|scope| scope == "net.egress"));
}
