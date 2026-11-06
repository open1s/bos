//! A workspace dispatches local and remote bindings through one path.

use std::sync::Arc;

use async_trait::async_trait;
use capability::composition::{Binding, ResolvedCapability, ResolvedComposition};
use capability::descriptor::{CapabilityDescriptor, OutputContract};
use capability::id::{
    CapabilityId, CapabilityKind, NodeId, ProviderId, VersionSource, WorkspaceId,
};
use capability::provider::{
    CapabilityCall, CapabilityOutput, CapabilityProvider, ProviderHealth, ProviderKind,
};
use capability::{
    CapabilityError, GrantRef, InvocationContext, InvocationId, Principal, WorkspaceError,
    WorkspaceRuntime,
};
use semver::Version;
use serde_json::json;

struct Fake {
    provider: ProviderId,
    kind: ProviderKind,
    label: &'static str,
}

#[async_trait]
impl CapabilityProvider for Fake {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        self.kind.clone()
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        Ok(CapabilityOutput::text(self.label))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn descriptor(id: &str) -> CapabilityDescriptor {
    let id: CapabilityId = id.parse().expect("a valid id");
    CapabilityDescriptor::new(
        id,
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
    .with_output(OutputContract::json())
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:test").expect("a valid workspace id")
}

fn plan(binding: Binding, id: &str) -> ResolvedComposition {
    ResolvedComposition {
        id: "compose:test".into(),
        version: Version::new(1, 0, 0),
        workspace: workspace(),
        capabilities: vec![ResolvedCapability {
            descriptor: descriptor(id),
            binding,
        }],
        skipped: Vec::new(),
    }
}

fn context() -> InvocationContext {
    InvocationContext::local(
        InvocationId::new("inv:test"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    )
}

#[tokio::test]
async fn a_local_binding_dispatches_to_its_provider() {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Fake {
        provider: ProviderId::builtin(),
        kind: ProviderKind::Builtin,
        label: "local",
    });
    let plan = plan(
        Binding::Local {
            provider: ProviderId::builtin(),
        },
        "builtin/read_file",
    );
    let runtime = WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime");
    let id = "builtin/read_file".parse().expect("a valid id");

    let output = runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect("the provider answers");

    assert_eq!(output, CapabilityOutput::text("local"));
    assert_eq!(runtime.workspace(), &workspace());
    assert!(runtime.contains(&id));
}

#[tokio::test]
async fn a_remote_binding_dispatches_to_the_peer_provider() {
    let node = NodeId::new("node-b").expect("a valid node id");
    let peer: Arc<dyn CapabilityProvider> = Arc::new(Fake {
        provider: capability::workspace::remote_provider_id(&node),
        kind: ProviderKind::Remote { node: node.clone() },
        label: "remote",
    });
    let plan = plan(
        Binding::Remote {
            node: node.clone(),
            grant: GrantRef::new("grant:1"),
        },
        "builtin/build",
    );
    let runtime = WorkspaceRuntime::instantiate(plan, &[peer]).expect("a runtime");
    let id = "builtin/build".parse().expect("a valid id");

    let output = runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect("the peer provider answers");

    assert_eq!(output, CapabilityOutput::text("remote"));
    assert_eq!(runtime.workspace(), &workspace());
    assert!(matches!(
        runtime.binding(&id),
        Some(Binding::Remote { node: bound, .. }) if bound == &node
    ));
}

#[tokio::test]
async fn a_missing_provider_fails_at_instantiation_not_on_the_first_call() {
    let plan = plan(
        Binding::Local {
            provider: ProviderId::builtin(),
        },
        "builtin/read_file",
    );

    let error = WorkspaceRuntime::instantiate(plan, &[]).expect_err("no provider is registered");

    assert!(matches!(error, WorkspaceError::MissingProvider { .. }));
}

#[tokio::test]
async fn a_capability_outside_the_workspace_is_not_found() {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Fake {
        provider: ProviderId::builtin(),
        kind: ProviderKind::Builtin,
        label: "local",
    });
    let runtime = WorkspaceRuntime::instantiate(
        plan(
            Binding::Local {
                provider: ProviderId::builtin(),
            },
            "builtin/read_file",
        ),
        &[provider],
    )
    .expect("a runtime");
    let outside = "builtin/write_file".parse().expect("a valid id");

    let error = runtime
        .invoke(&outside, json!({}), &context())
        .await
        .expect_err("the workspace was not built with it");

    assert!(matches!(error, CapabilityError::NotFound { .. }));
}
