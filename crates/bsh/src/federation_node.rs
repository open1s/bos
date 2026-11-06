//! The app's federation node: assembled once and kept for the process.
//!
//! The node is only started when the settings enable it. Assembly is split
//! from transport so the tests can run the whole path over the in-memory
//! transport: identity file, persisted peers, router task.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use agent::capabilities::{catalogue_from_registry, NativeToolProvider};
use agent::capability::composition::{
    resolve, CapabilityRequirement, CompositionSpec, GrantRef, LocalScope, PeerCatalogue, PeerRef,
    RemotePolicy,
};
use agent::capability::transport::FederationTransport;
use agent::capability::{
    unix_ms, CapabilityProvider, Catalogue, CompositionSnapshot, NodeId, WorkspaceId,
    WorkspaceRuntime,
};
use agent::tools::ToolRegistry;
use bus::{Bus, BusConfig};
use federation::{
    FederationNode, NodeIdentity, PeerStore, RemoteCapabilityProvider, ZenohTransport,
};
use semver::{Version, VersionReq};

/// One running federation endpoint and the state that must outlive it.
pub(crate) struct FederationRuntime {
    node: Arc<FederationNode>,
    node_id: NodeId,
    /// What each trusted peer last published, refreshed in the background.
    peer_capabilities: Arc<RwLock<BTreeMap<NodeId, Catalogue>>>,
    /// The session and the router must outlive the handle; dropping either
    /// would leave a node that answers nothing.
    #[allow(dead_code)]
    bus: Option<Bus>,
    #[allow(dead_code)]
    task: tokio::task::JoinHandle<()>,
}

impl FederationRuntime {
    /// The node this process runs.
    pub(crate) fn node(&self) -> &Arc<FederationNode> {
        &self.node
    }

    /// The discovered capability catalogue of every trusted peer.
    pub(crate) fn peer_capabilities(&self) -> Arc<RwLock<BTreeMap<NodeId, Catalogue>>> {
        Arc::clone(&self.peer_capabilities)
    }

    /// The Node id this runtime answers to.
    pub(crate) fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// How many peers discovery has cached.
    pub(crate) fn discovered_peers(&self) -> usize {
        self.peer_capabilities
            .read()
            .map(|catalogues| catalogues.len())
            .unwrap_or(0)
    }

    /// How many remote capabilities those peers published.
    pub(crate) fn remote_capabilities(&self) -> usize {
        self.peer_capabilities
            .read()
            .map(|catalogues| {
                catalogues
                    .values()
                    .map(|catalogue| catalogue.len())
                    .sum::<usize>()
            })
            .unwrap_or(0)
    }

    /// How many delegations this node is currently performing or awaiting.
    ///
    /// The store is authoritative; a finished or cancelled delegation stops
    /// counting the moment its state changes, so the row cannot report work
    /// that already settled.
    pub(crate) fn active_delegations(&self) -> usize {
        self.node
            .delegations()
            .read()
            .map(|store| {
                store
                    .iter()
                    .filter(|record| record.state.is_active())
                    .count()
            })
            .unwrap_or(0)
    }

    /// One line for the startup log.
    ///
    /// Read-only and cheap: it reports the node id and what discovery has
    /// cached, and never composes a workspace, because this is called while a
    /// pane renders.
    pub(crate) fn status(&self) -> String {
        format!(
            "node {} discovered_peers={} remote_capabilities={}",
            self.node_id,
            self.discovered_peers(),
            self.remote_capabilities()
        )
    }

    /// Refresh every trusted peer's catalogue now.
    ///
    /// Startup runs one pass and then refreshes periodically; a caller that
    /// just enrolled a peer can ask for the pass immediately. Today that caller
    /// is the test suite; the UI and the CLI reach it through `status`.
    #[cfg(test)]
    pub(crate) async fn refresh_peers(&self) {
        refresh(&self.node, &self.peer_capabilities).await;
    }
}

/// A workspace composed from this node's tools and its trusted peers.
pub(crate) struct ComposedWorkspace {
    pub(crate) runtime: WorkspaceRuntime,
    pub(crate) snapshot: CompositionSnapshot,
    /// The capabilities that resolved to a peer.
    pub(crate) remote: Vec<agent::capability::CapabilityId>,
}

