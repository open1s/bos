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
//! # One vocabulary for a network rule
//!
//! A network rule list is written by a human in two places - the workspace file
//! `.bsh/egress-allowlist` and a preset's own `egress` field - and both must
//! mean the same three things. [`EgressRuleSet`] is that one parser and that one
//! vocabulary: a bare integer is a PORT, something that parses as an address or
//! a CIDR range is an ADDRESS rule (the strong door, decided on the resolved
//! IP), and anything else is a HOSTNAME rule (the weaker door, decided on the
//! name). The workspace file, the preset field and the tests all classify
//! through it, so a rule cannot mean one thing in the file and another in a
//! preset.

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

    /// Resolve, allow the addresses, then decide the URL PATH as well.
    ///
    /// This is the plain-HTTP door: a forward-proxy request carries the
    /// absolute URI, so the transport can hand the path to the SAME policy the
    /// address check asked. It is deliberately a separate entry point rather
    /// than a parameter on [`EgressGate::authorize`], because a caller that does
    /// not see a path must not pretend it enforced one.
    ///
    /// The address decision is the cached one; the path decision is not cached,
    /// because it is a fact about one URL rather than about a host. A policy
    /// with no path rule performs no second evaluation at all.
    pub fn authorize_path(
        &self,
        host: &str,
        port: u16,
        path: &str,
    ) -> Result<Vec<SocketAddr>, EgressDenial> {
        let addrs = self.authorize(host, port)?;
        if self.policy.data().egress_deny_paths.is_empty() {
            return Ok(addrs);
        }
        for addr in &addrs {
            let input = PolicyInput::new(
                self.tool.clone(),
                Action::Egress,
                Target::host_path_on(host, addr.ip().to_string(), port, path),
            )
            .with_session(self.session.clone())
            .with_preset(self.preset.clone());
            let decision = self.policy.check(&input);
            if decision.is_denied() {
                return Err(EgressDenial {
                    decision,
                    host: host.to_string(),
                    ip: Some(addr.ip().to_string()),
                });
            }
        }
        Ok(addrs)
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

/// One classified set of network rules: the three shapes a rule list may carry.
///
/// This is the ONE parser for a human-written network rule list. The workspace
/// file `.bsh/egress-allowlist` and a preset's `egress` field both read through
/// it, so a rule cannot mean one thing in the file and another in a preset.
///
/// A line is classified by what it can be, in this order:
///
/// - a bare integer is a destination PORT (`443`). No port rule means any port;
///   one or more means only those.
/// - something that parses as an address or as a CIDR range is an ADDRESS rule
///   (`203.0.113.7`, `10.0.0.0/8`). This is the strong door: the policy decides
///   on the resolved address, so a DNS rebind cannot ride it.
/// - anything else is a HOSTNAME rule (`api.example.com`, `*.crates.io`). This
///   is the weaker door - it survives a rebind - and it exists for a policy that
///   names a service rather than an address.
///
/// An empty set is not the same as no set: a caller that receives one has a
/// declared, empty rule list, which denies every address, host and port. The
/// distinction between “absent” and “empty” is not expressible here, so it
/// lives in the caller: bsh's file reader reports absent vs present-but-empty
/// and mirrors it as `PolicyData::egress_unrestricted`, while a preset's own
/// field is absent vs declared in `preset_egress_restricted`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressRuleSet {
    /// Address rules: an exact address or a CIDR range.
    pub addresses: Vec<String>,
    /// Hostname rules, lower-cased. A leading `*.` matches any subdomain.
    pub hosts: Vec<String>,
    /// Destination ports. Empty means any port.
    pub ports: Vec<u16>,
}

impl EgressRuleSet {
    /// Classify every entry of a rule list. Comments, blank lines and a
    /// trailing `#` comment on a line are ignored, so the same text a file
    /// carries can be handed in directly.
    pub fn parse<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = Self::default();
        for raw in entries {
            let text = raw.as_ref();
            for line in text.lines() {
                let entry = line.split('#').next().unwrap_or("").trim();
                set.push(entry);
            }
        }
        set
    }

    /// Classify ONE already-trimmed entry, appending it to the right list.
    ///
    /// An empty entry is ignored; every other entry lands in exactly one list,
    /// because the three tests are ordered: a port first, then an address, and
    /// a hostname as the fallback.
    pub fn push(&mut self, entry: &str) {
        if entry.is_empty() {
            return;
        }
        if let Ok(port) = entry.parse::<u16>() {
            self.ports.push(port);
            return;
        }
        let base = entry.split('/').next().unwrap_or(entry);
        let is_address = entry.parse::<std::net::IpAddr>().is_ok()
            || (entry.contains('/') && base.parse::<std::net::IpAddr>().is_ok());
        if is_address {
            self.addresses.push(entry.to_string());
        } else {
            self.hosts.push(normalize_hostname(entry));
        }
    }

    /// Whether the set declares no rule at all, which denies everything.
    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty() && self.hosts.is_empty() && self.ports.is_empty()
    }
}

/// Lower-case a hostname rule and drop a trailing dot, so it matches the name
/// the policy later compares it against.
fn normalize_hostname(entry: &str) -> String {
    entry.trim().trim_end_matches('.').to_ascii_lowercase()
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
