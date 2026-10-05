//! `rex` — unified resource explorer CLI.
//!
//! Explore resources through one client, without distinguishing local from
//! remote. The same commands work against either:
//!
//! ```text
//! rex list
//! rex tree folder:///tmp/demo
//! rex peek file:///tmp/demo/a.txt --bytes 64
//! rex watch mem://kv ready
//! ```
//!
//! Wiring:
//!   `--register folder:///tmp/demo`        register a local resource by URI
//!   `--node 127.0.0.1:4433 --server myserver --ca --cert --key`
//!                                          connect a remote node (QUIC, mTLS)

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::process::exit;
use std::sync::Arc;
use std::time::Instant;

use bus::{Bus, BusConfig};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use rcgen::{BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair};
use resource::discovery::{discovery_topic, LoadSnapshot, NodeAnnounce, DISCOVERY_TOPIC};
use resource::explorer::{handler_for, Explorer, Row};
use resource::meta::ResourceStateLabel;
use resource::policy::{Effect, PolicyDoc, Rule, SharedPolicy};
use resource::transport::Transport;
use resource::transport::{QuicServer, QuicTransport};
use resource::{
    d, init_logging, ResourceClient, ResourceError, ResourceManager, ResourceType,
    VirtualNodeResource,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

#[derive(Parser)]
#[command(name = "rex", about = "Unified resource explorer (local == remote)")]
struct Cli {
    /// Enable verbose debug output on stderr (routing, transport, dispatch).
    #[arg(long, global = true)]
    debug: bool,

    /// Disable colored output (useful for piping).
    #[arg(long, global = true)]
    no_color: bool,

    /// Register a local resource by URI (`file:///path`, `folder:///path`,
    /// `mem://id`, `proc://name`, `sock://addr`, `combine://name`). Repeatable.
    #[arg(long)]
    register: Vec<String>,

    /// Connect to one remote node at this address (e.g. 127.0.0.1:4433). The
    /// node is used as the *default* transport, so its host-less resources are
    /// operated with the exact same commands as local ones. Requires --ca,
    /// --cert, --key, and --server.
    #[arg(long)]
    node: Option<String>,

    /// Server name (SNI) used to reach --node; must match the node's server
    /// certificate CN.
    #[arg(long, default_value = "server")]
    server: String,

    /// CA bundle (PEM) used to verify the remote node's certificate.
    #[arg(long)]
    ca: Option<String>,

    /// This explorer's client certificate (PEM) for remote node auth.
    #[arg(long)]
    cert: Option<String>,

    /// This explorer's client key (PEM).
    #[arg(long)]
    key: Option<String>,

    /// Load defaults from a config file (TOML). Values set on the command line
    /// win; everything else falls back to the file, then to built-in defaults.
    /// When omitted, `./rex.toml` or `./config.toml` is used if present.
    #[arg(long)]
    config: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

/// File-config defaults for `rex` (TOML). Every field is optional and only
/// fills in command-line flags that were not explicitly provided.
///
/// Supports both flat keys (legacy) and nested sections:
///
/// ```toml
/// # Flat (legacy)
/// node = "127.0.0.1:4433"
/// server = "myserver"
/// ca = "certs/ca.pem"
/// register = ["folder:///tmp/data"]
///
/// # Nested sections (preferred)
/// [node_section]
/// id = "nodeA"
/// bind = "127.0.0.1:4433"
/// server = "myserver"
///
/// [certs]
/// ca = "certs/ca.pem"
/// server = "certs/server.pem"
/// server_key = "certs/server.key"
/// auto_generate = true
///
/// [discovery]
/// shared_secret = "my-secret"
/// ```
#[derive(serde::Deserialize, Default)]
struct RexFileConfig {
    // Flat keys (legacy, backward-compatible).
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    ca: Option<String>,
    #[serde(default)]
    cert: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    register: Vec<String>,
    // Nested sections.
    #[serde(default, rename = "node_section")]
    node_cfg: NodeConfig,
    #[serde(default)]
    certs: CertsConfig,
    #[serde(default)]
    discovery: DiscoveryConfig,
}

#[derive(serde::Deserialize, Default)]
struct NodeConfig {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    bind: Option<String>,
    #[serde(default)]
    server: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct CertsConfig {
    #[serde(default)]
    ca: Option<String>,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    server_key: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    client: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    client_key: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    auto_generate: bool,
}

#[derive(serde::Deserialize, Default)]
struct DiscoveryConfig {
    #[serde(default)]
    shared_secret: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Discover resources across the local node and every connected node.
    List {
        /// Filter by kind: storage | network | compute | system | abstract.
        #[arg(long)]
        kind: Option<String>,
    },
    /// Recursive listing of a resource and its children.
    Tree {
        uri: String,
        /// Maximum recursion depth.
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
    /// Read-only peek: first N bytes of a file or the next N from a socket.
    Peek {
        uri: String,
        #[arg(long, default_value_t = 64)]
        bytes: u64,
    },
    /// Short description of one resource (state, owner, kind).
    Status { uri: String },
    /// Live view: stream a resource's push events until Ctrl-C.
    Watch {
        uri: String,
        /// Event names to subscribe to (resource-specific).
        events: Vec<String>,
    },
    /// Generate a CA + server + client certificate set for running a node.
    Ca {
        /// Directory to write ca.pem, server.pem/key, client.pem/key into.
        #[arg(long, default_value = "certs")]
        out: String,
        /// CN for the server cert (also the --server name the client verifies).
        #[arg(long, default_value = "server")]
        server_cn: String,
        /// CN for the client cert (becomes the agent identity, e.g. "agent1").
        #[arg(long, default_value = "agent1")]
        client_cn: String,
    },
    /// Start a resource node: bind a QUIC server, host --register resources,
    /// and accept connections until interrupted.
    Serve {
        /// Address to bind, e.g. 127.0.0.1:4433 or 0.0.0.0:4433.
        #[arg(long, default_value = "127.0.0.1:4433")]
        bind: String,
        /// Resources this node hosts, repeatable (`folder:///tmp/x`, `mem://id`, …).
        #[arg(long)]
        register: Vec<String>,
        /// Rego policy file to load for authorization (~/.bos/ress / rex.toml).
        /// If not provided, defaults to permissive policy (allow all).
        #[arg(long)]
        policy: Option<String>,
        /// CA bundle (PEM) that issued both server and client certs.
        #[arg(long, default_value = "certs/ca.pem")]
        ca: String,
        /// This node's server certificate (PEM).
        #[arg(long, default_value = "certs/server.pem")]
        cert: String,
        /// This node's server key (PEM).
        #[arg(long, default_value = "certs/server.key")]
        key: String,
        /// Unique node identifier for bus-based discovery (e.g. "nodeA").
        /// Enables auto-discovery: this node announces its resources on the
        /// bus and discovers peers to form a super vnode cluster.
        /// Defaults to the system hostname for zero-config operation.
        #[arg(long, default_value_t = hostname::get().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|_| "node".to_string()))]
        node_id: String,
    },
}

fn kind_of(s: &str) -> std::result::Result<ResourceType, String> {
    Ok(match s {
        "storage" => ResourceType::Storage,
        "network" => ResourceType::Network,
        "compute" => ResourceType::Compute,
        "system" => ResourceType::System,
        "abstract" => ResourceType::Abstract,
        "combine" => ResourceType::Combine,
        other => return Err(format!("unknown kind '{other}'")),
    })
}

/// Assemble the explorer client: one agent identity, a permissive local
/// manager holding `--register` resources, and an optional default remote
/// transport for `--node`. Everything after this point is purely URI-driven:
/// a host-less URI that is not local is routed to the node transparently.
async fn assemble(cli: &Cli) -> Result<Arc<ResourceClient>, String> {
    let reviewer = "explorer";
    let policy = SharedPolicy::new(PolicyDoc {
        admins: vec![reviewer.to_string()],
        rules: vec![Rule {
            agents: vec![reviewer.to_string()],
            uris: vec!["*".to_string()],
            actions: vec!["*".to_string()],
            effect: Effect::Allow,
        }],
        ..Default::default()
    });
    let manager = Arc::new(ResourceManager::new(policy));
    for uri in &cli.register {
        let handler = handler_for(uri).ok_or_else(|| {
            format!("no known scheme for `{uri}` (try file/folder/mem/proc/sock/combine)")
        })?;
        manager
            .register(handler, reviewer.to_string())
            .await
            .map_err(|e| format!("register {uri}: {e}"))?;
        d!("registered local resource {uri}");
    }

    // Build the default remote transport before constructing the client, so
    // host-less remote resources route to it with zero caller branching.
    let transport: Option<Arc<dyn resource::transport::Transport>> = match &cli.node {
        Some(addr) => {
            let cert = cli.cert.as_deref().ok_or("--node requires --cert")?;
            let key = cli.key.as_deref().ok_or("--node requires --key")?;
            let sock: SocketAddr = addr
                .parse()
                .map_err(|e| format!("node addr `{addr}`: {e}"))?;

            // If --ca not provided, auto-discover the server's CA from the bus.
            let ca_bytes = match cli.ca.as_deref() {
                Some(ca_path) => std::fs::read(ca_path).map_err(|e| format!("read ca: {e}"))?,
                None => {
                    d!("--ca not provided, discovering server CA from bus");
                    let bus_cfg = load_bus_config_from_discovery().unwrap_or_default();
                    let mut bus = Bus::from(bus_cfg).await;
                    let topic = format!("{DISCOVERY_TOPIC}/**");
                    let (tx, rx) = std::sync::mpsc::channel::<String>();
                    let server_name = cli.server.clone();
                    let target_addr = addr.clone();
                    bus.subscribe(&topic, move |ann: NodeAnnounce| {
                        if ann.quic_addr == target_addr || ann.server_name == server_name {
                            if !ann.ca_cert.is_empty() {
                                let _ = tx.send(ann.ca_cert.clone());
                            }
                        }
                    })
                    .await;
                    // Wait for server CA — peer re-announces every 5s.
                    match rx.recv_timeout(std::time::Duration::from_secs(8)) {
                        Ok(ca_pem) => {
                            d!("discovered server CA for {}", cli.server);
                            ca_pem.into_bytes()
                        }
                        _ => {
                            return Err(format!(
                                "could not discover server CA for {addr} (hint: start the server first, or use --ca)"
                            ));
                        }
                    }
                }
            };

            let cert_bytes = std::fs::read(cert).map_err(|e| format!("read cert: {e}"))?;
            let key_bytes = std::fs::read(key).map_err(|e| format!("read key: {e}"))?;
            let t =
                build_quic_from_bytes(sock, &cli.server, &ca_bytes, &cert_bytes, &key_bytes, None)?;
            Some(Arc::new(t))
        }
        None => {
            d!("no remote node; local-only");
            None
        }
    };

    let client = Arc::new(ResourceClient::new(reviewer, Some(manager), transport));
    d!("client assembled (agent `{reviewer}`)");
    Ok(client)
}

fn build_quic_from_bytes(
    addr: std::net::SocketAddr,
    server_name: &str,
    ca: &[u8],
    cert_bytes: &[u8],
    key_bytes: &[u8],
    extra_ca: Option<&[u8]>,
) -> std::result::Result<QuicTransport, String> {
    let combined_ca = match extra_ca {
        Some(extra) if !extra.is_empty() => {
            let mut combined = ca.to_vec();
            combined.extend_from_slice(extra);
            combined
        }
        _ => ca.to_vec(),
    };
    let cert: CertificateDer<'static> = rustls_pemfile::certs(&mut &cert_bytes[..])
        .next()
        .ok_or("cert PEM contained no certificate")?
        .map_err(|e| format!("parse cert: {e}"))?
        .into_owned();
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut &key_bytes[..])
        .map_err(|e| format!("parse key: {e}"))?
        .ok_or("key PEM contained no private key")?;
    d!("quic client transport: addr={addr} server_name={server_name}");
    QuicTransport::new(addr, server_name.to_string(), &combined_ca, cert, key)
        .map_err(|e| format!("quic client: {e}"))
}

/// Extract the Common Name (CN) from a PEM-encoded certificate.
fn extract_cn_from_pem(pem: &[u8]) -> Result<String, String> {
    let cert = rustls_pemfile::certs(&mut &pem[..])
        .next()
        .ok_or("no certificate in PEM")?
        .map_err(|e| format!("parse cert: {e}"))?
        .into_owned();
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
        .map_err(|e| format!("x509 parse: {e}"))?;
    for attr in parsed.subject().iter_common_name() {
        if let Ok(cn) = attr.as_str() {
            return Ok(cn.to_string());
        }
    }
    Err("cert has no CN".to_string())
}

/// Generate a CA, a server certificate, and a client certificate, writing all
/// five PEM files into `out_dir`. The client cert's CN becomes the agent
/// identity when that client connects to a node.
fn gen_certs(out_dir: &str, server_cn: &str, client_cn: &str) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("create {out_dir}: {e}"))?;
    let write = |name: &str, data: &[u8]| {
        let p = Path::new(out_dir).join(name);
        std::fs::write(&p, data).map_err(|e| format!("write {}: {e}", p.display()))
    };

    // CA (self-signed).
    let mut ca_params =
        CertificateParams::new(vec!["rex-ca".to_string()]).map_err(|e| e.to_string())?;
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "rex-ca");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().map_err(|e| e.to_string())?;
    let ca_cert = ca_params.self_signed(&ca_key).map_err(|e| e.to_string())?;
    let ca = CertifiedKey {
        cert: ca_cert,
        key_pair: ca_key,
    };

    // Server + client signed by the CA.
    let server = sign(&ca, server_cn)?;
    let client = sign(&ca, client_cn)?;

    write("ca.pem", ca.cert.pem().as_bytes())?;
    write("server.pem", server.cert.pem().as_bytes())?;
    write("server.key", server.key_pair.serialize_pem().as_bytes())?;
    write("client.pem", client.cert.pem().as_bytes())?;
    write("client.key", client.key_pair.serialize_pem().as_bytes())?;

    println!("wrote CA + server + client certs to {out_dir}/");
    println!("  server CN = {server_cn}");
    println!("  client CN = {client_cn} (agent identity)");
    Ok(())
}