/// Compose the local catalogue with every discovered peer catalogue.
///
/// A capability id is bound once: a remote capability whose id is also local
/// is skipped rather than shadowed, because one workspace cannot dispatch one
/// id two ways. Everything else resolves to the first peer in the spec's peer
/// order that offers it, which is the order the cache sorts by node id.
pub(crate) fn compose(
    local: &ToolRegistry,
    federation: &FederationRuntime,
    workspace: WorkspaceId,
) -> Result<ComposedWorkspace, String> {
    let local_catalogue = catalogue_from_registry(local).map_err(|error| error.to_string())?;
    let cache = federation.peer_capabilities();
    let cache = cache
        .read()
        .map_err(|_| "the peer cache is poisoned".to_string())?;

    let mut spec = CompositionSpec::new(
        "compose:node",
        Version::new(1, 0, 0),
        "this node's tools and its trusted peers",
    );
    let mut scope = LocalScope::new(workspace);
    for id in local_catalogue.ids() {
        scope.admit(id.clone());
        spec = spec.with_requirement(CapabilityRequirement::new(id.clone(), VersionReq::STAR));
    }

    let mut remote_ids = std::collections::BTreeSet::new();
    let mut peers = Vec::new();
    for (node, catalogue) in cache.iter() {
        spec = spec.with_peer(
            PeerRef::new(node.clone()).with_grant(GrantRef::new(format!("peer:{node}"))),
        );
        for descriptor in catalogue.iter() {
            let id = descriptor.id.clone();
            if local_catalogue.get(&id).is_some() || !remote_ids.insert(id.clone()) {
                continue;
            }
            spec = spec.with_requirement(
                CapabilityRequirement::new(id, VersionReq::STAR).with_remote(RemotePolicy::Allowed),
            );
        }
        peers.push(PeerCatalogue::new(node.clone(), catalogue.clone()));
    }

    let resolved =
        resolve(&spec, &local_catalogue, &peers, &scope).map_err(|error| error.to_string())?;
    let mut providers: Vec<Arc<dyn CapabilityProvider>> =
        vec![Arc::new(NativeToolProvider::from_registry(local))];
    for peer in &peers {
        let descriptors: Vec<_> = peer.catalogue.iter().cloned().collect();
        providers.push(Arc::new(RemoteCapabilityProvider::new(
            peer.node.clone(),
            Arc::clone(federation.node()),
            descriptors,
        )));
    }
    let runtime = WorkspaceRuntime::instantiate(resolved.clone(), &providers)
        .map_err(|error| error.to_string())?;
    let snapshot = CompositionSnapshot::from_resolved(&resolved, None, unix_ms());
    let remote = resolved
        .capabilities
        .iter()
        .filter(|capability| capability.binding.is_remote())
        .map(|capability| capability.descriptor.id.clone())
        .collect();
    Ok(ComposedWorkspace {
        runtime,
        snapshot,
        remote,
    })
}
static RUNTIME: OnceLock<FederationRuntime> = OnceLock::new();

/// The node this process runs, when settings enabled one.
pub(crate) fn running() -> Option<&'static FederationRuntime> {
    RUNTIME.get()
}

/// Start the node once per process, when the settings enable it.
///
/// Returns whether a node is running. A failure is logged and the app keeps
/// working without federation: a node that cannot start must not stop the
/// window from opening.
pub(crate) fn start_from_settings(settings: &crate::settings::Settings) -> bool {
    let config = &settings.global.federation;
    if !config.enabled {
        return false;
    }
    if RUNTIME.get().is_some() {
        return true;
    }
    let dir = crate::federation::dir();
    let identity_path = config
        .identity_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("node.key"));
    let peers_path = config
        .peers_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("peers.json"));
    let bus_config = BusConfig {
        mode: config.mode.clone().unwrap_or_else(|| "peer".to_string()),
        connect: (!config.endpoints.is_empty()).then(|| config.endpoints.clone()),
        listen: (!config.listen.is_empty()).then(|| config.listen.clone()),
        peer: None,
    };
    let started = crate::extension::block_on_tokio(async move {
        build(identity_path, peers_path, bus_config).await
    });
    match started {
        Ok(runtime) => {
            let peers = runtime
                .node()
                .peers()
                .read()
                .map(|store| store.iter().count())
                .unwrap_or(0);
            log::info!("federation {} peers={peers}", runtime.status());
            let _ = RUNTIME.set(runtime);
            true
        }
        Err(error) => {
            log::warn!("federation node did not start: {error}");
            false
        }
    }
}

