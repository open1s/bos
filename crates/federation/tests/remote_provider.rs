//! A remote binding executes through the peer provider, with no local fallback.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use capability::composition::{
    resolve_host, Binding, CapabilityRequirement, CompositionSpec, GrantRef, LocalScope,
    PeerCatalogue, PeerRef, RemotePolicy,
};
use capability::{
    remote_provider_id, CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityId,
    CapabilityKind, CapabilityOutput, CapabilityProvider, Catalogue, DeliveryState,
    InvocationContext, InvocationId, NodeId, Principal, ProviderHealth, ProviderId, ProviderKind,
    VersionSource, WorkspaceId, WorkspaceRuntime,
};
use federation::{FederationNode, InMemoryNetwork, NodeIdentity, RemoteCapabilityProvider};
use semver::{Version, VersionReq};
use serde_json::json;

struct Echo {
    provider: ProviderId,
    descriptor: CapabilityDescriptor,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl CapabilityProvider for Echo {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(vec![self.descriptor.clone()])
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CapabilityOutput::json(json!({
            "echo": call.input,
            "provider": self.provider.as_str(),
        })))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

fn descriptor(name: &str) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        capability(name),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
    .with_display_name(name)
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:caller").expect("a valid workspace id")
}

fn context() -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new("inv:remote"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(5));
    context
}

fn echo_runtime(
    descriptor: CapabilityDescriptor,
    calls: Arc<AtomicUsize>,
) -> Arc<WorkspaceRuntime> {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Echo {
        provider: ProviderId::builtin(),
        descriptor: descriptor.clone(),
        calls,
    });
    let plan = capability::composition::ResolvedComposition {
        id: "compose:peer".into(),
        version: Version::new(1, 0, 0),
        workspace: WorkspaceId::new("ws:peer").expect("a valid workspace id"),
        capabilities: vec![capability::composition::ResolvedCapability {
            descriptor,
            binding: Binding::Local {
                provider: ProviderId::builtin(),
            },
        }],
        skipped: Vec::new(),
    };
    Arc::new(WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime"))
}

struct Pair {
    caller: Arc<FederationNode>,
    provider_node: NodeId,
    calls: Arc<AtomicUsize>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn pair() -> Pair {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(31);
    let provider_identity = identity(32);
    let caller_id = caller_identity.node_id();
    let provider_node = provider_identity.node_id();

    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_node.clone());

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = FederationNode::new(provider_identity, Arc::new(provider_transport))
        .with_runtime(echo_runtime(descriptor("echo"), Arc::clone(&calls)));
    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));

    let tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];

    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(provider_node.clone(), now, None);
        store
            .grant(&provider_node, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = provider.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(caller_id.clone(), now, None);
        store
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }

    tokio::time::sleep(Duration::from_millis(20)).await;

    Pair {
        caller,
        provider_node,
        calls,
        _tasks: tasks,
    }
}

fn remote_spec(provider_node: &NodeId) -> CompositionSpec {
    CompositionSpec::new("compose:remote", Version::new(1, 0, 0), "remote invocation")
        .with_peer(PeerRef::new(provider_node.clone()).with_grant(GrantRef::new("grant:peer")))
        .with_requirement(
            CapabilityRequirement::new(
                capability("echo"),
                VersionReq::parse("^1").expect("a range"),
            )
            .with_remote(RemotePolicy::Allowed),
        )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_binding_executes_through_the_peer_provider() {
    let pair = pair().await;
    let local = Catalogue::new();
    let scope = LocalScope::admitting(workspace(), &local);
    let peer = PeerCatalogue::new(pair.provider_node.clone(), {
        let mut catalogue = Catalogue::new();
        catalogue.insert(descriptor("echo")).expect("unique");
        catalogue
    });
    let plan = resolve_host(&remote_spec(&pair.provider_node), &local, &[peer], &scope)
        .expect("resolved remotely");
    assert!(matches!(
        plan.capabilities[0].binding,
        Binding::Remote { .. }
    ));

    let provider: Arc<dyn CapabilityProvider> = Arc::new(RemoteCapabilityProvider::new(
        pair.provider_node.clone(),
        Arc::clone(&pair.caller),
        vec![descriptor("echo")],
    ));
    let runtime = WorkspaceRuntime::instantiate(plan, &[provider]).expect("a workspace");

    let output = runtime
        .invoke(&capability("echo"), json!({ "v": 1 }), &context())
        .await
        .expect("the peer answers");

    assert_eq!(output.structured.unwrap()["echo"]["v"], 1);
    assert_eq!(pair.calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.workspace(), &workspace());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_peer_fails_without_a_local_fallback() {
    let pair = pair().await;
    // A local provider that could answer the same id, if dispatch were by id
    // rather than by binding.
    let local_calls = Arc::new(AtomicUsize::new(0));
    let local_provider: Arc<dyn CapabilityProvider> = Arc::new(Echo {
        provider: ProviderId::builtin(),
        descriptor: descriptor("echo"),
        calls: Arc::clone(&local_calls),
    });

    let ghost = identity(33).node_id();
    let now = capability::unix_ms();
    {
        let store = pair.caller.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(ghost.clone(), now, None);
        store
            .grant(&ghost, capability("echo"), now, None)
            .expect("granted");
    }
    let local = Catalogue::new();
    let scope = LocalScope::admitting(workspace(), &local);
    let peer = PeerCatalogue::new(ghost.clone(), {
        let mut catalogue = Catalogue::new();
        catalogue.insert(descriptor("echo")).expect("unique");
        catalogue
    });
    let plan =
        resolve_host(&remote_spec(&ghost), &local, &[peer], &scope).expect("resolved remotely");

    let provider: Arc<dyn CapabilityProvider> = Arc::new(RemoteCapabilityProvider::new(
        ghost.clone(),
        Arc::clone(&pair.caller),
        vec![descriptor("echo")],
    ));
    let runtime = WorkspaceRuntime::instantiate(plan, &[local_provider, provider])
        .expect("a workspace with both providers");

    let error = runtime
        .invoke(&capability("echo"), json!({}), &context())
        .await
        .expect_err("the peer is not on the network");

    assert!(
        matches!(
            error,
            CapabilityError::Transport {
                delivered: DeliveryState::NotSent,
                ..
            } | CapabilityError::UnknownOutcome { .. }
        ),
        "{error:?}"
    );
    assert_eq!(
        local_calls.load(Ordering::SeqCst),
        0,
        "a remote binding must never fall back to a local provider"
    );
}

#[tokio::test]
async fn the_remote_provider_reports_the_peer_and_its_descriptors() {
    let identity_a = identity(34);
    let node = identity_a.node_id();
    let network = Arc::new(InMemoryNetwork::new());
    let (transport, inbox) = network.join(node.clone());
    let federation = Arc::new(FederationNode::new(identity_a, Arc::new(transport)));
    let task = federation.start(inbox);

    let provider =
        RemoteCapabilityProvider::new(node.clone(), federation, vec![descriptor("echo")]);

    assert_eq!(provider.id(), &remote_provider_id(&node));
    assert_eq!(provider.node(), &node);
    assert!(matches!(
        provider.kind(),
        ProviderKind::Remote { node: remote } if remote == node
    ));
    let described = provider.describe().await.expect("a catalogue");
    assert_eq!(described.len(), 1);
    assert_eq!(described[0].id, capability("echo"));

    task.abort();
}