fn sign(ca: &CertifiedKey, cn: &str) -> Result<CertifiedKey, String> {
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let mut params = CertificateParams::new(vec![cn.to_string()]).map_err(|e| e.to_string())?;
    params.distinguished_name.push(DnType::CommonName, cn);
    let cert = params
        .signed_by(&key, &ca.cert, &ca.key_pair)
        .map_err(|e| e.to_string())?;
    Ok(CertifiedKey {
        cert,
        key_pair: key,
    })
}

/// Load `RexFileConfig` from `path`, if it exists; otherwise `None`. A missing
/// file is not an error (config is optional); a present-but-unparseable file is.
fn load_file_config(path: &str) -> Result<Option<RexFileConfig>, String> {
    if !Path::new(path).exists() {
        return Ok(None);
    }
    let mut loader = config::ConfigLoader::new().add_file(path);
    let value = loader
        .load_sync()
        .map_err(|e| format!("load {path}: {e}"))?;
    let cfg: RexFileConfig =
        serde_json::from_value(value).map_err(|e| format!("parse {path}: {e}"))?;
    Ok(Some(cfg))
}

/// Load `BusConfig` from the standard bos config discovery paths
/// (`~/.bos/conf/config.toml`, `./bos/conf/`, etc.). The `[bus]` section
/// provides `mode`, `connect`, `listen`, `peer`.
///
/// Falls back to `BusConfig::default()` (peer mode) when no bus config is
/// found or when it can't be parsed. For local development, automatically
/// adds `connect` to `tcp/127.0.0.1:7447` (zenohd default) when no
/// `listen` or `connect` is configured — peers find each other through
/// the router.
fn load_bus_config_from_discovery() -> Result<BusConfig, String> {
    let mut loader = config::ConfigLoader::new().discover();
    let value = loader
        .load_sync()
        .map_err(|e| format!("load bus config: {e}"))?;
    let mut cfg = match value.get("bus") {
        Some(bus_section) => match serde_json::from_value::<BusConfig>(bus_section.clone()) {
            Ok(mut cfg) => {
                if cfg.mode.is_empty() {
                    cfg.mode = "peer".to_string();
                }
                cfg
            }
            Err(e) => {
                d!("bus config parse error ({e}), using defaults");
                BusConfig::default()
            }
        },
        None => {
            d!("no [bus] config found, using defaults");
            BusConfig::default()
        }
    };
    // For local development: if no listen/connect is configured, connect to
    // the default zenohd router so two processes can discover each other.
    if cfg.listen.is_none() && cfg.connect.is_none() {
        cfg.connect = Some(vec!["tcp/127.0.0.1:7447".to_string()]);
        d!("no bus endpoints configured, connecting to local zenohd at tcp/127.0.0.1:7447");
    }
    Ok(cfg)
}