/// Offer the composed remote capabilities to the model for this turn.
///
/// Local tools keep their existing path; only capabilities bound to a peer are
/// registered, as tool shims that dispatch through the composed runtime. A name
/// the live registry already holds is skipped rather than replaced: the local
/// tool the reader already has wins.
pub(crate) fn attach_remote_tools(
    agent: &agent::Agent,
    federation: &FederationRuntime,
    workspace: WorkspaceId,
    principal: agent::capability::Principal,
) -> Result<usize, String> {
    use agent::capabilities::CapabilityTool;
    use agent::tools::AsyncTool as _;

    let registry = agent
        .registry()
        .ok_or("the agent has no tool registry")?
        .clone();
    let composed = compose(&registry, federation, workspace)?;
    log::debug!(
        "federation: composed {} with {} remote capabilities",
        composed.snapshot.composition,
        composed.remote.len()
    );
    let runtime = Arc::new(composed.runtime);
    let mut added = 0;
    for id in &composed.remote {
        let Some(tool) = CapabilityTool::new(Arc::clone(&runtime), id, principal.clone()) else {
            continue;
        };
        let name = tool.name().to_string();
        if registry.get(&name).is_some() || registry.get_async(&name).is_some() {
            log::debug!("federation: {name} is already a local tool; the local one stays");
            continue;
        }
        if agent.add_live_async_tool(Arc::new(tool)).is_err() {
            continue;
        }
        added += 1;
    }
    Ok(added)
}
async fn build(
    identity_path: PathBuf,
    peers_path: PathBuf,
    config: BusConfig,
) -> Result<FederationRuntime, String> {
    let identity =
        NodeIdentity::load_or_create(&identity_path).map_err(|error| error.to_string())?;
    let peers = PeerStore::load(&peers_path).map_err(|error| error.to_string())?;
    let bus = Bus::from(config).await;
    let transport = ZenohTransport::new(bus.session(), identity.node_id());
    let inbox = transport
        .subscribe()
        .await
        .map_err(|error| error.to_string())?;
    // The audit log sits beside the identity and the peer store: the same
    // federation directory, so it travels with the node's own facts.
    let audit = identity_path
        .parent()
        .map(|directory| directory.join("audit.jsonl"));
    if let Some(path) = &audit {
        // Once per launch, not per write: a crashed instance leaves its
        // per-process file behind, and seven days is long enough to read it.
        let _ = crate::federation_audit::prune_stale(path, std::time::SystemTime::now());
    }
    Ok(assemble(
        identity,
        peers,
        Arc::new(transport),
        inbox.into_receiver(),
        Some(bus),
        audit,
    ))
}

