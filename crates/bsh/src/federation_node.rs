//! The app's federation node: assembled once and kept for the process.
//!
//! The node is only started when the settings enable it. Assembly is split
//! from transport so the tests can run the whole path over the in-memory
//! transport: identity file, persisted peers, router task.

use std::collections::BTreeMap;
use std::net::ToSocketAddrs as _;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use agent::capabilities::{catalogue_from_registry, NativeToolProvider};
use agent::capability::composition::{
    resolve_host, CapabilityRequirement, CompositionSpec, GrantRef, LocalScope, PeerCatalogue,
    PeerRef, RemotePolicy,
};
use agent::capability::transport::FederationTransport;
use agent::capability::{
    unix_ms, CapabilityProvider, Catalogue, CompositionSnapshot, NodeId, WorkspaceId,
    WorkspaceRuntime,
};
use agent::tools::ToolRegistry;
use bus::{Bus, BusConfig};
use federation::{
    DiscoveryRecord, Envelope, FederationMessage, FederationNode, NodeIdentity, PeerStore,
    RemoteCapabilityProvider, RotationRecord, ZenohTransport,
};
use semver::{Version, VersionReq};

/// How often a node re-announces presence and public descriptors.
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(10);

/// The TLS endpoint peers connect to while Zenoh listens on loopback.
static TLS_TERMINATOR: OnceLock<std::sync::Mutex<Option<bus::tls_terminator::TlsTerminator>>> =
    OnceLock::new();

static TLS_CLIENTS: OnceLock<std::sync::Mutex<Vec<bus::tls_client::TlsClientTerminator>>> =
    OnceLock::new();

fn client_slot() -> &'static std::sync::Mutex<Vec<bus::tls_client::TlsClientTerminator>> {
    TLS_CLIENTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn terminator_slot() -> &'static std::sync::Mutex<Option<bus::tls_terminator::TlsTerminator>> {
    TLS_TERMINATOR.get_or_init(|| std::sync::Mutex::new(None))
}

/// Start the public TLS terminator and move Zenoh's listener to loopback.
async fn start_tls_terminator(
    config: &mut BusConfig,
    settings: &crate::settings::Settings,
) -> Option<bus::tls_terminator::TlsTerminator> {
    let tls = config.tls.as_ref()?;
    let listen = config.listen.as_ref()?.first()?.strip_prefix("tls/")?;
    let public = listen.parse::<std::net::SocketAddr>().ok()?;
    let certificate = tls.certificate.as_ref().map(PathBuf::from)?;
    let private_key = tls.private_key.as_ref().map(PathBuf::from)?;
    let probe = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    let backend_port = probe.local_addr().ok()?.port();
    drop(probe);
    let backend = format!("127.0.0.1:{backend_port}")
        .parse()
        .expect("a loopback address");
    let terminator =
        bus::tls_terminator::TlsTerminator::start(bus::tls_terminator::TerminatorConfig {
            listen: public,
            backend,
            certificate,
            private_key,
            client_ca: tls.root_ca_certificate.as_ref().map(PathBuf::from),
            require_client_certificate: tls.enable_mtls.unwrap_or(false),
        })
        .await
        .map_err(|error| log::warn!("federation: TLS terminator did not start: {error}"))
        .ok()?;
    config.listen = Some(vec![format!("tcp/127.0.0.1:{backend_port}")]);

    // Outbound TLS has the same lifecycle problem. Keep Zenoh on loopback and
    // let a reloadable client proxy own the handshake and its client cert.
    if let Some(connect) = config.connect.take() {
        let mut endpoints = Vec::with_capacity(connect.len());
        let mut clients = Vec::new();
        for endpoint in connect {
            let Some(remote) = endpoint.strip_prefix("tls/") else {
                endpoints.push(endpoint);
                continue;
            };
            let Some((host, port)) = remote.rsplit_once(':') else {
                endpoints.push(endpoint.clone());
                continue;
            };
            let Ok(port) = port.parse::<u16>() else {
                endpoints.push(endpoint.clone());
                continue;
            };
            let Ok(remote) = (host, port).to_socket_addrs().map(|mut addrs| {
                addrs
                    .next()
                    .unwrap_or_else(|| std::net::SocketAddr::from(([127, 0, 0, 1], port)))
            }) else {
                endpoints.push(endpoint.clone());
                continue;
            };
            let Some(root_ca) = tls.root_ca_certificate.as_ref() else {
                endpoints.push(endpoint.clone());
                continue;
            };
            let proxy = bus::tls_client::TlsClientTerminator::start(
                bus::tls_client::ClientTerminatorConfig {
                    listen: "127.0.0.1:0".parse().expect("a loopback address"),
                    remote,
                    server_name: host.to_string(),
                    root_ca: PathBuf::from(root_ca),
                    certificate: tls.certificate.as_ref().map(PathBuf::from),
                    private_key: tls.private_key.as_ref().map(PathBuf::from),
                },
            )
            .await;
            match proxy {
                Ok(proxy) => {
                    endpoints.push(format!("tcp/127.0.0.1:{}", proxy.local_addr().port()));
                    clients.push(proxy);
                }
                Err(error) => {
                    log::warn!(
                        "federation: TLS client proxy for {endpoint} did not start: {error}"
                    );
                    endpoints.push(endpoint);
                }
            }
        }
        config.connect = Some(endpoints);
        if let Ok(mut slot) = client_slot().lock() {
            for proxy in slot.drain(..) {
                proxy.abort();
            }
            *slot = clients;
        }
    }
    let _ = settings;
    Some(terminator)
}

/// How long a discovered record stays listed without a fresh announcement.
const DISCOVERY_STALE_MS: u64 = 30_000;

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
    /// The 30-second discovery refresher, stopped with the node.
    #[allow(dead_code)]
    refresher: tokio::task::JoinHandle<()>,
    /// The announcement task: presence plus public descriptors, every interval.
    #[allow(dead_code)]
    announce_task: tokio::task::JoinHandle<()>,
    /// Retries failed remote compensations from compensation.json.
    #[allow(dead_code)]
    reconciler: Option<tokio::task::JoinHandle<()>>,
    /// Nodes seen on the announce key, excluding this one.
    discovered: Arc<RwLock<BTreeMap<NodeId, DiscoveryRecord>>>,
    /// This node's signing identity, kept for an on-demand announcement.
    identity: NodeIdentity,
    /// The transport announcements leave through.
    announce_transport: Arc<dyn FederationTransport>,
    /// Public descriptors this node announces.
    advertise: Vec<agent::capability::CapabilityDescriptor>,
    /// The name this node announces; an edit takes effect on the next frame.
    node_name: Arc<RwLock<Option<String>>>,
    /// The federation this runtime advertises, when configured.
    federation: Option<federation::FederationId>,
    /// The policy revision announced with discovery records.
    policy_revision: u64,
    /// The data directory's writer lock, held for the process's lifetime.
    #[allow(dead_code)]
    lock: Option<crate::federation_lock::NodeLocks>,
}

impl FederationRuntime {
    /// The node this process runs.
    pub(crate) fn node(&self) -> &Arc<FederationNode> {
        &self.node
    }

