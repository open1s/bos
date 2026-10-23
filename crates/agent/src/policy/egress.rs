//! Egress resolution and the gate that enforces it.
//!
//! A network decision is made on the **resolved IP**, never on the hostname
//! alone. The hostname travels with the decision as a hint for a reader, but a
//! policy that allowed a name would be beaten by a DNS rebind: the name stays
//! the same, the address changes. [`EgressGate`] resolves the name, asks the
//! policy about every resolved address, and only then lets a connection run.
//!
//! `connect` opens the socket to the address the gate validated, and
//! [`PolicyResolver`] plugs the same check into ureq's resolver, which is the
//! one place an HTTP client turns a name into an address. Resolution results are
//! cached for the life of the gate (one turn / one session), so a tool that
//! makes several requests to one host resolves once.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};

use super::set::PolicySet;
use super::target::{Action, Decision, PolicyInput, Target};

/// Resolves a host to socket addresses. Injectable so a test never touches the
/// network.
pub trait Resolver: Send + Sync {
    /// Resolve `host` on `port`.
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>>;
}

/// The operating system's resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        (host, port).to_socket_addrs().map(|addrs| addrs.collect())
    }
}

/// One refusal to connect, carrying the policy decision that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressDenial {
    /// The policy's decision.
    pub decision: Decision,
    /// The name the caller asked for.
    pub host: String,
    /// The resolved address that was refused, when one was.
    pub ip: Option<String>,
}

impl std::fmt::Display for EgressDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.ip {
            Some(ip) => write!(f, "{} ({ip}) was refused", self.host),
            None => write!(f, "{} was refused", self.host),
        }
    }
}

impl std::error::Error for EgressDenial {}

/// Why an egress attempt did not produce a stream.
#[derive(Debug)]
pub enum EgressError {
    /// The policy refused the address.
    Denied(EgressDenial),
    /// The name could not be resolved.
    Resolve(String),
    /// The socket could not be opened.
    Io(std::io::Error),
}

impl std::fmt::Display for EgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EgressError::Denied(denial) => write!(f, "egress denied: {denial}"),
            EgressError::Resolve(message) => write!(f, "egress resolve failed: {message}"),
            EgressError::Io(error) => write!(f, "egress connect failed: {error}"),
        }
    }
}

impl std::error::Error for EgressError {}

/// A cached resolution, either allowed addresses or the refusal that stopped it.
#[derive(Clone)]
struct Cached {
    addrs: Vec<SocketAddr>,
    denial: Option<EgressDenial>,
}

/// Resolves, checks and only then connects. One gate per turn.
pub struct EgressGate {
    policy: Arc<PolicySet>,
    resolver: Arc<dyn Resolver>,
    session: String,
    preset: String,
    tool: String,
    cache: Mutex<HashMap<String, Cached>>,
}