/// Fill any unset `Cli` options from the config file. Command-line values win.
fn apply_config(cli: &mut Cli, cfg: &RexFileConfig) {
    // Priority: CLI > flat keys > nested [node_section].
    if cli.node.is_none() {
        cli.node = cfg
            .node
            .clone()
            .or_else(|| cfg.node_cfg.bind.as_ref().map(|b| b.clone()));
    }
    if cli.server == "server" {
        if let Some(s) = cfg.server.as_ref().or(cfg.node_cfg.server.as_ref()) {
            cli.server = s.clone();
        }
    }
    if cli.ca.is_none() {
        cli.ca = cfg.ca.clone().or_else(|| cfg.certs.ca.clone());
    }
    if cli.cert.is_none() {
        cli.cert = cfg.cert.clone().or_else(|| cfg.certs.server.clone());
    }
    if cli.key.is_none() {
        cli.key = cfg.key.clone().or_else(|| cfg.certs.server_key.clone());
    }
    if cli.register.is_empty() {
        cli.register = cfg.register.clone();
    }
}

/// Run a resource node: build a permissive manager, register the hosted
/// resources, then bind and serve QUIC with mutual TLS until interrupted.
async fn serve(
    bind: &str,
    register: &[String],
    ca_path: &str,
    cert_path: &str,
    key_path: &str,
    policy_path: Option<&str>,
    node_id: Option<&str>,
    _server_name: &str,
    shared_secret: Option<&str>,
) -> Result<(), String> {
    // Load policy from file. Rego files (`.rego`) are used directly; otherwise
    // treated as a JSON `PolicyDoc` (JSON with optional `rego` field).
    let policy_doc = if let Some(path) = policy_path {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("read policy file {path}: {e}"))?;
        if path.ends_with(".rego") {
            // Raw Rego source → wrap in PolicyDoc.
            d!("loaded Rego policy from {path}");
            PolicyDoc {
                admins: vec![],
                rego: Some(content),
                rules: vec![],
            }
        } else {
            // JSON or TOML PolicyDoc
            PolicyDoc::from_json(&content).map_err(|e| format!("parse {path}: {e}"))?
        }
    } else {
        // Permissive default.
        PolicyDoc {
            admins: vec!["*".to_string()],
            rules: vec![Rule {
                agents: vec!["*".to_string()],
                uris: vec!["*".to_string()],
                actions: vec!["*".to_string()],
                effect: Effect::Allow,
            }],
            ..Default::default()
        }
    };

    let policy = SharedPolicy::new(policy_doc);
    let manager = Arc::new(ResourceManager::new(policy));
    // Host a proc:// manager: rex serve always offers process management so
    // peers can spawn/kill/list on this node.
    {
        let pm = resource::resource::proc::manager::ProcManager::new(manager.clone());
        manager
            .register(Box::new(pm), "node".to_string())
            .await
            .map_err(|e| format!("register proc:// manager: {e}"))?;
        d!("hosting proc:// manager");
    }
    for uri in register {
        let handler = handler_for(uri).ok_or_else(|| {
            format!("no known scheme for `{uri}` (try file/folder/mem/proc/sock/combine)")
        })?;
        let owner = handler.meta().owner.clone();
        manager
            .register(
                handler,
                if owner.is_empty() {
                    "node".into()
                } else {
                    owner
                },
            )
            .await
            .map_err(|e| format!("register {uri}: {e}"))?;
        d!("hosting resource {uri}");
    }

    // Auto-generate certs if missing and node_id is provided.
    let (effective_ca, effective_cert, effective_key) = if !Path::new(ca_path).exists()
        || !Path::new(cert_path).exists()
        || !Path::new(key_path).exists()
    {
        if let Some(nid) = node_id {
            let certs_dir = Path::new(ca_path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("certs");
            println!("[auto] no certs found, generating CA + server/client certs in {certs_dir}/");
            gen_certs(certs_dir, nid, "client")?;
            // Re-derive paths from the certs dir.
            let ca = Path::new(certs_dir).join("ca.pem");
            let cert = Path::new(certs_dir).join("server.pem");
            let key = Path::new(certs_dir).join("server.key");
            (
                ca.to_str().unwrap().to_string(),
                cert.to_str().unwrap().to_string(),
                key.to_str().unwrap().to_string(),
            )
        } else {
            return Err(format!(
                "certs not found at {ca_path} (hint: run `rex ca` first, or use --node-id for auto-generation)"
            ));
        }
    } else {
        (
            ca_path.to_string(),
            cert_path.to_string(),
            key_path.to_string(),
        )
    };

    // Load server identity + CA for client verification.
    let ca = std::fs::read(&effective_ca).map_err(|e| format!("read ca: {e}"))?;
    let cert_bytes = std::fs::read(&effective_cert).map_err(|e| format!("read cert: {e}"))?;
    let key_bytes = std::fs::read(&effective_key).map_err(|e| format!("read key: {e}"))?;
    let cert: CertificateDer<'static> = rustls_pemfile::certs(&mut &cert_bytes[..])
        .next()
        .ok_or("server cert PEM contained no certificate")?
        .map_err(|e| format!("parse cert: {e}"))?
        .into_owned();
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut &key_bytes[..])
        .map_err(|e| format!("parse key: {e}"))?
        .ok_or("key PEM contained no private key")?;
    let addr: SocketAddr = bind.parse().map_err(|e| format!("bind `{bind}`: {e}"))?;

    // Extract the cert CN for discovery announcements.
    let cert_cn = extract_cn_from_pem(&cert_bytes).unwrap_or_else(|e| {
        d!("could not extract cert CN ({e}), falling back to 'server'");
        "server".to_string()
    });

    // Bus-based discovery: connect first to collect peer CAs, then bind server.
    let mut _bus: Option<Bus> = None;
    let mut peer_ca_pems: Vec<String> = Vec::new();
    if let Some(nid) = node_id {
        let bus_cfg = load_bus_config_from_discovery().unwrap_or_default();
        d!("connecting to bus for discovery (node_id={nid})");
        let mut bus = Bus::from(bus_cfg).await;

        // Briefly subscribe to collect any already-running peer CAs.
        let topic = format!("{DISCOVERY_TOPIC}/**");
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let my_nid = nid.to_string();
        let collect_secret = shared_secret.map(|s| s.to_string());
        bus.subscribe(&topic, move |ann: NodeAnnounce| {
            if ann.node_id == my_nid {
                return;
            }
            if let Some(ref secret) = collect_secret {
                if !ann.verify(secret.as_bytes()) {
                    return;
                }
            }
            if !ann.ca_cert.is_empty() {
                let _ = tx.send(ann.ca_cert.clone());
            }
        })
        .await;
        // Wait for peer CAs — peers re-announce every 5s, so we need to wait
        // at least that long to catch a re-announcement.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                Ok(ca) => {
                    d!("collected peer CA ({} bytes)", ca.len());
                    peer_ca_pems.push(ca);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        d!("collected {} peer CAs from bus", peer_ca_pems.len());
        _bus = Some(bus);
    }

    // Bind QUIC server with local CA + discovered peer CAs.
    let mgr_for_discovery = manager.clone();
    let server = QuicServer::new(manager as Arc<dyn resource::transport::Dispatcher>);
    let extra_cas: Vec<&[u8]> = peer_ca_pems.iter().map(|p| p.as_bytes()).collect();
    let local = server
        .bind(addr, cert, key, &ca, &extra_cas)
        .await
        .map_err(|e| format!("bind: {e}"))?;
    println!("serving {} resources on {}", register.len(), local);

    // If discovery is active, publish announcement and subscribe for ongoing peer discovery.
    if let Some(nid) = node_id {
        let bus = _bus.as_mut().expect("bus already connected");
        let quic_addr = bind.to_string();
        let resources = mgr_for_discovery.list_all().await;
        let ca_pem_str = String::from_utf8_lossy(&ca).to_string();

        // Rich directory: which subsystems this node exposes, plus a load
        // snapshot refreshed on each re-announce.
        let capabilities = vec![
            "stream".to_string(),
            "proc".to_string(),
            "supervisor".to_string(),
            "relay".to_string(),
            "vnode".to_string(),
        ];
        let started = Instant::now();
        let num_resources = resources.len() as u32;
        let mut announce = NodeAnnounce::new(
            nid.to_string(),
            quic_addr,
            cert_cn.clone(),
            resources,
            ca_pem_str,
        )
        .with_capabilities(capabilities)
        .with_load(LoadSnapshot {
            num_resources,
            num_procs: 0,
            uptime_secs: 0,
        });
        if let Some(secret) = shared_secret {
            announce.sign(secret.as_bytes());
            d!("signed discovery announcement with shared secret");
        }
        bus.publish(&discovery_topic(nid), &announce)
            .await
            .map_err(|e| format!("bus publish: {e}"))?;
        d!(
            "published discovery announcement to {}",
            discovery_topic(nid)
        );

        let my_nid = nid.to_string();
        let ca_clone = ca.clone();
        let cert_clone = cert_bytes.clone();
        let key_clone = key_bytes.clone();
        let mgr = mgr_for_discovery.clone();
        let verify_secret = shared_secret.map(|s| s.to_string());

        // Track when each peer last announced, for peer-loss detection.
        let last_seen: Arc<tokio::sync::RwLock<HashMap<String, Instant>>> =
            Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        let last_seen_sub = last_seen.clone();

        // Transport pool: deduped per peer, evicted on peer loss.
        let net = Arc::new(resource::Net::new());
        let net_sub = net.clone();

        let topic = format!("{DISCOVERY_TOPIC}/**");
        bus.subscribe(&topic, move |ann: NodeAnnounce| {
            if ann.node_id == my_nid {
                return;
            }
            if let Some(ref secret) = verify_secret {
                d!(
                    "verifying signature from {} (secret configured)",
                    ann.node_id
                );
                if !ann.verify(secret.as_bytes()) {
                    eprintln!(
                        "discovery: rejecting unsigned/tampered announcement from {}",
                        ann.node_id
                    );
                    return;
                }
                d!("signature valid for {}", ann.node_id);
            }

            // Record that this peer is alive.
            {
                let ls = last_seen_sub.clone();
                let nid = ann.node_id.clone();
                tokio::spawn(async move {
                    ls.write().await.insert(nid, Instant::now());
                });
            }

            d!(
                "discovered peer {} (caps=[{}]) with {} resources (procs={}, uptime={}s)",
                ann.node_id,
                ann.capabilities.join(", "),
                ann.resources.len(),
                ann.load.num_procs,
                ann.load.uptime_secs
            );

            let nid2 = ann.node_id.clone();
            let ca_c = ca_clone.clone();
            let cert_c = cert_clone.clone();
            let key_c = key_clone.clone();
            let mgr_c = mgr.clone();
            let net_c = net_sub.clone();
            tokio::spawn(async move {
                let peer_addr: std::net::SocketAddr = match ann.quic_addr.parse() {
                    Ok(a) => a,
                    Err(e) => {
                        eprintln!("discovery: invalid quic_addr for {nid2}: {e}");
                        return;
                    }
                };
                let server_name = ann.server_name.clone();
                let extra_ca = ann.ca_cert.clone();
                let transport = match net_c
                    .get_or_add(&nid2, || {
                        build_quic_from_bytes(
                            peer_addr,
                            &server_name,
                            &ca_c,
                            &cert_c,
                            &key_c,
                            Some(extra_ca.as_bytes()),
                        )
                        .map(|t| Arc::new(t) as Arc<dyn Transport>)
                        .map_err(ResourceError::Other)
                    })
                    .await
                {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("discovery: failed to build transport for {nid2}: {e}");
                        return;
                    }
                };

                let members: Vec<(String, Arc<dyn Transport>)> = ann
                    .resources
                    .iter()
                    .map(|r| (r.0.clone(), transport.clone()))
                    .collect();

                if members.is_empty() {
                    d!("peer {nid2} has no resources, skipping vnode");
                    return;
                }

                let vnode = Box::new(VirtualNodeResource::new(&nid2, "node", members));
                let vnode_uri = format!("vnode://{nid2}");
                if mgr_c.contains(&vnode_uri).await {
                    d!("vnode://{nid2} already registered, skipping");
                    return;
                }
                match mgr_c.register(vnode, "discovery".into()).await {
                    Ok(()) => d!("registered vnode://{nid2} from peer discovery"),
                    Err(e) => eprintln!("discovery: failed to register vnode://{nid2}: {e}"),
                }
            });
        })
        .await;
        d!("subscribed to {DISCOVERY_TOPIC}/* for peer discovery");

        // Periodic re-announce so late-joining peers discover us. Load is
        // refreshed (uptime + live resource/proc counts) on each tick.
        let re_topic = discovery_topic(nid);
        let mut re_bus = bus.clone();
        let mut re_announce = announce.clone();
        let re_secret = shared_secret.map(|s| s.to_string());
        let re_mgr = mgr_for_discovery.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            interval.tick().await; // skip immediate first fire
            loop {
                interval.tick().await;
                let num_resources = re_mgr.list_all().await.len() as u32;
                re_announce.load = LoadSnapshot {
                    num_resources,
                    num_procs: 0, // proc manager count is not inspected here
                    uptime_secs: started.elapsed().as_secs(),
                };
                if let Some(ref secret) = re_secret {
                    re_announce.sign(secret.as_bytes());
                }
                if re_bus.publish(&re_topic, &re_announce).await.is_err() {
                    break;
                }
            }
        });

        // Peer-loss detection: remove vnodes for peers that haven't announced
        // within PEER_LOST_SECS. Re-announce interval is 5s, so 15s gives
        // reasonable tolerance for temporary delays.
        const PEER_LOST_SECS: u64 = 15;
        let cleanup_mgr = mgr_for_discovery.clone();
        let cleanup_ls = last_seen.clone();
        let cleanup_net = net.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let now = Instant::now();
                let mut ls = cleanup_ls.write().await;
                let stale: Vec<String> = ls
                    .iter()
                    .filter(|(_, ts)| now.duration_since(**ts).as_secs() > PEER_LOST_SECS)
                    .map(|(id, _)| id.clone())
                    .collect();
                for peer_id in &stale {
                    let uri = format!("vnode://{peer_id}");
                    if cleanup_mgr.contains(&uri).await {
                        if let Err(e) = cleanup_mgr.deregister(&uri).await {
                            eprintln!("peer-loss: failed to unregister {uri}: {e}");
                        } else {
                            d!("peer-loss: unregistered {uri} (no announcements for {PEER_LOST_SECS}s)");
                        }
                    }
                    cleanup_net.remove(peer_id).await; // evict pooled transport
                    ls.remove(peer_id);
                }
            }
        });
    }

    d!("accepting QUIC connections (mTLS)…");
    server.run().await.map_err(|e| format!("serve: {e}"))
}