    /// The federation advertised by this runtime, when configured.
    pub(crate) fn federation(&self) -> Option<&federation::FederationId> {
        self.federation.as_ref()
    }

    /// The discovered capability catalogue of every trusted peer.
    pub(crate) fn peer_capabilities(&self) -> Arc<RwLock<BTreeMap<NodeId, Catalogue>>> {
        Arc::clone(&self.peer_capabilities)
    }

    /// The Node id this runtime answers to.
    pub(crate) fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// The name this node shows to peers, when it has one.
    pub(crate) fn node_name(&self) -> Option<String> {
        self.node_name
            .read()
            .map(|name| name.clone())
            .unwrap_or_default()
    }

    /// Rename this node; the next announcement carries the new name.
    pub(crate) fn set_name(&self, name: Option<String>) {
        if let Ok(mut slot) = self.node_name.write() {
            *slot = name;
        }
    }

    /// Nodes seen on the announce key, excluding this one.
    pub(crate) fn announced_peers(&self) -> Vec<DiscoveryRecord> {
        self.discovered
            .read()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Announce now on a worker, for a reader-triggered refresh.
    pub(crate) fn reannounce(&self) {
        let identity = self.identity.clone();
        let transport = Arc::clone(&self.announce_transport);
        let own = self.node_id.clone();
        let advertise = self.advertise.clone();
        let name = self.node_name();
        let federation = self.federation.clone();
        let policy_revision = self.policy_revision;
        std::thread::spawn(move || {
            crate::extension::block_on_tokio(async move {
                announce_once(
                    &identity,
                    &transport,
                    &own,
                    &advertise,
                    name.as_deref(),
                    federation.as_ref(),
                    policy_revision,
                )
                .await;
            });
        });
    }

    /// Announce immediately, for a test or a caller already on a runtime.
    #[cfg(test)]
    pub(crate) async fn announce_now(&self) {
        let name = self.node_name();
        announce_once(
            &self.identity,
            &self.announce_transport,
            &self.node_id,
            &self.advertise,
            name.as_deref(),
            self.federation.as_ref(),
            self.policy_revision,
        )
        .await;
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

    /// One line for the startup log and the headless node.
    ///
    /// Read-only and cheap: it reports the node id and what discovery has
    /// cached, and never composes a workspace, because this is called while a
    /// pane renders.
    ///
    /// `peers` is PRESENCE: nodes whose announcement has been seen and has not
    /// expired. `catalogues` is how many of those published capabilities this
    /// node has fetched - a linked pair of default nodes has one peer and no
    /// catalogues, and reporting only the catalogue count made a healthy link
    /// read as "0 peers".
    pub(crate) fn status(&self) -> String {
        format!(
            "node {} peers={} catalogues={} remote_capabilities={}",
            self.node_id,
            self.announced_peers().len(),
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
        resolve_host(&spec, &local_catalogue, &peers, &scope).map_err(|error| error.to_string())?;
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
/// Every node this process runs, by profile.
///
/// One process may host several nodes (profiles) at once: each has its own
/// identity, peer store, locks and transport session. The app starts the
/// profile it launched with; `bsh federation serve --profile a --profile b`
/// starts several headless nodes in one process.
static RUNTIMES: OnceLock<std::sync::RwLock<BTreeMap<String, Arc<FederationRuntime>>>> =
    OnceLock::new();

/// The profile the app presents. A headless process may have none, in which
/// case its single node is the answer.
static PRIMARY: OnceLock<String> = OnceLock::new();

fn nodes() -> &'static std::sync::RwLock<BTreeMap<String, Arc<FederationRuntime>>> {
    RUNTIMES.get_or_init(|| std::sync::RwLock::new(BTreeMap::new()))
}

/// The node this process presents: the profile it launched with, or the only
/// node it runs.
pub(crate) fn running() -> Option<Arc<FederationRuntime>> {
    let nodes = nodes().read().ok()?;
    if let Some(profile) = PRIMARY.get() {
        return nodes.get(profile).cloned();
    }
    if nodes.len() == 1 {
        return nodes.values().next().cloned();
    }
    None
}

/// One profile's node, when this process runs it.
pub(crate) fn running_profile(profile: &str) -> Option<Arc<FederationRuntime>> {
    nodes().read().ok()?.get(profile).cloned()
}

/// Every node this process runs, by profile.
pub(crate) fn running_profiles() -> Vec<(String, Arc<FederationRuntime>)> {
    nodes()
        .read()
        .map(|nodes| {
            nodes
                .iter()
                .map(|(profile, runtime)| (profile.clone(), Arc::clone(runtime)))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the settings ask for a TLS transport.
fn has_tls(config: &crate::settings::global::FederationSection) -> bool {
    config.tls_ca.is_some()
        || config.tls_cert.is_some()
        || config.tls_key.is_some()
        || config.tls_mtls
}

/// Replace the running listener so it rereads the configured TLS material.
///
/// Zenoh builds its TLS `ServerConfig` when a listener starts, so replacing
/// certificate files cannot affect a live listener. This stops the node and
/// starts it again from the current settings; established sessions reconnect,
/// which is a supervised restart rather than a zero-interruption handoff.
/// Returns whether a node is running afterwards.
pub(crate) fn reload_tls(settings: &crate::settings::Settings) -> bool {
    if let Ok(slot) = terminator_slot().lock() {
        if let Some(terminator) = slot.as_ref() {
            match terminator.reload() {
                Ok(()) => {
                    if let Ok(clients) = client_slot().lock() {
                        for client in clients.iter() {
                            if let Err(error) = client.reload() {
                                log::warn!("federation: TLS client reload failed: {error}");
                            }
                        }
                    }
                    log::info!("federation: TLS certificate reloaded without dropping sessions");
                    return running().is_some();
                }
                Err(error) => {
                    log::warn!("federation: TLS reload failed, restarting listener: {error}");
                }
            }
        }
    }
    let was_running = running().is_some();
    if was_running {
        stop();
    }
    if !settings.global.federation.enabled {
        return false;
    }
    start_from_settings(settings)
}

/// Stop the node and release its data directory lock.
///
/// The peer store is on disk, so a stop loses no state; the next start (or the
/// next launch) loads it again. Returns whether a node was running.
pub(crate) fn stop() -> bool {
    let profile = PRIMARY.get().cloned().or_else(|| {
        nodes()
            .read()
            .ok()
            .and_then(|nodes| nodes.keys().next().cloned())
    });
    match profile {
        Some(profile) => stop_profile(&profile),
        None => false,
    }
}

/// Stop one profile's node and release its directory locks.
pub(crate) fn stop_profile(profile: &str) -> bool {
    let taken = nodes()
        .write()
        .ok()
        .and_then(|mut nodes| nodes.remove(profile));
    let Some(runtime) = taken else {
        return false;
    };
    runtime.task.abort();
    runtime.refresher.abort();
    runtime.announce_task.abort();
    if let Some(reconciler) = runtime.reconciler.as_ref() {
        reconciler.abort();
    }
    // The TLS terminators are process-wide, so they are stopped only when no
    // node is left: a plaintext profile may still be serving beside the one
    // that owned them.
    let empty = nodes().read().map(|nodes| nodes.is_empty()).unwrap_or(true);
    if empty {
        if let Ok(mut slot) = terminator_slot().lock() {
            if let Some(terminator) = slot.take() {
                terminator.abort();
            }
        }
        if let Ok(mut clients) = client_slot().lock() {
            for client in clients.drain(..) {
                client.abort();
            }
        }
    }
    log::info!("federation node stopped (profile {profile})");
    true
}

/// Publish one signed announcement over `transport`.
async fn announce_once(
    identity: &NodeIdentity,
    transport: &Arc<dyn FederationTransport>,
    own: &NodeId,
    advertise: &[agent::capability::CapabilityDescriptor],
    name: Option<&str>,
    federation: Option<&federation::FederationId>,
    policy_revision: u64,
) {
    let record = DiscoveryRecord::publish_federated(
        identity,
        name.map(str::to_string),
        federation.cloned(),
        policy_revision,
        advertise.to_vec(),
    );
    let Ok(payload) = serde_json::to_vec(&record) else {
        return;
    };
    let message = FederationMessage::new("announce", payload);
    let Ok(envelope) = Envelope::seal(&message, identity, own) else {
        return;
    };
    if let Ok(frame) = envelope.encode() {
        let _ = transport.announce(frame).await;
    }
}

/// Apply one announcement frame. Returns whether the map changed.
///
/// The announcement is subject to the same envelope checks as any other frame:
/// protocol, schema, signature, and expiry. On top of that the record must
/// carry its own signature, must name the envelope's sender, and must not be
/// this node. A failure is a silently ignored frame, never a state change.
pub(crate) fn apply_announce(
    discovered: &mut BTreeMap<NodeId, DiscoveryRecord>,
    own: &NodeId,
    frame: &[u8],
    now_ms: u64,
) -> bool {
    let Ok(envelope) = Envelope::decode(frame) else {
        return false;
    };
    if envelope.kind != "announce" || envelope.open(now_ms).is_err() {
        return false;
    }
    let Ok(record) = serde_json::from_value::<DiscoveryRecord>(envelope.payload.clone()) else {
        return false;
    };
    if record.node != envelope.sender || &record.node == own {
        return false;
    }
    let known = discovered
        .get(&record.node)
        .map(|current| current.published_at_ms);
    if record.check(now_ms, known).is_err() {
        return false;
    }
    discovered.insert(record.node.clone(), record);
    true
}

/// Start the node once per process, when the settings enable it.
///
/// Returns whether a node is running. A failure is logged and the app keeps
/// working without federation: a node that cannot start must not stop the
/// window from opening.
pub(crate) fn start_from_settings(settings: &crate::settings::Settings) -> bool {
    let profile = crate::federation::resolve_profile(Some(&settings.global.federation.profile));
    start_profile(settings, &profile, true)
}

/// Start one profile's node; `primary` names the one the window presents.
pub(crate) fn start_profile(
    settings: &crate::settings::Settings,
    profile: &str,
    primary: bool,
) -> bool {
    let config = &settings.global.federation;
    if !config.enabled {
        return false;
    }
    // Production fails closed: an insecure transport never reaches the bus.
    if let Err(reason) = config.validate_transport() {
        log::error!("federation: refusing to start profile {profile}: {reason}");
        return false;
    }
    if running_profile(profile).is_some() {
        return true;
    }
    // One TLS terminator exists per process, and every profile shares this
    // settings document, so a second node while TLS is on would replace the
    // first one's listener. One TLS node per process is the honest limit.
    if has_tls(config)
        && nodes()
            .read()
            .map(|nodes| !nodes.is_empty())
            .unwrap_or(false)
    {
        log::warn!(
            "federation: profile {profile} asks for TLS, but the TLS transport is process-wide; run one TLS node per process"
        );
        return false;
    }
    // One profile is one node. The default profile keeps the old layout, so an
    // existing node is not moved; every other profile is its own directory and
    // therefore its own key, peer store, delegations and audit log.
    let dir = crate::federation::node_dir_for(profile);
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
    let tls = has_tls(config).then(|| bus::TlsConfig {
        root_ca_certificate: config.tls_ca.clone(),
        certificate: config.tls_cert.clone(),
        private_key: config.tls_key.clone(),
        enable_mtls: Some(config.tls_mtls),
        verify_name_on_connect: Some(config.tls_verify_name.unwrap_or(true)),
    });
    let scouting = tls.is_none();
    let bus_config = BusConfig {
        mode: config.mode.clone().unwrap_or_else(|| "peer".to_string()),
        connect: (!config.endpoints.is_empty()).then(|| config.endpoints.clone()),
        listen: (!config.listen.is_empty()).then(|| config.listen.clone()),
        peer: None,
        tls,
        scouting,
    };
    let settings_for_build = settings.clone();
    let started = crate::extension::block_on_tokio(async move {
        build(settings_for_build, identity_path, peers_path, bus_config).await
    });
    match started {
        Ok(runtime) => {
            let peers = runtime
                .node()
                .peers()
                .read()
                .map(|store| store.iter().count())
                .unwrap_or(0);
            log::info!(
                "federation {} profile={profile} peers={peers}",
                runtime.status()
            );
            if let Ok(mut nodes) = nodes().write() {
                nodes.insert(profile.to_string(), Arc::new(runtime));
            }
            if primary {
                let _ = PRIMARY.set(profile.to_string());
            }
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

/// Build the node's exposed capability runtime, if any are configured.
///
/// Only the read-only file tools are accepted: a node that answers strangers
/// must not be handed the shell. The tools are confined to `expose_root`, and
/// the exposed descriptors are made Public so discovery can publish them.
pub(crate) fn node_runtime(
    settings: &crate::settings::Settings,
) -> Result<
    (
        Arc<WorkspaceRuntime>,
        Vec<agent::capability::CapabilityDescriptor>,
    ),
    String,
> {
    use agent::capability::{CapabilityDescriptor, Visibility};
    use agent::tools::{DeclaredTool, ToolRegistry};

    let config = &settings.global.federation;
    if config.expose.is_empty() {
        return Err("no capabilities are exposed".to_string());
    }
    let root = config
        .expose_root
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| "expose_root is required when expose is set".to_string())?;
    if !root.is_dir() {
        return Err(format!("expose_root {} is not a directory", root.display()));
    }
    // The file tools compare a canonicalized target against this root, so the
    // root itself must be canonical (on macOS /var is a symlink to /private).
    let root = std::fs::canonicalize(&root).map_err(|error| {
        format!(
            "expose_root {} could not be resolved: {error}",
            root.display()
        )
    })?;

    let eff = settings.effective();
    let policy = crate::caps::policy_for_effective(settings, &eff);
    let session = "federation:node".to_string();
    let preset = eff.preset_id.clone();
    let mut registry = ToolRegistry::new();
    for raw in &config.expose {
        let id: agent::capability::CapabilityId = raw
            .parse()
            .map_err(|error| format!("invalid exposed capability id {raw:?}: {error}"))?;
        let tool: Arc<dyn agent::tools::Tool> = match id.name() {
            "glob" => Arc::new(DeclaredTool::new(
                Arc::new(crate::caps::glob_tool(
                    Some(root.clone()),
                    true,
                    Arc::clone(&policy),
                    session.clone(),
                    preset.clone(),
                )),
                crate::tool_metadata::declared("glob"),
            )),
            "grep" => Arc::new(DeclaredTool::new(
                Arc::new(crate::caps::grep_tool(
                    Some(root.clone()),
                    true,
                    Arc::clone(&policy),
                    session.clone(),
                    preset.clone(),
                )),
                crate::tool_metadata::declared("grep"),
            )),
            "read_file" => Arc::new(DeclaredTool::new(
                Arc::new(crate::caps::read_file_tool(
                    Some(root.clone()),
                    true,
                    Arc::clone(&policy),
                    session.clone(),
                    preset.clone(),
                )),
                crate::tool_metadata::declared("read_file"),
            )),
            "read_files" => Arc::new(DeclaredTool::new(
                Arc::new(crate::caps::read_files_tool(
                    Some(root.clone()),
                    true,
                    Arc::clone(&policy),
                    session.clone(),
                    preset.clone(),
                )),
                crate::tool_metadata::declared("read_files"),
            )),
            other => {
                return Err(format!(
                    "{other} is not a capability this node can expose; use read_file, read_files, glob, or grep"
                ));
            }
        };
        registry.register(tool).map_err(|error| error.to_string())?;
    }

    let catalogue = catalogue_from_registry(&registry).map_err(|error| error.to_string())?;
    let workspace = WorkspaceId::new("ws:federation-node").map_err(|error| error.to_string())?;
    let mut scope = LocalScope::new(workspace);
    let mut spec = CompositionSpec::new(
        "compose:federation-node",
        Version::new(1, 0, 0),
        "the capabilities this node exposes",
    );
    for id in catalogue.ids() {
        scope.admit(id.clone());
        spec = spec.with_requirement(CapabilityRequirement::new(id.clone(), VersionReq::STAR));
    }
    let plan = resolve_host(&spec, &catalogue, &[], &scope).map_err(|error| error.to_string())?;
    let provider: Arc<dyn CapabilityProvider> =
        Arc::new(NativeToolProvider::new(Arc::new(registry)));
    let runtime = Arc::new(
        WorkspaceRuntime::instantiate(plan.clone(), &[provider])
            .map_err(|error| error.to_string())?,
    );
    let descriptors: Vec<CapabilityDescriptor> = plan
        .capabilities
        .iter()
        .map(|capability| {
            let mut descriptor = capability.descriptor.clone();
            descriptor.visibility = Visibility::Public;
            descriptor
        })
        .collect();
    Ok((runtime, descriptors))
}

/// The delegation file beside the peer store, in the node's data directory.
fn delegation_store_path(peers_path: &std::path::Path) -> PathBuf {
    peers_path
        .parent()
        .map(|dir| dir.join("delegations.json"))
        .unwrap_or_else(|| PathBuf::from("delegations.json"))
}

/// The endpoint a node listens on when the settings name none.
///
/// A fixed local port is what makes a second node on the same machine
/// connectable without configuration: the first node owns it and the second
/// links to it. LAN peers still reach the first node through scouting; a
/// deployment that needs another address sets `listen`.
const DEFAULT_FEDERATION_ENDPOINT: &str = "tcp/[::]:7447";

/// Where the second node on this machine links to the first.
const LOCAL_FEDERATION_ENDPOINT: &str = "tcp/127.0.0.1:7447";

/// Open the federation transport, falling back to a loopback ephemeral port
/// when the default listen endpoint is taken.
///
/// Two nodes on one machine cannot share one fixed listen port. The second node
/// taking a loopback port keeps local scouting working - the port it actually
/// bound is what scouting advertises - so a pair of local nodes find each other
/// with no configuration.
async fn open_bus(mut config: BusConfig, explicit_listen: bool) -> Result<Bus, String> {
    if config.listen.is_none() {
        config.listen = Some(vec![DEFAULT_FEDERATION_ENDPOINT.to_string()]);
    }
    match Bus::try_from(config.clone()).await {
        Ok(bus) => Ok(bus),
        Err(first) => {
            // The reader named the endpoint, so a refused bind is their fact
            // to see: do not silently listen somewhere else.
            if explicit_listen {
                return Err(format!("the federation transport did not open: {first}"));
            }
            let port = free_loopback_port().map_err(|error| {
                format!("the federation transport did not open: {first}; no fallback port: {error}")
            })?;
            let mut fallback = config;
            fallback.listen = Some(vec![format!("tcp/127.0.0.1:{port}")]);
            // The node that owns the default port is a local peer, so link to
            // it directly instead of waiting for scouting to find an ephemeral
            // port. One link is enough: peers exchange their other locators.
            let mut connect = fallback.connect.take().unwrap_or_default();
            if !connect
                .iter()
                .any(|endpoint| endpoint == LOCAL_FEDERATION_ENDPOINT)
            {
                connect.push(LOCAL_FEDERATION_ENDPOINT.to_string());
            }
            fallback.connect = Some(connect);
            log::info!(
                "federation: the default listen endpoint is unavailable ({first}); listening on 127.0.0.1:{port} and linking to the local node"
            );
            Bus::try_from(fallback).await.map_err(|error| {
                format!(
                    "the federation transport did not open: {first}; the loopback fallback also failed: {error}"
                )
            })
        }
    }
}

/// A currently free loopback port.
///
/// The probe is released before Zenoh binds it, so this is a best-effort
/// choice: a collision between the two is a bind failure the caller reports,
/// not a silent fallback to a shared port.
fn free_loopback_port() -> Result<u16, String> {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| error.to_string())
}

async fn build(
    settings: crate::settings::Settings,
    identity_path: PathBuf,
    peers_path: PathBuf,
    config: BusConfig,
) -> Result<FederationRuntime, String> {
    // One writer per directory: a second instance must not start a node whose
    // in-memory peer store would diverge from this one's. The identity and the
    // peer store may live in different directories, so both are locked in one
    // sorted pass - a shared peer store with two keys is the shape that would
    // otherwise corrupt it.
    let lock = crate::federation_lock::NodeLocks::acquire(&identity_path, &peers_path)?;
    let identity =
        NodeIdentity::load_or_create(&identity_path).map_err(|error| error.to_string())?;
    let peers = PeerStore::load(&peers_path).map_err(|error| error.to_string())?;
    let mut config = config;
    if let Some(terminator) = start_tls_terminator(&mut config, &settings).await {
        if let Ok(mut slot) = terminator_slot().lock() {
            if let Some(old) = slot.take() {
                old.abort();
            }
            *slot = Some(terminator);
        }
    }
    let explicit_listen = !settings.global.federation.listen.is_empty();
    let bus = open_bus(config, explicit_listen).await?;
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
    let (runtime, advertise) = match node_runtime(&settings) {
        Ok((runtime, advertise)) => (Some(runtime), advertise),
        Err(error) if !settings.global.federation.expose.is_empty() => {
            log::warn!("federation: exposed capabilities were not built: {error}");
            (None, Vec::new())
        }
        Err(_) => (None, Vec::new()),
    };
    let profile = crate::federation::resolve_profile(Some(&settings.global.federation.profile));
    Ok(assemble_with_federation(
        identity,
        peers,
        Arc::new(transport),
        inbox.into_receiver(),
        Some(bus),
        audit,
        Some(lock),
        runtime,
        Some(delegation_store_path(&peers_path)),
        advertise,
        Some(crate::federation::display_name(
            settings.global.federation.node_name.as_deref(),
            &profile,
        )),
        settings.global.federation.federation_id.clone(),
        settings.global.federation.knowledge_enabled,
    ))
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble(
    identity: NodeIdentity,
    peers: PeerStore,
    transport: Arc<dyn FederationTransport>,
    inbox: tokio::sync::mpsc::Receiver<Vec<u8>>,
    bus: Option<Bus>,
    audit: Option<PathBuf>,
    lock: Option<crate::federation_lock::NodeLocks>,
    runtime: Option<Arc<WorkspaceRuntime>>,
    delegations: Option<PathBuf>,
    advertise: Vec<agent::capability::CapabilityDescriptor>,
    node_name: Option<String>,
) -> FederationRuntime {
    assemble_with_federation(
        identity,
        peers,
        transport,
        inbox,
        bus,
        audit,
        lock,
        runtime,
        delegations,
        advertise,
        node_name,
        None,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_with_federation(
    identity: NodeIdentity,
    peers: PeerStore,
    transport: Arc<dyn FederationTransport>,
    inbox: tokio::sync::mpsc::Receiver<Vec<u8>>,
    bus: Option<Bus>,
    audit: Option<PathBuf>,
    lock: Option<crate::federation_lock::NodeLocks>,
    runtime: Option<Arc<WorkspaceRuntime>>,
    delegations: Option<PathBuf>,
    advertise: Vec<agent::capability::CapabilityDescriptor>,
    node_name: Option<String>,
    federation_id: Option<String>,
    knowledge_enabled: bool,
) -> FederationRuntime {
    let announce_transport = Arc::clone(&transport);
    let peer_file = delegations
        .as_ref()
        .and_then(|path| path.parent())
        .map(|directory| directory.join("peers.json"));
    let mut node = FederationNode::new(identity.clone(), transport)
        .with_peer_store(Arc::new(RwLock::new(peers)));
    if let Some(peer_file) = &peer_file {
        let root = peer_file
            .parent()
            .map(|directory| directory.join("artifacts"));
        if let Some(root) = root {
            let _ = std::fs::create_dir_all(&root);
            node = node.with_artifact_store(Arc::new(federation::FileArtifactStore::new(root)));
        }
    }
    if knowledge_enabled {
        let knowledge_peers = node.peers();
        let knowledge_path = peer_file
            .as_ref()
            .map(|path| path.with_file_name("knowledge.json"));
        let service = knowledge_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(
                |text| match federation::KnowledgeService::from_json(&text) {
                    Ok(service) => Some(service),
                    Err(error) => {
                        log::warn!("federation: knowledge store was not loaded: {error}");
                        None
                    }
                },
            )
            .unwrap_or_default();
        node = node.with_knowledge(
            Arc::new(RwLock::new(service)),
            Arc::new(crate::federation_knowledge::GrantKnowledgeAuthorizer::new(
                knowledge_peers,
            )),
        );
        if let Some(path) = knowledge_path {
            node = node.with_knowledge_persistence(path);
        }
    }
    let federation = federation_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(federation::FederationId::new);
    if let Some(id) = federation.clone() {
        node = node.with_federation(id, 1);
    }
    if let Some(runtime) = runtime {
        node = node.with_runtime(runtime);
    }
    if let Some(path) = delegations {
        node = node.with_lease_persistence(path.with_file_name("leases.json"));
        node = node.with_replay_persistence(path.with_file_name("replay.json"));
        node = node.with_task_persistence(path.with_file_name("tasks.json"));
        node = node.with_delegation_persistence(path);
    }
    if let Some(path) = audit {
        // The ledger is the authoritative record; the bounded tail stays the
        // diagnostic view. If the ledger cannot open, the node keeps the tail
        // rather than failing to start, and the warning names the reason.
        match crate::federation_audit::DurableAudit::new(path.clone()) {
            Ok(sink) => node = node.with_audit(Arc::new(sink)),
            Err(error) => {
                log::warn!("federation audit ledger did not open: {error}");
                node = node.with_audit(Arc::new(crate::federation_audit::FileAudit::new(path)));
            }
        }
    }
    let node = Arc::new(node);
    let node_id = node.node();
    let task = node.start(inbox);
    let reconciler = peer_file.as_ref().map(|peers_path| {
        let node = Arc::clone(&node);
        let queue_path = peers_path.with_file_name("compensation.json");
        let tasks_path = peers_path.with_file_name("tasks.json");
        tokio::spawn(async move {
            let auditor = federation::ReceiptAuditor::new(
                Arc::clone(&node),
                agent::capability::InvocationContext::local(
                    agent::capability::InvocationId::new("receipt-audit"),
                    agent::capability::Principal::Local {
                        session: "federation-auditor".to_string(),
                        preset: String::new(),
                    },
                    WorkspaceId::new("ws:federation").expect("a workspace"),
                ),
            );
            loop {
                let mut queue = federation::CompensationQueue::load(&queue_path);
                let now = unix_ms();
                for entry in queue.due_entries(now) {
                    let Ok(target) = NodeId::new(entry.target.clone()) else {
                        queue.remove(&entry.delegation_id);
                        continue;
                    };
                    let id = federation::DelegationId::new(entry.delegation_id.clone());
                    let context = agent::capability::InvocationContext::local(
                        agent::capability::InvocationId::new(format!("comp-{}", entry.task_id)),
                        agent::capability::Principal::Local {
                            session: "federation-reconciler".to_string(),
                            preset: String::new(),
                        },
                        WorkspaceId::new("ws:federation").expect("a workspace"),
                    );
                    match node.cancel_delegation(&target, &id, &context).await {
                        Ok(_) => {
                            queue.remove(&entry.delegation_id);
                        }
                        Err(error) => {
                            queue.record_failure(&entry.delegation_id, error.to_string(), unix_ms())
                        }
                    }
                }
                if let Err(error) = queue.save() {
                    log::debug!("federation: compensation queue was not saved: {error}");
                }
                if tasks_path.is_file() {
                    if let Ok(graph) = federation::TaskGraph::load(&tasks_path) {
                        let tasks: Vec<federation::TaskNode> =
                            graph.tasks.values().cloned().collect();
                        let mismatches = auditor.audit(&tasks).await;
                        if !mismatches.is_empty() {
                            log::warn!(
                                "federation: {} compensation receipt(s) disagree with their target",
                                mismatches.len()
                            );
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        })
    });
    if let Some(peer_file) = peer_file {
        notify_rotation(&node, &peer_file);
    }
    let peer_capabilities = Arc::new(RwLock::new(BTreeMap::new()));
    let refresher = {
        let node = Arc::clone(&node);
        let cache = Arc::clone(&peer_capabilities);
        tokio::spawn(async move {
            loop {
                refresh(&node, &cache).await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        })
    };
    let discovered = Arc::new(RwLock::new(BTreeMap::<NodeId, DiscoveryRecord>::new()));
    let node_name = Arc::new(RwLock::new(node_name));
    let announce_task = {
        let transport = Arc::clone(&announce_transport);
        let identity = identity.clone();
        let discovered = Arc::clone(&discovered);
        let own = node_id.clone();
        let advertise = advertise.clone();
        let federation = federation.clone();
        let name_slot = Arc::clone(&node_name);
        tokio::spawn(async move {
            let Some(mut frames) = (match transport.subscribe_announce().await {
                Ok(frames) => frames,
                Err(error) => {
                    log::debug!("federation: announcements are unavailable: {error}");
                    None
                }
            }) else {
                return;
            };
            let mut tick = tokio::time::interval(ANNOUNCE_INTERVAL);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let name = name_slot
                            .read()
                            .map(|slot| slot.clone())
                            .unwrap_or_default();
                        announce_once(
                            &identity,
                            &transport,
                            &own,
                            &advertise,
                            name.as_deref(),
                            federation.as_ref(),
                            1,
                        )
                        .await;
                        let now = unix_ms();
                        if let Ok(mut map) = discovered.write() {
                            map.retain(|_, record| {
                                now.saturating_sub(record.published_at_ms) < DISCOVERY_STALE_MS
                            });
                        }
                    }
                    frame = frames.recv() => {
                        let Some(frame) = frame else { break };
                        if let Ok(mut map) = discovered.write() {
                            apply_announce(&mut map, &own, &frame, unix_ms());
                        }
                    }
                }
            }
        })
    };
    FederationRuntime {
        node,
        node_id,
        peer_capabilities,
        bus,
        task,
        refresher,
        announce_task,
        reconciler,
        discovered,
        identity,
        announce_transport,
        advertise,
        node_name,
        federation,
        policy_revision: 1,
        lock,
    }
}

/// Deliver a pending signed rotation record to every currently enrolled peer.
///
/// The file is produced by the federation rotate command; sending is idempotent,
/// so a peer that was offline receives it on a later launch. Delivery failures
/// are logged, never fatal to startup.
fn notify_rotation(node: &Arc<FederationNode>, peers_path: &std::path::Path) {
    let Some(directory) = peers_path.parent() else {
        return;
    };
    let path = directory.join("rotation.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(record) = RotationRecord::from_json(&text) else {
        log::warn!(
            "federation: rotation record {} is unreadable",
            path.display()
        );
        return;
    };
    let peers: Vec<NodeId> = node
        .peers()
        .read()
        .map(|store| {
            store
                .iter()
                .filter(|peer| peer.enrolled(unix_ms()))
                .map(|peer| peer.node.clone())
                .collect()
        })
        .unwrap_or_default();
    if peers.is_empty() {
        return;
    }
    let node = Arc::clone(node);
    crate::extension::spawn_async(async move {
        for peer in peers {
            let context = agent::capability::InvocationContext::local(
                agent::capability::InvocationId::new(format!("rotation-{peer}")),
                agent::capability::Principal::Local {
                    session: "federation-rotation".to_string(),
                    preset: String::new(),
                },
                WorkspaceId::new("ws:federation").expect("a workspace"),
            );
            if let Err(error) = node.publish_rotation(&peer, &record, &context).await {
                log::debug!("federation: rotation was not delivered to {peer}: {error}");
            }
        }
    });
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

    /// Two nodes see each other on the announce key without any enrollment,
    /// and neither lists itself.
    #[tokio::test]
    async fn two_nodes_discover_each_other_by_announcement() {
        use agent::capability::{CapabilityDescriptor, CapabilityKind, VersionSource, Visibility};

        let network = Arc::new(InMemoryNetwork::new());
        let a_identity = NodeIdentity::from_seed([61u8; 32]);
        let b_identity = NodeIdentity::from_seed([62u8; 32]);
        let a_id = a_identity.node_id();
        let b_id = b_identity.node_id();
        let descriptor = CapabilityDescriptor::new(
            "builtin/read_file".parse().expect("an id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        )
        .with_visibility(Visibility::Public);

        let (a_transport, a_inbox) = network.join(a_id.clone());
        let a = assemble(
            a_identity,
            PeerStore::new(),
            Arc::new(a_transport),
            a_inbox,
            None,
            None,
            None,
            None,
            None,
            vec![descriptor.clone()],
            Some("alpha".to_string()),
        );
        let (b_transport, b_inbox) = network.join(b_id.clone());
        let b = assemble(
            b_identity,
            PeerStore::new(),
            Arc::new(b_transport),
            b_inbox,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
            Some("test node".to_string()),
        );

        // Let both announce listeners subscribe before the first frame.
        tokio::time::sleep(Duration::from_millis(20)).await;
        a.announce_now().await;

        for _ in 0..100 {
            if !b.announced_peers().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let seen = b.announced_peers();
        assert_eq!(seen.len(), 1, "one peer announced");
        assert_eq!(seen[0].node, a_id);
        assert_eq!(
            seen[0].name.as_deref(),
            Some("alpha"),
            "the announcement carries the node's name"
        );
        assert_eq!(seen[0].descriptors.len(), 1);
        assert_eq!(seen[0].descriptors[0].id.to_string(), "builtin/read_file");

        assert!(
            a.announced_peers().iter().all(|record| record.node != a_id),
            "a node never lists itself"
        );
    }

    /// An announcement that is not signed by the node it names is ignored, and
    /// so is a frame that is not an announcement at all.
    #[tokio::test]
    async fn a_forged_announcement_is_ignored() {
        use agent::capability::{CapabilityDescriptor, CapabilityKind, VersionSource, Visibility};
        use federation::DiscoveryRecord as Record;

        let identity = NodeIdentity::from_seed([63u8; 32]);
        let own = identity.node_id();
        let stranger = NodeIdentity::from_seed([64u8; 32]);
        let descriptor = CapabilityDescriptor::new(
            "builtin/read_file".parse().expect("an id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        )
        .with_visibility(Visibility::Public);

        let mut map = BTreeMap::new();

        // A record signed by one node but naming another.
        let mut forged = Record::publish(&identity, vec![descriptor.clone()]);
        forged.node = stranger.node_id();
        let message =
            FederationMessage::new("announce", serde_json::to_vec(&forged).expect("bytes"));
        let envelope = Envelope::seal(&message, &identity, &own).expect("sealed");
        let frame = envelope.encode().expect("encoded");
        assert!(!apply_announce(&mut map, &own, &frame, unix_ms()));
        assert!(map.is_empty());

        // A properly signed record whose envelope is not an announcement.
        let record = Record::publish(&stranger, vec![descriptor]);
        let message = FederationMessage::new("invoke", serde_json::to_vec(&record).expect("bytes"));
        let envelope = Envelope::seal(&message, &stranger, &own).expect("sealed");
        let frame = envelope.encode().expect("encoded");
        assert!(!apply_announce(&mut map, &own, &frame, unix_ms()));
        assert!(map.is_empty());
    }

    /// An exposed read-only capability is advertised Public and can actually
    /// be invoked by a peer that holds a grant on both sides.
    #[tokio::test]
    async fn an_exposed_capability_is_invokable_by_a_granted_peer() {
        use agent::capability::{
            CapabilityId, InvocationContext, InvocationId, Principal, WorkspaceId,
        };
        use serde_json::json;

        let root = std::env::temp_dir().join(format!("bsh-node-expose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        std::fs::write(root.join("a.txt"), b"hello from the node\n").expect("a file");

        let mut settings = crate::settings::Settings::default();
        let workspace = settings.note_workspace("expose", &root.to_string_lossy());
        settings.global.general.active_workspace = workspace;
        settings.global.federation.expose = vec!["builtin/read_file".to_string()];
        settings.global.federation.expose_root = Some(root.to_string_lossy().to_string());

        let (runtime, advertise) = node_runtime(&settings).expect("the node runtime");
        assert_eq!(advertise.len(), 1);
        assert_eq!(
            advertise[0].visibility,
            agent::capability::Visibility::Public,
            "only a public descriptor may be announced"
        );

        let network = Arc::new(InMemoryNetwork::new());
        let provider_identity = NodeIdentity::from_seed([81u8; 32]);
        let provider_id = provider_identity.node_id();
        let (provider_transport, provider_inbox) = network.join(provider_id.clone());
        let provider = assemble(
            provider_identity,
            PeerStore::new(),
            Arc::new(provider_transport),
            provider_inbox,
            None,
            None,
            None,
            Some(Arc::clone(&runtime)),
            None,
            advertise,
            Some("provider".to_string()),
        );

        let caller_identity = NodeIdentity::from_seed([82u8; 32]);
        let caller_id = caller_identity.node_id();
        let (caller_transport, caller_inbox) = network.join(caller_id.clone());
        let caller = assemble(
            caller_identity,
            PeerStore::new(),
            Arc::new(caller_transport),
            caller_inbox,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
            Some("test node".to_string()),
        );

        let capability: CapabilityId = "builtin/read_file".parse().expect("an id");
        let now = unix_ms();
        {
            let peers = provider.node().peers();
            let mut peers = peers.write().expect("peers");
            peers.enroll(caller_id.clone(), now, None);
            peers
                .grant(&caller_id, capability.clone(), now, None)
                .expect("granted");
        }
        {
            let peers = caller.node().peers();
            let mut peers = peers.write().expect("peers");
            peers.enroll(provider_id.clone(), now, None);
            peers
                .grant(&provider_id, capability.clone(), now, None)
                .expect("granted");
        }

        let context = InvocationContext::local(
            InvocationId::new("inv:node-expose"),
            Principal::Local {
                session: "session".to_string(),
                preset: "preset".to_string(),
            },
            WorkspaceId::new("ws:caller").expect("a workspace"),
        );
        let output = caller
            .node()
            .invoke(
                &provider_id,
                &capability,
                json!({ "path": "a.txt" }),
                &context,
            )
            .await
            .expect("the peer reads the file");
        let text = serde_json::to_string(&output).expect("the output serializes");
        assert!(text.contains("hello from the node"), "{text}");
    }

    #[test]
    fn an_expose_entry_the_node_cannot_serve_is_refused() {
        let mut settings = crate::settings::Settings::default();
        settings.global.federation.expose = vec!["builtin/bash".to_string()];
        settings.global.federation.expose_root =
            Some(std::env::temp_dir().to_string_lossy().to_string());

        let refused = node_runtime(&settings);

        assert!(refused.is_err(), "the shell is never exposed");
    }

    #[test]
    fn a_replayed_older_announcement_is_ignored() {
        use agent::capability::{CapabilityDescriptor, CapabilityKind, VersionSource, Visibility};

        let identity = NodeIdentity::from_seed([65u8; 32]);
        let own = NodeIdentity::from_seed([66u8; 32]).node_id();
        let descriptor = CapabilityDescriptor::new(
            "builtin/read_file".parse().expect("an id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        )
        .with_visibility(Visibility::Public);

        let first = DiscoveryRecord::publish_named(
            &identity,
            Some("peer".into()),
            vec![descriptor.clone()],
        );
        std::thread::sleep(Duration::from_millis(2));
        let second =
            DiscoveryRecord::publish_named(&identity, Some("peer".into()), vec![descriptor]);
        assert!(first.published_at_ms < second.published_at_ms);

        let frame = |record: &DiscoveryRecord| {
            let message =
                FederationMessage::new("announce", serde_json::to_vec(record).expect("bytes"));
            Envelope::seal(&message, &identity, &own)
                .expect("sealed")
                .encode()
                .expect("encoded")
        };

        let mut map = BTreeMap::new();
        assert!(apply_announce(&mut map, &own, &frame(&second), unix_ms()));
        assert!(
            !apply_announce(&mut map, &own, &frame(&first), unix_ms()),
            "the older record is a replay"
        );
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.values().next().expect("a record").published_at_ms,
            second.published_at_ms
        );
    }
    #[test]
    fn stopping_a_node_that_is_not_running_reports_false() {
        // No test in this binary starts the process-wide node through
        // `start_from_settings`; the slot is empty here.
        assert!(!crate::federation_node::stop());
        assert!(crate::federation_node::running().is_none());
    }
    #[test]
    fn a_disabled_setting_starts_no_node() {
        let mut settings = crate::settings::Settings::default();
        settings.global.federation.enabled = false;
        assert!(!start_from_settings(&settings));
    }

    /// A TLS node is one per process: the terminator is a singleton, and every
    /// profile shares the settings that carry the certificates.
    #[test]
    fn a_tls_node_is_recognised_from_the_settings() {
        let mut config = crate::settings::global::FederationSection::default();
        assert!(!has_tls(&config));
        config.tls_mtls = true;
        assert!(has_tls(&config));
        config.tls_mtls = false;
        config.tls_cert = Some("cert.pem".to_string());
        assert!(has_tls(&config));
    }

    /// A process with no node reports no profiles, and asking for a profile
    /// that was never started is a miss rather than a creation.
    #[test]
    fn a_process_with_no_node_reports_no_profiles() {
        assert!(crate::federation_node::running_profiles().is_empty());
        assert!(crate::federation_node::running_profile("work").is_none());
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
                decision: None,
            }],
            skipped: Vec::new(),
            budgets: agent::capability::composition::Budgets::default(),
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
            None,
            None,
            None,
            Vec::new(),
            Some("test node".to_string()),
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
            operations: Vec::new(),
            resources: Vec::new(),
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
            operations: Vec::new(),
            resources: Vec::new(),
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
            None,
            None,
            None,
            Vec::new(),
            Some("test node".to_string()),
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
            runtime.status().contains("catalogues=1"),
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
                decision: None,
            }],
            skipped: Vec::new(),
            budgets: agent::capability::composition::Budgets::default(),
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
            None,
            None,
            None,
            Vec::new(),
            Some("test node".to_string()),
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
            None,
            None,
            None,
            Vec::new(),
            Some("test node".to_string()),
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

    /// END-TO-END over the REAL transport: two profiles on this host, each with
    /// its own directory, key, peer store, lock and Zenoh session, linked by TCP
    /// on loopback. This is the path the app runs, minus the window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_local_profiles_federate_over_the_real_transport() {
        use agent::capability::{InvocationContext, InvocationId, Principal, WorkspaceId};
        use federation::{DelegationId, DelegationRequest, DelegationState};
        use serde_json::json;

        let root =
            std::env::temp_dir().join(format!("bsh-fed-e2e-{}-{}", std::process::id(), unix_ms()));
        let docs = root.join("docs");
        std::fs::create_dir_all(&docs).expect("the exposed root");
        std::fs::write(docs.join("hello.txt"), "hello from node a").expect("the exposed file");

        // Two profiles, one identity each, paired in both directions: each side
        // trusts the other and the caller holds the provider's grant.
        for profile in ["a", "b"] {
            let dir = crate::federation::node_dir_in(&root, profile);
            std::fs::create_dir_all(&dir).expect("a profile directory");
            NodeIdentity::load_or_create(&dir.join("node.key")).expect("an identity");
        }
        let mut out = Vec::new();
        for (this, other) in [("a", "b"), ("b", "a")] {
            assert_eq!(
                crate::federation::pair(
                    &root,
                    this,
                    other,
                    &["builtin/read_file".to_string()],
                    &mut out
                ),
                0,
                "{}",
                String::from_utf8_lossy(&out)
            );
        }
        let a_identity = NodeIdentity::load_or_create(
            &crate::federation::node_dir_in(&root, "a").join("node.key"),
        )
        .expect("node a");
        let b_identity = NodeIdentity::load_or_create(
            &crate::federation::node_dir_in(&root, "b").join("node.key"),
        )
        .expect("node b");
        let a_id = a_identity.node_id();
        let b_id = b_identity.node_id();

        let mut settings = crate::settings::Settings::default();
        settings.global.federation.enabled = true;
        settings.global.federation.expose = vec!["builtin/read_file".to_string()];
        settings.global.federation.expose_root = Some(docs.to_string_lossy().to_string());
        // The exposed tool's policy is the workspace policy, so the exposed root
        // must ALSO be the active workspace; otherwise the read is refused by
        // the baseline as "outside the workspace" before any peer is involved.
        settings.global.workspaces = vec![crate::settings::global::WorkspaceRef {
            id: "ws-e2e".to_string(),
            name: "e2e".to_string(),
            root: docs.to_string_lossy().to_string(),
            created_at: 0,
            updated_at: 0,
        }];
        settings.global.general.active_workspace = "ws-e2e".to_string();

        let port_a = free_loopback_port().expect("a port for a");
        let port_b = free_loopback_port().expect("a port for b");
        let bus = |port: u16, connect: Option<u16>| BusConfig {
            mode: "peer".to_string(),
            connect: connect.map(|port| vec![format!("tcp/127.0.0.1:{port}")]),
            listen: Some(vec![format!("tcp/127.0.0.1:{port}")]),
            peer: None,
            tls: None,
            // The real host may run other BSH nodes; this pair must find each
            // other through the endpoint it was given, not through scouting.
            scouting: false,
        };
        let a = build(
            settings.clone(),
            root.join("a").join("node.key"),
            root.join("a").join("peers.json"),
            bus(port_a, None),
        )
        .await
        .expect("node a starts over TCP");
        let b = build(
            settings.clone(),
            root.join("b").join("node.key"),
            root.join("b").join("peers.json"),
            bus(port_b, Some(port_a)),
        )
        .await
        .expect("node b starts over TCP");

        // Both profile locks are held while the nodes run.
        assert!(
            crate::federation_lock::DataDirLock::acquire(&root.join("a")).is_err(),
            "a running node owns its directory"
        );

        // Presence over the real link, then the catalogue behind the grant.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if !a.announced_peers().is_empty() && !b.announced_peers().is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no announcement over TCP: a={} b={}",
                a.status(),
                b.status()
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(
            a.announced_peers().iter().any(|peer| peer.node == b_id),
            "a sees b: {}",
            a.status()
        );
        b.refresh_peers().await;
        assert!(
            b.discovered_peers() >= 1,
            "the granted catalogue arrives: {}",
            b.status()
        );

        let context = InvocationContext::local(
            InvocationId::new("inv:e2e-host"),
            Principal::Local {
                session: "e2e".to_string(),
                preset: "standard".to_string(),
            },
            WorkspaceId::new("ws:e2e-host").expect("a workspace"),
        );

        // A remote read over the real socket.
        let output = b
            .node()
            .invoke(
                &a_id,
                &"builtin/read_file".parse().expect("a capability"),
                json!({ "path": "hello.txt" }),
                &context,
            )
            .await
            .expect("the peer answers");
        let text = serde_json::to_string(&output).expect("the output serializes");
        assert!(text.contains("hello from node a"), "{text}");

        // The coordination plane: accepted, readable, cancelled.
        let request = DelegationRequest {
            id: DelegationId::new("d-e2e-host"),
            parent: None,
            target: a_id.clone(),
            objective: "read hello.txt".to_string(),
            capabilities: vec!["builtin/read_file".parse().expect("a capability")],
            operations: Vec::new(),
            resources: Vec::new(),
            deadline_ms: None,
            allow_sub_delegation: false,
            result_contract: None,
        };
        let accepted = b
            .node()
            .delegate(&a_id, request, &context)
            .await
            .expect("the peer accepts the task");
        assert_eq!(accepted.state, DelegationState::Accepted);
        let status = b
            .node()
            .delegation_status(&a_id, &DelegationId::new("d-e2e-host"), &context)
            .await
            .expect("the owner reports");
        assert_eq!(status.state, DelegationState::Accepted);
        let cancelled = b
            .node()
            .cancel_delegation(&a_id, &DelegationId::new("d-e2e-host"), &context)
            .await
            .expect("cancelled");
        assert_eq!(cancelled.state, DelegationState::Cancelled);

        // The provider's audit log names the work it did.
        let audit = std::fs::read_dir(root.join("a"))
            .expect("node a's directory")
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("audit") && name.ends_with(".jsonl"))
            });
        let log = audit
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        assert!(log.contains("builtin/read_file"), "audit: {log}");
        assert!(log.contains("d-e2e-host"), "audit: {log}");

        // Revocation cuts the plane: the same call is refused now.
        a.node().peers().write().expect("peers").revoke(&b_id);
        let refused = b
            .node()
            .invoke(
                &a_id,
                &"builtin/read_file".parse().expect("a capability"),
                json!({ "path": "hello.txt" }),
                &context,
            )
            .await;
        assert!(refused.is_err(), "a revoked peer is refused");
        // The coordination plane is cut by the same decision.
        let revoked_task = DelegationRequest {
            id: DelegationId::new("d-e2e-revoked"),
            parent: None,
            target: a_id.clone(),
            objective: "should not run".to_string(),
            capabilities: vec!["builtin/read_file".parse().expect("a capability")],
            operations: Vec::new(),
            resources: Vec::new(),
            deadline_ms: None,
            allow_sub_delegation: false,
            result_contract: None,
        };
        assert!(
            b.node()
                .delegate(&a_id, revoked_task, &context)
                .await
                .is_err(),
            "a revoked peer cannot delegate either"
        );

        // Dropping a node releases both of its directory locks.
        drop(a);
        drop(b);
        assert!(crate::federation_lock::DataDirLock::acquire(&root.join("a")).is_ok());
        assert!(crate::federation_lock::DataDirLock::acquire(&root.join("b")).is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }
}