impl EgressGate {
    /// Build a gate for `tool` in `session`/`preset` over `policy`.
    pub fn new(
        policy: Arc<PolicySet>,
        resolver: Arc<dyn Resolver>,
        session: impl Into<String>,
        preset: impl Into<String>,
        tool: impl Into<String>,
    ) -> Self {
        Self {
            policy,
            resolver,
            session: session.into(),
            preset: preset.into(),
            tool: tool.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The policy this gate asks.
    pub fn policy(&self) -> &Arc<PolicySet> {
        &self.policy
    }

    /// The session this gate decides for.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The preset this gate decides for.
    pub fn preset(&self) -> &str {
        &self.preset
    }

    /// The tool name this gate decides for.
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Resolve `host` and allow every address only if the policy allows it.
    ///
    /// All resolved addresses must be allowed. A name that resolves to one
    /// allowlisted address and one unknown address is refused, because the
    /// client could pick the unknown one.
    pub fn authorize(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, EgressDenial> {
        let key = format!("{host}:{port}");
        if let Ok(cache) = self.cache.lock() {
            if let Some(cached) = cache.get(&key) {
                return match &cached.denial {
                    Some(denial) => Err(denial.clone()),
                    None => Ok(cached.addrs.clone()),
                };
            }
        }

        let resolved = match self.resolver.resolve(host, port) {
            Ok(addrs) if !addrs.is_empty() => addrs,
            Ok(_) => Vec::new(),
            Err(error) => {
                let denial = EgressDenial {
                    decision: Decision::deny(
                        "egress-unresolved",
                        format!("the host could not be resolved: {error}"),
                    ),
                    host: host.to_string(),
                    ip: None,
                };
                self.cache_denial(&key, &denial);
                return Err(denial);
            }
        };

        if resolved.is_empty() {
            let denial = EgressDenial {
                decision: Decision::deny("egress-unresolved", "the host resolved to no addresses"),
                host: host.to_string(),
                ip: None,
            };
            self.cache_denial(&key, &denial);
            return Err(denial);
        }

        let mut allowed = Vec::new();
        for addr in resolved {
            let input = PolicyInput::new(
                self.tool.clone(),
                Action::Egress,
                Target::host_on(host, addr.ip().to_string(), port),
            )
            .with_session(self.session.clone())
            .with_preset(self.preset.clone());
            let decision = self.policy.check(&input);
            if decision.is_denied() {
                let denial = EgressDenial {
                    decision,
                    host: host.to_string(),
                    ip: Some(addr.ip().to_string()),
                };
                self.cache_denial(&key, &denial);
                return Err(denial);
            }
            allowed.push(addr);
        }

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                key,
                Cached {
                    addrs: allowed.clone(),
                    denial: None,
                },
            );
        }
        Ok(allowed)
    }

    /// Open a TCP connection to the first address the policy allowed.
    ///
    /// The socket targets the validated address, not the name, so a resolver
    /// that changes its answer between the check and the connect cannot move
    /// the connection.
    pub fn connect(&self, host: &str, port: u16) -> Result<TcpStream, EgressError> {
        let addrs = self.authorize(host, port).map_err(EgressError::Denied)?;
        let mut last_error = None;
        for addr in addrs {
            match TcpStream::connect(addr) {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = Some(error),
            }
        }
        Err(EgressError::Io(last_error.unwrap_or_else(|| {
            std::io::Error::other("no allowed address to connect to")
        })))
    }

    /// The refusal cached for `host`/`port`, if the gate already refused it.
    pub fn denial(&self, host: &str, port: u16) -> Option<EgressDenial> {
        let key = format!("{host}:{port}");
        self.cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key).and_then(|cached| cached.denial.clone()))
    }

    fn cache_denial(&self, key: &str, denial: &EgressDenial) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                key.to_string(),
                Cached {
                    addrs: Vec::new(),
                    denial: Some(denial.clone()),
                },
            );
        }
    }
}

impl std::fmt::Debug for EgressGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressGate")
            .field("session", &self.session)
            .field("preset", &self.preset)
            .field("tool", &self.tool)
            .finish_non_exhaustive()
    }
}

/// The ureq resolver that makes [`EgressGate`] the only path from a name to an
/// address.
///
/// ureq calls this at the point it would normally call `getaddrinfo`; refusing
/// here means no socket is opened at all. Because the returned addresses are
/// the ones the policy approved, the later connect cannot be rebound to another
/// address.
#[derive(Debug, Clone)]
pub struct PolicyResolver {
    gate: Arc<EgressGate>,
}

impl PolicyResolver {
    /// Resolve through `gate`.
    pub fn new(gate: Arc<EgressGate>) -> Self {
        Self { gate }
    }

    /// The gate behind this resolver.
    pub fn gate(&self) -> &Arc<EgressGate> {
        &self.gate
    }
}

impl ureq::unversioned::resolver::Resolver for PolicyResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        _config: &ureq::config::Config,
        _timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let Some(host) = uri.host() else {
            return Err(ureq::Error::HostNotFound);
        };
        // `Uri::host` keeps the brackets around an IPv6 literal; `ToSocketAddrs`
        // wants the bare address.
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        let port = uri.port_u16().unwrap_or_else(|| {
            if uri.scheme_str() == Some("https") {
                443
            } else {
                80
            }
        });
        let addrs = self
            .gate
            .authorize(host, port)
            .map_err(|_| ureq::Error::HostNotFound)?;
        let mut out = <Self as ureq::unversioned::resolver::Resolver>::empty(self);
        // The resolver's array holds at most 16 addresses; a name that resolves
        // to more still connects to one of the approved first sixteen.
        for addr in addrs.into_iter().take(16) {
            out.push(addr);
        }
        Ok(out)
    }
}

/// The host and port a URL names, for a transport that must check before it
/// builds an agent. Returns none for a URL with no host.
pub fn host_and_port(url: &str) -> Option<(String, u16)> {
    let uri: ureq::http::Uri = url.parse().ok()?;
    let host = uri.host()?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .to_string();
    let port = uri.port_u16().unwrap_or_else(|| {
        if uri.scheme_str() == Some("https") {
            443
        } else {
            80
        }
    });
    Some((host, port))
}