pub(crate) fn assemble(
    identity: NodeIdentity,
    peers: PeerStore,
    transport: Arc<dyn FederationTransport>,
    inbox: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    bus: Option<Bus>,
    audit: Option<PathBuf>,
) -> FederationRuntime {
    let mut node =
        FederationNode::new(identity, transport).with_peer_store(Arc::new(RwLock::new(peers)));
    if let Some(path) = audit {
        node = node.with_audit(Arc::new(crate::federation_audit::FileAudit::new(path)));
    }
    let node = Arc::new(node);
    let node_id = node.node();
    let task = node.start(inbox);
    let peer_capabilities = Arc::new(RwLock::new(BTreeMap::new()));
    {
        let node = Arc::clone(&node);
        let cache = Arc::clone(&peer_capabilities);
        tokio::spawn(async move {
            loop {
                refresh(&node, &cache).await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
    }
    FederationRuntime {
        node,
        node_id,
        peer_capabilities,
        bus,
        task,
    }
}

/// One discovery pass: ask every enrolled peer what it publishes.
async fn refresh(node: &Arc<FederationNode>, cache: &Arc<RwLock<BTreeMap<NodeId, Catalogue>>>) {
    let peers: Vec<NodeId> = node
        .peers()
        .read()
        .map(|store| {
            store
                .iter()
                .filter(|record| record.enrolled(agent::capability::unix_ms()))
                .map(|record| record.node.clone())
                .collect()
        })
        .unwrap_or_default();
    for peer in peers {
        match node.peer_catalogue(&peer).await {
            Ok(catalogue) => {
                let count = catalogue.len();
                if let Ok(mut cache) = cache.write() {
                    cache.insert(peer.clone(), catalogue);
                }
                log::debug!("federation peer {peer} offers {count} capabilities");
            }
            Err(error) => {
                log::debug!("federation peer {peer} did not answer discovery: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::capability::unix_ms;
    use federation::InMemoryNetwork;

    #[test]
    fn a_disabled_setting_starts_no_node() {
        let settings = crate::settings::Settings::default();
        assert!(!settings.global.federation.enabled, "federation is opt-in");
        assert!(!start_from_settings(&settings));
    }

    /// The whole federation path over two in-process nodes: discovery,
    /// composition, a remote call, a delegation, a bounded resource stream,
    /// and revocation cutting every plane at once.
    #[tokio::test]
    async fn two_nodes_share_capabilities_delegations_and_bytes_until_revoked() {
        use agent::capability::composition::{Binding, ResolvedCapability, ResolvedComposition};
        use agent::capability::{
            CapabilityCall, CapabilityDescriptor, CapabilityKind, CapabilityOutput,
            CapabilityProvider, InvocationContext, InvocationId, Principal, ProviderHealth,
            ProviderId, ProviderKind, VersionSource, Visibility, WorkspaceId, WorkspaceRuntime,
        };
        use agent::tools::{FunctionTool, ToolRegistry};
        use async_trait::async_trait;
        use federation::{
            DelegationId, DelegationRequest, DelegationState, InMemoryNetwork, LeaseId,
            ResourceLease, ResourceSource, ResourceStreamMessage,
        };
        use semver::Version;
        use serde_json::{json, Value};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Echo {
            descriptor: CapabilityDescriptor,
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl CapabilityProvider for Echo {
            fn id(&self) -> &ProviderId {
                static BUILTIN: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
                BUILTIN.get_or_init(ProviderId::builtin)
            }
            fn kind(&self) -> ProviderKind {
                ProviderKind::Builtin
            }
            async fn describe(
                &self,
            ) -> Result<Vec<CapabilityDescriptor>, agent::capability::CapabilityError> {
                Ok(vec![self.descriptor.clone()])
            }
            async fn invoke(
                &self,
                call: CapabilityCall<'_>,
            ) -> Result<CapabilityOutput, agent::capability::CapabilityError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(CapabilityOutput::json(json!({ "deployed": call.input })))
            }
            async fn health(&self) -> ProviderHealth {
                ProviderHealth::Available
            }
        }

        struct Bytes(Vec<u8>);

        #[async_trait]
        impl ResourceSource for Bytes {
            async fn read(
                &self,
                _uri: &str,
                offset: u64,
                max: usize,
            ) -> Result<Option<Vec<u8>>, agent::capability::CapabilityError> {
                let start = offset as usize;
                if start >= self.0.len() {
                    return Ok(None);
                }
                let end = (start + max).min(self.0.len());
                Ok(Some(self.0[start..end].to_vec()))
            }
        }

        let network = Arc::new(InMemoryNetwork::new());
        let provider_identity = NodeIdentity::from_seed([41u8; 32]);
        let provider_id = provider_identity.node_id();
        let descriptor = CapabilityDescriptor::new(
            "builtin/deploy".parse().expect("an id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        )
        .with_visibility(Visibility::Public);
        let calls = Arc::new(AtomicUsize::new(0));
        let echo: Arc<dyn CapabilityProvider> = Arc::new(Echo {
            descriptor: descriptor.clone(),
            calls: Arc::clone(&calls),
        });
        let provider_plan = ResolvedComposition {
            id: "compose:peer".into(),
            version: Version::new(1, 0, 0),
            workspace: WorkspaceId::new("ws:peer").expect("a workspace"),
            capabilities: vec![ResolvedCapability {
                descriptor: descriptor.clone(),
                binding: Binding::Local {
                    provider: ProviderId::builtin(),
                },
            }],
            skipped: Vec::new(),
        };
        let peer_runtime = Arc::new(
            WorkspaceRuntime::instantiate(provider_plan, &[echo]).expect("a peer runtime"),
        );
        let (provider_transport, provider_inbox) = network.join(provider_id.clone());
        let provider = FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_public_capabilities(vec![descriptor])
            .with_runtime(peer_runtime)
            .with_resource_source(Arc::new(Bytes(b"0123456789".to_vec())));
        let _provider_task = provider.start(provider_inbox);

        let identity = NodeIdentity::from_seed([42u8; 32]);
        let local_id = identity.node_id();
        let (transport, inbox) = network.join(local_id.clone());
        let runtime = assemble(
            identity,
            PeerStore::new(),
            Arc::new(transport),
            inbox,
            None,
            None,
        );

        let now = unix_ms();
        {
            let peers = runtime.node().peers();
            let mut peers = peers.write().expect("peers");
            peers.enroll(provider_id.clone(), now, None);
            for capability in ["builtin/deploy", "resource/fs.read"] {
                peers
                    .grant(&provider_id, capability.parse().expect("an id"), now, None)
                    .expect("granted");
            }
        }
        {
            let peers = provider.peers();
            let mut peers = peers.write().expect("peers");
            peers.enroll(local_id.clone(), now, None);
            for capability in ["builtin/deploy", "resource/fs.read"] {
                peers
                    .grant(&local_id, capability.parse().expect("an id"), now, None)
                    .expect("granted");
            }
        }
        runtime.refresh_peers().await;

        // Discovery fed the cache, and composition put the peer's capability in
        // the same workspace as a local tool.
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(FunctionTool::new(
                "local_echo",
                "Echo locally",
                json!({ "type": "object" }),
                |args: &Value| Ok(json!({ "local": args })),
            )))
            .expect("a unique tool");
        let composed = compose(
            &registry,
            &runtime,
            WorkspaceId::new("ws:e2e").expect("a workspace"),
        )
        .expect("the workspace composes");
        assert_eq!(composed.remote.len(), 1, "one remote capability");

        let context = InvocationContext::local(
            InvocationId::new("inv:e2e"),
            Principal::Local {
                session: "session".to_string(),
                preset: "preset".to_string(),
            },
            WorkspaceId::new("ws:e2e").expect("a workspace"),
        );
        let deployed = composed
            .runtime
            .invoke(
                &"builtin/deploy".parse().expect("an id"),
                json!({ "rev": 3 }),
                &context,
            )
            .await
            .expect("the peer deploys");
        assert_eq!(
            deployed.structured,
            Some(json!({ "deployed": { "rev": 3 } }))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The coordination plane: submit, read the authoritative state, cancel.
        let request = DelegationRequest {
            id: DelegationId::new("d-e2e"),
            parent: None,
            target: provider_id.clone(),
            objective: "verify the deployment".to_string(),
            capabilities: vec!["builtin/deploy".parse().expect("an id")],
            deadline_ms: None,
            allow_sub_delegation: false,
            result_contract: None,
        };
        let accepted = runtime
            .node()
            .delegate(&provider_id, request, &context)
            .await
            .expect("the peer accepts the task");
        assert_eq!(accepted.state, DelegationState::Accepted);
        let status = runtime
            .node()
            .delegation_status(&provider_id, &DelegationId::new("d-e2e"), &context)
            .await
            .expect("the owner reports");
        assert_eq!(status.state, DelegationState::Accepted);
        let cancelled = runtime
            .node()
            .cancel_delegation(&provider_id, &DelegationId::new("d-e2e"), &context)
            .await
            .expect("cancelled");
        assert_eq!(cancelled.state, DelegationState::Cancelled);

        // The data plane: a four-byte quota stops a ten-byte resource.
        let lease = ResourceLease {
            id: LeaseId::new("lease-e2e"),
            capability: "resource/fs.read".parse().expect("an id"),
            holder: local_id.clone(),
            granted_at_ms: now,
            expires_at_ms: None,
            max_bytes: Some(4),
            used_bytes: 0,
        };
        provider
            .leases()
            .write()
            .expect("leases")
            .grant(lease)
            .expect("granted");
        let mut stream = runtime
            .node()
            .open_resource(
                &provider_id,
                &LeaseId::new("lease-e2e"),
                "file:///artifact",
                0,
                64,
            )
            .await
            .expect("the stream opens");
        let mut bytes = Vec::new();
        loop {
            let message = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
                .await
                .expect("a stream message")
                .expect("the stream is open");
            match message {
                ResourceStreamMessage::Chunk { bytes: chunk, .. } => bytes.extend(chunk),
                ResourceStreamMessage::Error { message, .. } => {
                    assert!(message.contains("bytes left"), "{message}");
                    break;
                }
                other => panic!("unexpected stream message: {other:?}"),
            }
        }
        assert_eq!(bytes.len(), 4, "the quota bounds the stream");

        // Revocation cuts every plane: the capability call and the delegation
        // are both refused by the provider from now on.
        provider.peers().write().expect("peers").revoke(&local_id);
        let denied = composed
            .runtime
            .invoke(
                &"builtin/deploy".parse().expect("an id"),
                json!({}),
                &context,
            )
            .await
            .expect_err("revoked");
        assert!(
            matches!(denied, agent::capability::CapabilityError::Denied { ref reason, .. } if reason.contains("revoked")),
            "{denied:?}"
        );
        let request = DelegationRequest {
            id: DelegationId::new("d-after-revoke"),
            parent: None,
            target: provider_id.clone(),
            objective: "should be refused".to_string(),
            capabilities: vec!["builtin/deploy".parse().expect("an id")],
            deadline_ms: None,
            allow_sub_delegation: false,
            result_contract: None,
        };
        let refused = runtime
            .node()
            .delegate(&provider_id, request, &context)
            .await
            .expect_err("revoked");
        assert!(
            matches!(refused, agent::capability::CapabilityError::Denied { .. }),
            "{refused:?}"
        );
    }

    /// A frame that fails verification never reaches a handler: a forged
    /// payload, an expired message, and a future schema are all dropped, and
    /// the node keeps answering the peer that is entitled to an answer.
    #[tokio::test]
    async fn forged_expired_and_wrong_version_frames_are_dropped() {
        use agent::capability::{CapabilityDescriptor, CapabilityKind, VersionSource, Visibility};
        use federation::{Envelope, FederationMessage};
        use semver::Version;

        let network = Arc::new(InMemoryNetwork::new());
        let victim_identity = NodeIdentity::from_seed([51u8; 32]);
        let victim_id = victim_identity.node_id();
        let (victim_transport, victim_inbox) = network.join(victim_id.clone());
        let descriptor = CapabilityDescriptor::new(
            "builtin/read_file".parse().expect("an id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        )
        .with_visibility(Visibility::Public);
        let victim = FederationNode::new(victim_identity, Arc::new(victim_transport))
            .with_public_capabilities(vec![descriptor]);
        let victim_task = victim.start(victim_inbox);

        let attacker = NodeIdentity::from_seed([52u8; 32]);
        let (attacker_transport, _attacker_inbox) = network.join(attacker.node_id());
        let message = || FederationMessage::new("discover", b"{}".to_vec());

        // A payload mutated after signing.
        let mut forged = Envelope::seal(&message(), &attacker, &victim_id).expect("sealed");
        forged.payload = serde_json::json!({ "forged": true });
        attacker_transport
            .send(&victim_id, forged.encode().expect("encoded"))
            .await
            .expect("sent");

        // An expired message.
        let expired = message().with_expiry_ms(agent::capability::unix_ms().saturating_sub(1));
        let expired = Envelope::seal(&expired, &attacker, &victim_id).expect("sealed");
        attacker_transport
            .send(&victim_id, expired.encode().expect("encoded"))
            .await
            .expect("sent");

        // A schema this build does not speak.
        let mut future = Envelope::seal(&message(), &attacker, &victim_id).expect("sealed");
        future.schema_version = 2;
        attacker_transport
            .send(&victim_id, future.encode().expect("encoded"))
            .await
            .expect("sent");

        // The entitled peer still gets an answer.
        let caller_identity = NodeIdentity::from_seed([53u8; 32]);
        let caller_id = caller_identity.node_id();
        let (caller_transport, caller_inbox) = network.join(caller_id.clone());
        let caller = FederationNode::new(caller_identity, Arc::new(caller_transport));
        let caller_task = caller.start(caller_inbox);
        let now = agent::capability::unix_ms();
        victim
            .peers()
            .write()
            .expect("peers")
            .enroll(caller_id.clone(), now, None);
        caller
            .peers()
            .write()
            .expect("peers")
            .enroll(victim_id.clone(), now, None);

        let record = caller
            .discover(&victim_id)
            .await
            .expect("the node still answers");
        assert_eq!(record.node, victim_id);
        assert!(record.verify());

        victim_task.abort();
        caller_task.abort();
    }
    #[tokio::test]
    async fn the_runtime_discovers_a_trusted_peers_capabilities() {
        use agent::capability::{CapabilityDescriptor, CapabilityKind, VersionSource, Visibility};
        use semver::Version;

        let network = Arc::new(InMemoryNetwork::new());
        let provider_identity = NodeIdentity::from_seed([9u8; 32]);
        let provider_id = provider_identity.node_id();
        let (provider_transport, provider_inbox) = network.join(provider_id.clone());
        let descriptor = CapabilityDescriptor::new(
            "builtin/read_file".parse().expect("a valid id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        )
        .with_visibility(Visibility::Public);
        let provider = FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_public_capabilities(vec![descriptor]);
        let _provider_task = provider.start(provider_inbox);

        let identity = NodeIdentity::from_seed([10u8; 32]);
        let local_id = identity.node_id();
        let (transport, inbox) = network.join(local_id.clone());
        let runtime = assemble(
            identity,
            PeerStore::new(),
            Arc::new(transport),
            inbox,
            None,
            None,
        );

        let now = unix_ms();
        runtime
            .node()
            .peers()
            .write()
            .expect("peers")
            .enroll(provider_id.clone(), now, None);
        provider
            .peers()
            .write()
            .expect("peers")
            .enroll(local_id, now, None);

        runtime.refresh_peers().await;

        let cache = runtime.peer_capabilities();
        let cache = cache.read().expect("the cache");
        let catalogue = cache.get(&provider_id).expect("the peer was discovered");
        assert_eq!(catalogue.len(), 1);
        assert!(catalogue
            .get(&"builtin/read_file".parse().expect("an id"))
            .is_some());
        assert!(
            runtime.status().contains("discovered_peers=1"),
            "{}",
            runtime.status()
        );
    }

    #[tokio::test]
    async fn a_workspace_composes_a_local_tool_and_a_remote_capability() {
        use agent::capability::composition::{Binding, ResolvedCapability, ResolvedComposition};
        use agent::capability::{
            CapabilityCall, CapabilityDescriptor, CapabilityKind, CapabilityOutput,
            InvocationContext, InvocationId, Principal, ProviderHealth, ProviderId, ProviderKind,
            VersionSource, Visibility,
        };
        use agent::tools::{FunctionTool, ToolRegistry};
        use async_trait::async_trait;
        use serde_json::{json, Value};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Echo {
            descriptor: CapabilityDescriptor,
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl CapabilityProvider for Echo {
            fn id(&self) -> &ProviderId {
                static BUILTIN: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
                BUILTIN.get_or_init(ProviderId::builtin)
            }
            fn kind(&self) -> ProviderKind {
                ProviderKind::Builtin
            }
            async fn describe(
                &self,
            ) -> Result<Vec<CapabilityDescriptor>, agent::capability::CapabilityError> {
                Ok(vec![self.descriptor.clone()])
            }
            async fn invoke(
                &self,
                call: CapabilityCall<'_>,
            ) -> Result<CapabilityOutput, agent::capability::CapabilityError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(CapabilityOutput::json(json!({ "remote": call.input })))
            }
            async fn health(&self) -> ProviderHealth {
                ProviderHealth::Available
            }
        }

        let network = Arc::new(InMemoryNetwork::new());
        let provider_identity = NodeIdentity::from_seed([21u8; 32]);
        let provider_id = provider_identity.node_id();
        let descriptor = CapabilityDescriptor::new(
            "builtin/remote_echo".parse().expect("an id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        )
        .with_visibility(Visibility::Public);
        let calls = Arc::new(AtomicUsize::new(0));
        let provider: Arc<dyn CapabilityProvider> = Arc::new(Echo {
            descriptor: descriptor.clone(),
            calls: Arc::clone(&calls),
        });
        let provider_plan = ResolvedComposition {
            id: "compose:peer".into(),
            version: Version::new(1, 0, 0),
            workspace: WorkspaceId::new("ws:peer").expect("a workspace"),
            capabilities: vec![ResolvedCapability {
                descriptor: descriptor.clone(),
                binding: Binding::Local {
                    provider: ProviderId::builtin(),
                },
            }],
            skipped: Vec::new(),
        };
        let provider_runtime = Arc::new(
            WorkspaceRuntime::instantiate(provider_plan, &[provider]).expect("a peer runtime"),
        );
        let (provider_transport, provider_inbox) = network.join(provider_id.clone());
        let provider_node = FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_public_capabilities(vec![descriptor])
            .with_runtime(provider_runtime);
        let _provider_task = provider_node.start(provider_inbox);

        let identity = NodeIdentity::from_seed([22u8; 32]);
        let local_id = identity.node_id();
        let (transport, inbox) = network.join(local_id.clone());
        let runtime = assemble(
            identity,
            PeerStore::new(),
            Arc::new(transport),
            inbox,
            None,
            None,
        );

        let now = unix_ms();
        {
            let peers = runtime.node().peers();
            let mut peers = peers.write().expect("peers");
            peers.enroll(provider_id.clone(), now, None);
            peers
                .grant(
                    &provider_id,
                    "builtin/remote_echo".parse().expect("an id"),
                    now,
                    None,
                )
                .expect("granted");
        }
        {
            let peers = provider_node.peers();
            let mut peers = peers.write().expect("peers");
            peers.enroll(local_id.clone(), now, None);
            peers
                .grant(
                    &local_id,
                    "builtin/remote_echo".parse().expect("an id"),
                    now,
                    None,
                )
                .expect("granted");
        }
        runtime.refresh_peers().await;

        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(FunctionTool::new(
                "local_echo",
                "Echo locally",
                json!({ "type": "object" }),
                |args: &Value| Ok(json!({ "local": args })),
            )))
            .expect("a unique tool");
        let composed = compose(
            &registry,
            &runtime,
            WorkspaceId::new("ws:composed").expect("a workspace"),
        )
        .expect("the workspace composes");

        let context = InvocationContext::local(
            InvocationId::new("inv:composed"),
            Principal::Local {
                session: "session".to_string(),
                preset: "preset".to_string(),
            },
            WorkspaceId::new("ws:composed").expect("a workspace"),
        );
        let local = composed
            .runtime
            .invoke(
                &"builtin/local_echo".parse().expect("an id"),
                json!({ "v": 1 }),
                &context,
            )
            .await
            .expect("the local tool runs");
        assert_eq!(local.structured, Some(json!({ "local": { "v": 1 } })));

        let remote = composed
            .runtime
            .invoke(
                &"builtin/remote_echo".parse().expect("an id"),
                json!({ "v": 2 }),
                &context,
            )
            .await
            .expect("the peer runs it");
        assert_eq!(remote.structured, Some(json!({ "remote": { "v": 2 } })));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        assert_eq!(composed.snapshot.capabilities.len(), 2);
        assert_eq!(composed.snapshot.remote_capabilities().count(), 1);
    }
    #[tokio::test]
    async fn the_assembled_node_answers_a_trusted_peer() {
        let network = Arc::new(InMemoryNetwork::new());
        let identity = NodeIdentity::from_seed([3u8; 32]);
        let node_id = identity.node_id();
        let (transport, inbox) = network.join(node_id.clone());
        let runtime = assemble(
            identity,
            PeerStore::new(),
            Arc::new(transport),
            inbox,
            None,
            None,
        );
        assert_eq!(runtime.node().node(), node_id);
        assert!(!runtime.task.is_finished(), "the router is running");

        let caller_identity = NodeIdentity::from_seed([4u8; 32]);
        let caller_id = caller_identity.node_id();
        let (caller_transport, caller_inbox) = network.join(caller_id.clone());
        let caller = Arc::new(FederationNode::new(
            caller_identity,
            Arc::new(caller_transport),
        ));
        let _caller_task = caller.start(caller_inbox);

        let now = unix_ms();
        runtime
            .node()
            .peers()
            .write()
            .expect("peers")
            .enroll(caller_id.clone(), now, None);
        caller
            .peers()
            .write()
            .expect("peers")
            .enroll(node_id.clone(), now, None);

        let record = caller
            .discover(&node_id)
            .await
            .expect("the assembled node answers");

        assert_eq!(record.node, node_id);
        assert!(record.verify());
    }
}