#[tokio::main]
async fn main() {
    let mut cli = Cli::parse();
    init_logging(cli.debug);

    // Load file config (--config, else ./rex.toml, else ./config.toml) and fill
    // any flags the user did not set explicitly.
    let mut file_cfg: Option<RexFileConfig> = None;
    let cfg_path = cli.config.clone().or_else(|| {
        ["rex.toml", "config.toml"]
            .iter()
            .find(|p| Path::new(p).exists())
            .map(|s| s.to_string())
    });
    if let Some(path) = cfg_path {
        d!("loading config from {path}");
        match load_file_config(&path) {
            Ok(Some(cfg)) => {
                d!(
                    "config loaded: node={:?} server={:?} register={:?}",
                    cfg.node,
                    cfg.server,
                    cfg.register
                );
                apply_config(&mut cli, &cfg);
                file_cfg = Some(cfg);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("rex: {e}");
                exit(2);
            }
        }
    } else {
        d!("no config file found (looked for rex.toml, config.toml)");
    }

    match &cli.cmd {
        Cmd::Ca {
            out,
            server_cn,
            client_cn,
        } => {
            if let Err(e) = gen_certs(out, server_cn, client_cn) {
                eprintln!("rex ca: {e}");
                exit(1);
            }
            return;
        }
        Cmd::Serve {
            bind,
            register,
            ca,
            cert,
            key,
            policy,
            node_id,
        } => {
            // Merge node_id from config if still default hostname.
            let effective_node_id = if file_cfg
                .as_ref()
                .and_then(|c| c.node_cfg.id.as_deref())
                .is_some()
            {
                file_cfg
                    .as_ref()
                    .and_then(|c| c.node_cfg.id.as_deref())
                    .unwrap()
            } else {
                node_id
            };
            // Merge bind from config if still default.
            let effective_bind = if bind == "127.0.0.1:4433" {
                file_cfg
                    .as_ref()
                    .and_then(|c| c.node_cfg.bind.as_deref())
                    .unwrap_or(bind)
            } else {
                bind
            };
            // Merge register from config if not on CLI.
            let effective_register: Vec<String> = if register.is_empty() {
                file_cfg
                    .as_ref()
                    .map(|c| c.register.clone())
                    .unwrap_or_default()
            } else {
                register.clone()
            };
            let shared_secret = file_cfg
                .as_ref()
                .and_then(|c| c.discovery.shared_secret.as_deref());
            if let Err(e) = serve(
                effective_bind,
                &effective_register,
                ca,
                cert,
                key,
                policy.as_deref(),
                Some(effective_node_id),
                &cli.server,
                shared_secret,
            )
            .await
            {
                eprintln!("rex serve: {e}");
                exit(1);
            }
            return;
        }
        _ => {}
    }
    let client = match assemble(&cli).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rex: {e}");
            exit(2);
        }
    };
    if let Err(e) = run(&cli.cmd, Explorer::new(client), cli.no_color).await {
        eprintln!("rex: {e}");
        exit(1);
    }
}

async fn run(cmd: &Cmd, explorer: Explorer, no_color: bool) -> resource::Result<()> {
    // Color helpers — no-op when color is disabled or stdout is not a terminal.
    let color = !no_color && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let c = |code: &str, text: &str| -> String {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let kind_icon = |kind: &ResourceType| -> &'static str {
        match kind {
            ResourceType::Storage => "\u{1f4c1}", // 📁
            ResourceType::Combine => "\u{1f517}", // 🔗
            ResourceType::Network => "\u{1f310}", // 🌐
            ResourceType::Compute => "\u{2699}",  // ⚙
            ResourceType::System => "\u{1f4bb}",  // 💻
            ResourceType::Abstract => "\u{2728}", // ✨
        }
    };
    match cmd {
        Cmd::List { kind } => {
            let kind = kind
                .as_deref()
                .map(kind_of)
                .transpose()
                .map_err(ResourceError::Other)?;
            d!("list kind={:?}", kind);
            for info in explorer.describe(kind).await? {
                let kind_str = format!("{}", info.kind);
                let colored_kind = match info.kind {
                    ResourceType::Storage => c("32", &kind_str), // green
                    ResourceType::Combine => c("34", &kind_str), // blue
                    ResourceType::Network => c("33", &kind_str), // yellow
                    ResourceType::Compute => c("36", &kind_str), // cyan
                    _ => kind_str,
                };
                let icon = kind_icon(&info.kind);
                let state_str = format!("{:?}", info.state);
                let colored_state = match info.state {
                    ResourceStateLabel::Open => c("32", &state_str),
                    ResourceStateLabel::Closed => c("90", &state_str),
                    ResourceStateLabel::Error => c("31", &state_str),
                    _ => state_str,
                };
                println!(
                    "  {icon}  {colored_kind:<10} {colored_state:<9} {:<10} {}",
                    info.owner, info.uri
                );
            }
        }
        Cmd::Tree { uri, depth } => {
            d!("tree {uri} depth={depth}");
            let rows = explorer.tree(uri, *depth).await?;
            let len = rows.len();
            for (i, row) in rows.into_iter().enumerate() {
                let is_last = i + 1 == len;
                let prefix = if is_last {
                    "  └── "
                } else {
                    "  ├── "
                };
                match row {
                    Row::Resource { depth, info } => {
                        let indent = "  │  ".repeat(depth.saturating_sub(1));
                        let icon = kind_icon(&info.kind);
                        let kind_str = format!("{}", info.kind);
                        let colored_kind = match info.kind {
                            ResourceType::Storage => c("32", &kind_str),
                            ResourceType::Combine => c("34", &kind_str),
                            ResourceType::Network => c("33", &kind_str),
                            _ => kind_str,
                        };
                        if depth == 0 {
                            println!(
                                "{icon} {} ({}, {})",
                                c("1", &info.uri),
                                colored_kind,
                                format!("{:?}", info.state)
                            );
                        } else {
                            println!(
                                "{indent}{prefix}{icon} {} ({}, {})",
                                c("1", &info.uri),
                                colored_kind,
                                format!("{:?}", info.state)
                            );
                        }
                    }
                    Row::Child { depth, name } => {
                        let indent = "  │  ".repeat(depth.saturating_sub(1));
                        println!("{indent}{prefix}{name}");
                    }
                }
            }
        }
        Cmd::Peek { uri, bytes } => {
            d!("peek {uri} bytes={bytes}");
            let data = explorer.peek(uri, *bytes).await?;
            d!("peek returned {} bytes", data.len());
            println!("{} bytes:", data.len());
            let text = String::from_utf8_lossy(&data);
            if data
                .iter()
                .all(|b| b.is_ascii_graphic() || b.is_ascii_whitespace())
            {
                println!("{text}");
            } else {
                for chunk in data.chunks(16) {
                    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                    println!("  {}", hex.join(" "));
                }
            }
        }
        Cmd::Status { uri } => {
            d!("status {uri}");
            let info = explorer.status(uri).await?;
            println!("uri:   {}", info.uri);
            println!("kind:  {}", info.kind);
            println!("state: {:?}", info.state);
            println!("owner: {}", info.owner);
        }
        Cmd::Watch { uri, events } => {
            d!("watch {uri} events={events:?}");
            let mut stream = explorer.watch(uri, events.clone()).await?;
            while let Some(event) = stream.next().await {
                println!("{event:?}");
            }
        }
        // Handled in `main` before the explorer is built; never reachable here.
        Cmd::Ca { .. } | Cmd::Serve { .. } => {}
    }
    Ok(())
}
