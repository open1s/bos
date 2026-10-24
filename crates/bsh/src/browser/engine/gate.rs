//! The egress gate the engine process goes through: a loopback HTTP proxy.
//!
//! # Why a proxy, and not the CDP Fetch domain
//!
//! The Fetch domain LOOKS like the right seam: `Fetch.requestPaused` names
//! every request, and `Fetch.failRequest` refuses it. MEASURED on obscura
//! 0.2.5 (docs/bsh-browser-substrate.md has the whole experiment), it is not a
//! gate: the engine's own fetcher reaches the origin BEFORE the pause is
//! emitted. A page served with a 1.2 s delay produced this order -
//! `[3138 SOCKET] [3139 REQ /slow]` and then NOTHING, because the pause only
//! arrives once the response has been fetched. A refused URL had already
//! opened its socket, so the Fetch domain cannot carry the one property this
//! pane needs: a refusal produces ZERO origin sockets.
//!
//! The seam that DOES run before the socket is `--proxy`: Obscura's fetcher
//! honours it for the document AND for every subresource (measured: a page with
//! an image, a script and a stylesheet produced four proxied requests and zero
//! direct ones). Pointing it at this module makes every request ask
//! `EgressGate::authorize` - the SAME gate the shell tool and the MCP
//! transport ask - before any origin socket exists.
//!
//! # What is proved, and what is not
//!
//! Proved by the tests in `super::tests`:
//!
//! - a refused URL produces NO request: a LIVE loopback listener stays at 0
//!   hits while the refusal carries the policy's own rule and prose;
//! - discrimination: an allowed URL moves that SAME listener's counter to >= 1,
//!   so the zero cannot pass by a gate that refuses everything;
//! - a subresource case: a page whose image comes from the refused origin loads
//!   its allowed parts while the refused origin stays at 0 hits.
//!
//! What the path door covers, and what it cannot: a PLAIN HTTP forward-proxy
//! request carries the absolute URI, so the proxy hands HOST, PORT **and PATH**
//! to the same policy before it connects and a path rule refuses with 403 (the
//! rule and the policy's prose) and zero origin sockets. A `CONNECT` tunnel
//! (https) carries only `host:port`: TLS is not terminated here, so a rule on a
//! path inside an encrypted page cannot be decided at the socket. The reader's
//! explicit choice, [`crate::browser::NetworkMode`], is what closes that hole:
//! `Proxied` accepts the host:port granularity for https, and `Strict` refuses
//! the tunnel outright so nothing traverses an un-terminated session.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agent::policy::egress::host_and_port;
use agent::policy::{EgressGate, PolicySet, SystemResolver};

use crate::browser::NetworkMode;

/// The rule id a Strict-mode refusal carries, in both the pane and a tool.
pub(crate) const STRICT_RULE: &str = "browser-strict-network";

/// The most request-header bytes a forwarded request may carry.
const HEADER_CAP: usize = 64 * 1024;
/// How long a relayed socket may sit idle before it is closed.
const RELAY_IDLE: Duration = Duration::from_secs(30);

/// One policy refusal, in the terms the pane and a tool can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Denial {
    /// The URL that was refused.
    pub(crate) url: String,
    /// The host the policy refused.
    pub(crate) host: String,
    /// The port the policy refused.
    pub(crate) port: u16,
    /// The rule that refused, when the policy named one.
    pub(crate) rule: Option<String>,
    /// The policy's own prose.
    pub(crate) message: String,
}

/// The gate every engine request passes, and the count of what it saw.
pub(crate) struct Gate {
    /// The port the proxy listens on.
    port: u16,
    /// The policy the current request asks. Replaced on every navigation so the
    /// proxy asks the SAME per-turn gate the tools ask, not a copy.
    policy: Arc<Mutex<Option<Arc<EgressGate>>>>,
    /// The gate a launch starts with, used until the first navigation names one.
    fallback: Arc<EgressGate>,
    /// Requests the gate authorized.
    allowed: Arc<AtomicU64>,
    /// Requests the gate refused.
    refused: Arc<AtomicU64>,
    /// The most recent refusal.
    last: Arc<Mutex<Option<Denial>>>,
    /// Whether https is refused outright ([`NetworkMode::Strict`]).
    strict: Arc<AtomicBool>,
    /// Whether the accept loop should keep running.
    running: Arc<AtomicBool>,
    /// The accept thread.
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gate")
            .field("port", &self.port)
            .field("allowed", &self.allowed())
            .field("refused", &self.refused())
            .finish_non_exhaustive()
    }
}

impl Gate {
    /// Bind a loopback proxy for `policy`.
    pub(crate) fn start(
        policy: Arc<PolicySet>,
        session: &str,
        preset: &str,
        mode: NetworkMode,
    ) -> Result<Arc<Self>, String> {
        let fallback = Arc::new(EgressGate::new(
            policy,
            Arc::new(SystemResolver),
            session,
            preset,
            "browser_render",
        ));
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("the browser gate could not bind a loopback port: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("the browser gate has no local address: {error}"))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("the browser gate could not be tuned: {error}"))?;
        let gate = Arc::new(Self {
            port,
            policy: Arc::new(Mutex::new(None)),
            fallback,
            allowed: Arc::new(AtomicU64::new(0)),
            refused: Arc::new(AtomicU64::new(0)),
            last: Arc::new(Mutex::new(None)),
            strict: Arc::new(AtomicBool::new(matches!(mode, NetworkMode::Strict))),
            running: Arc::new(AtomicBool::new(true)),
            thread: Mutex::new(None),
        });
        let running = gate.running.clone();
        let policy = gate.policy.clone();
        let fallback = gate.fallback.clone();
        let allowed = gate.allowed.clone();
        let refused = gate.refused.clone();
        let last = gate.last.clone();
        let strict = gate.strict.clone();
        let worker = thread::Builder::new()
            .name(String::from("bsh-browser-gate"))
            .spawn(move || {
                while running.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let policy = policy.clone();
                            let fallback = fallback.clone();
                            let allowed = allowed.clone();
                            let refused = refused.clone();
                            let last = last.clone();
                            let strict = strict.clone();
                            let _ = thread::Builder::new()
                                .name(String::from("bsh-browser-gate-conn"))
                                .spawn(move || {
                                    let _ = serve(
                                        stream, &policy, &fallback, &allowed, &refused, &last,
                                        &strict,
                                    );
                                });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| format!("the browser gate's thread did not start: {error}"))?;
        if let Ok(mut slot) = gate.thread.lock() {
            slot.replace(worker);
        }
        Ok(gate)
    }

    /// The loopback port the engine's `--proxy` points at.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// The gate a request asks.
    fn current(&self) -> Arc<EgressGate> {
        self.policy
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(|| self.fallback.clone())
    }

    /// Switch the network mode for every later request.
    pub(crate) fn set_mode(&self, mode: NetworkMode) {
        self.strict
            .store(matches!(mode, NetworkMode::Strict), Ordering::Relaxed);
    }

    /// Make every later request ask `gate` - the turn's own gate.
    pub(crate) fn set_gate(&self, gate: Arc<EgressGate>) {
        if let Ok(mut slot) = self.policy.lock() {
            slot.replace(gate);
        }
    }

    /// Authorize a host, a port AND a URL path, counting the decision.
    ///
    /// This is the plain-HTTP door: the proxy has the absolute request URI, so
    /// it can hand the path to the SAME policy the address check asked. A
    /// CONNECT tunnel has no path and must not ask this question.
    pub(crate) fn authorize_path(&self, host: &str, port: u16, path: &str) -> Result<(), Denial> {
        match self.current().authorize_path(host, port, path) {
            Ok(_) => {
                self.allowed.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(denial) => {
                self.refused.fetch_add(1, Ordering::Relaxed);
                let refusal = Denial {
                    url: format!("{host}:{port}{path}"),
                    host: host.to_string(),
                    port,
                    rule: denial.decision.rule.clone(),
                    message: self.current().policy().refusal(&denial.decision),
                };
                if let Ok(mut slot) = self.last.lock() {
                    slot.replace(refusal.clone());
                }
                Err(refusal)
            }
        }
    }

    /// Authorize a URL, which is what the Fetch seam and a navigation have.
    pub(crate) fn authorize_url(&self, url: &str) -> Result<(), Denial> {
        let Some((host, port)) = host_and_port(url) else {
            let denial = Denial {
                url: url.to_string(),
                host: String::new(),
                port: 0,
                rule: None,
                message: format!(
                    "{url} is not an http(s) URL, so the browser gate cannot authorize it"
                ),
            };
            self.refused.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut slot) = self.last.lock() {
                slot.replace(denial.clone());
            }
            return Err(denial);
        };
        // The Fetch seam sees the full URL too, so its delivery refusal uses
        // the same path door as the proxy. It is still delivery control, not
        // the socket guarantee.
        self.authorize_path(&host, port, &url_path(url))
            .map_err(|mut denial| {
                denial.url = url.to_string();
                denial
            })
    }

    /// Authorize a URL against a SPECIFIC gate, without counting it.
    ///
    /// This is the navigation pre-flight: the answer has to exist before
    /// `Page.navigate` does, and it must not be counted as a request because
    /// no request was made. The process's own proxy counts the requests that
    /// actually happen.
    pub(crate) fn preflight(gate: &EgressGate, url: &str, mode: NetworkMode) -> Result<(), Denial> {
        // Strict refuses the tunnel before Page.navigate, so no document
        // request exists at all and the reader gets the rule and the sentence
        // rather than a page that silently failed to load.
        if matches!(mode, NetworkMode::Strict) && is_https(url) {
            let (host, port) = host_and_port(url).unwrap_or_default();
            return Err(Denial {
                url: url.to_string(),
                host,
                port,
                rule: Some(STRICT_RULE.to_string()),
                message: strict_message(url),
            });
        }
        let Some((host, port)) = host_and_port(url) else {
            return Err(Denial {
                url: url.to_string(),
                host: String::new(),
                port: 0,
                rule: None,
                message: format!(
                    "{url} is not an http(s) URL, so the browser gate cannot authorize it"
                ),
            });
        };
        gate.authorize_path(&host, port, &url_path(url))
            .map(|_| ())
            .map_err(|denial| Denial {
                url: url.to_string(),
                host,
                port,
                rule: denial.decision.rule.clone(),
                message: gate.policy().refusal(&denial.decision),
            })
    }

    /// Requests the gate authorized.
    pub(crate) fn allowed(&self) -> u64 {
        self.allowed.load(Ordering::Relaxed)
    }

    /// Requests the gate refused.
    pub(crate) fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// The most recent refusal.
    pub(crate) fn last(&self) -> Option<Denial> {
        self.last.lock().ok().and_then(|slot| slot.clone())
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = thread.join();
        }
    }
}

/// Answer one proxied connection.
fn serve(
    mut client: TcpStream,
    policy: &Arc<Mutex<Option<Arc<EgressGate>>>>,
    fallback: &Arc<EgressGate>,
    allowed: &AtomicU64,
    refused: &AtomicU64,
    last: &Mutex<Option<Denial>>,
    strict: &AtomicBool,
) -> std::io::Result<()> {
    // The listener is nonblocking so the accept loop can be stopped; on this
    // platform an accepted socket can inherit that flag. One request per
    // connection is served with a read TIMEOUT, so blocking mode is what the
    // header read below actually expects.
    client.set_nonblocking(false)?;
    client.set_read_timeout(Some(RELAY_IDLE))?;
    let head = match read_head(&mut client) {
        Ok(head) => head,
        Err(_) => return Ok(()),
    };
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let gate = policy
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| fallback.clone());

    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = split_authority(target) else {
            return refuse(
                &mut client,
                &simple_denial(target, "the tunnel target has no port"),
                refused,
                last,
            );
        };
        // Strict: an https page is a CONNECT tunnel, and this pane does not
        // terminate TLS. Refusing here is the ONE place a path rule could
        // otherwise be silently unenforced inside an encrypted page.
        if strict.load(Ordering::Relaxed) {
            let denial = Denial {
                url: format!("{host}:{port}"),
                host: host.clone(),
                port,
                rule: Some(STRICT_RULE.to_string()),
                message: strict_message(target),
            };
            refused.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut slot) = last.lock() {
                slot.replace(denial.clone());
            }
            return refuse(&mut client, &denial, refused, last);
        }
        let addresses = match gate.authorize(&host, port) {
            Ok(addresses) => {
                allowed.fetch_add(1, Ordering::Relaxed);
                addresses
            }
            Err(denial) => {
                refused.fetch_add(1, Ordering::Relaxed);
                let refusal = Denial {
                    url: format!("{host}:{port}"),
                    host: host.clone(),
                    port,
                    rule: denial.decision.rule.clone(),
                    message: gate.policy().refusal(&denial.decision),
                };
                if let Ok(mut slot) = last.lock() {
                    slot.replace(refusal.clone());
                }
                let _ = client.write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                return Ok(());
            }
        };
        let Some(origin) = connect_to(&addresses) else {
            let _ = client.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return Ok(());
        };
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
        relay(client, origin);
        return Ok(());
    }

    // A forward-proxy request carries an absolute URI. An origin-form request
    // is accepted too, built onto its Host header, so a client that forgot the
    // proxy form still passes the gate rather than bypassing it.
    let target = if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else {
        let host = text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("host")
                    .then(|| value.trim().to_string())
            })
            .unwrap_or_default();
        if host.is_empty() {
            return refuse(
                &mut client,
                &simple_denial(target, "the request names no host"),
                refused,
                last,
            );
        }
        format!("http://{host}{target}")
    };
    let Some((host, port)) = host_and_port(&target) else {
        return refuse(
            &mut client,
            &simple_denial(&target, "the request is not an http(s) URL"),
            refused,
            last,
        );
    };
    // The absolute request URI carries the PATH, so a plain HTTP request is
    // authorized on host + port + path before any origin socket exists.
    let path = origin_form(&target);
    let addresses = match gate.authorize_path(&host, port, &path) {
        Ok(addresses) => {
            allowed.fetch_add(1, Ordering::Relaxed);
            addresses
        }
        Err(denial) => {
            refused.fetch_add(1, Ordering::Relaxed);
            let refusal = Denial {
                url: target.clone(),
                host: host.clone(),
                port,
                rule: denial.decision.rule.clone(),
                message: gate.policy().refusal(&denial.decision),
            };
            if let Ok(mut slot) = last.lock() {
                slot.replace(refusal.clone());
            }
            return refuse(&mut client, &refusal, refused, last);
        }
    };
    let Some(mut origin) = connect_to(&addresses) else {
        let _ = client.write_all(
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return Ok(());
    };
    origin.set_read_timeout(Some(RELAY_IDLE))?;

    // Rewrite the request line to origin form and force one request per
    // connection, so the response boundary is the origin's close and not a
    // Content-Length this proxy would have to parse.
    let path = origin_form(&target);
    let mut forwarded = format!("{method} {path} HTTP/1.1\r\n");
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        let (name, _) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        if name.eq_ignore_ascii_case("connection")
            || name.eq_ignore_ascii_case("proxy-connection")
            || name.eq_ignore_ascii_case("keep-alive")
        {
            continue;
        }
        forwarded.push_str(line);
        forwarded.push_str("\r\n");
    }
    forwarded.push_str("Connection: close\r\n\r\n");
    origin.write_all(forwarded.as_bytes())?;
    relay(client, origin);
    Ok(())
}

/// Read one request-header block, bounded, from the socket itself.
///
/// A `BufReader` over a cloned fd was tried and abandoned: on this platform
/// the clone's first read did not see bytes the peer had already written until
/// the client gave up, which made every refusal look like a hang. Reading the
/// header byte by byte stops exactly at the blank line, so the body that
/// follows is still in the socket for the relay to copy.
fn read_head(reader: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while head.len() < HEADER_CAP {
        let read = reader.read(&mut byte)?;
        if read == 0 {
            break;
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            return Ok(head);
        }
    }
    Ok(head)
}

/// Split `host:port`.
fn split_authority(authority: &str) -> Option<(String, u16)> {
    let (host, port) = authority.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
}

/// The origin-form path (and query) of an absolute URL.
fn origin_form(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    match (rest.find('/'), rest.find('?')) {
        (Some(path), Some(query)) => rest[path.min(query)..].to_string(),
        (Some(path), None) => rest[path..].to_string(),
        (None, Some(query)) => format!("/{}", &rest[query..]),
        (None, None) => String::from("/"),
    }
}

/// The path a navigation URL asks for, in the same origin form the proxy sees.
fn url_path(url: &str) -> String {
    origin_form(url)
}

/// Whether a URL is the https scheme.
fn is_https(url: &str) -> bool {
    url.len() >= 8 && url[..8].eq_ignore_ascii_case("https://")
}

/// The sentence a Strict-mode refusal carries.
///
/// The refusal names the reader's own choice, so it is actionable: either the
/// mode changes or the URL does. It does not pretend the page could be checked.
pub(crate) fn strict_message(url: &str) -> String {
    format!(
        "Strict browser network mode refused {url}: an https page is an encrypted tunnel,          and this pane does not terminate TLS, so a rule on the path inside it cannot be          enforced before the socket. Switch Browser network to Proxied to allow https, or          open an http URL."
    )
}

/// Connect to the first address the policy allowed.
fn connect_to(addresses: &[std::net::SocketAddr]) -> Option<TcpStream> {
    for address in addresses {
        if let Ok(stream) = TcpStream::connect_timeout(address, RELAY_IDLE) {
            return Some(stream);
        }
    }
    None
}

/// A refusal with no policy behind it (a malformed request).
fn simple_denial(url: &str, message: &str) -> Denial {
    Denial {
        url: url.to_string(),
        host: String::new(),
        port: 0,
        rule: None,
        message: message.to_string(),
    }
}

/// Answer with the policy's own prose and count nothing twice.
fn refuse(
    client: &mut TcpStream,
    denial: &Denial,
    _refused: &AtomicU64,
    _last: &Mutex<Option<Denial>>,
) -> std::io::Result<()> {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Refused</title>\
         <h1>Refused by policy</h1><p>{}</p><p>{}</p>",
        escape(&denial.message),
        denial
            .rule
            .as_deref()
            .map(|rule| format!("rule: {}", escape(rule)))
            .unwrap_or_default()
    );
    let response = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes())
}

/// HTML-escape the policy's prose, which is text and must stay text.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Pump bytes both ways until either side closes.
fn relay(client: TcpStream, origin: TcpStream) {
    let (mut client_read, mut client_write) = match (client.try_clone(), client) {
        (Ok(read), write) => (read, write),
        _ => return,
    };
    let (mut origin_read, mut origin_write) = match (origin.try_clone(), origin) {
        (Ok(read), write) => (read, write),
        _ => return,
    };
    let upstream = thread::spawn(move || {
        let _ = std::io::copy(&mut client_read, &mut origin_write);
        let _ = origin_write.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut origin_read, &mut client_write);
    let _ = client_write.shutdown(Shutdown::Write);
    let _ = upstream.join();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A loopback origin that answers every request with one small page and
    /// counts the sockets it saw.
    struct Origin {
        port: u16,
        hits: Arc<AtomicUsize>,
    }

    impl Origin {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            let port = listener.local_addr().expect("the port").port();
            let hits = Arc::new(AtomicUsize::new(0));
            let counter = hits.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut buffer = [0u8; 4096];
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        continue;
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    let body = "<html><title>origin</title></html>";
                    let response = format!(
                        "HTTP/1.1 200 OK
Content-Type: text/html
Content-Length: {}
Connection: close

{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self { port, hits }
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    /// Send one request through the proxy and read its whole answer.
    fn through_proxy(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the proxy accepts");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a timeout");
        stream
            .write_all(request.as_bytes())
            .expect("the request is written");
        // Read the header block, then exactly the announced body. A tunnel's
        // 200 has no body, so this returns without waiting for the relay to go
        // idle; a refusal page names its rule and prose in the body.
        let mut answer = Vec::new();
        let mut byte = [0u8; 1];
        while answer.len() < 8192 {
            match stream.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    answer.push(byte[0]);
                    if answer.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let head = String::from_utf8_lossy(&answer).to_ascii_lowercase();
        let length = head.lines().find_map(|line| {
            line.strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        });
        if let Some(length) = length {
            let mut body = vec![0u8; length.min(64 * 1024)];
            let _ = stream.read_exact(&mut body);
            answer.extend_from_slice(&body);
        }
        String::from_utf8_lossy(&answer).to_string()
    }

    fn gate(port: u16, deny_paths: &[&str], mode: NetworkMode) -> Arc<Gate> {
        let policy = PolicySet::compile_baseline(
            agent::policy::PolicyData::new("/tmp")
                .with_egress_hosts(["127.0.0.1"])
                .with_egress_ports([port])
                .with_egress_deny_paths(deny_paths.iter().copied()),
        );
        Gate::start(Arc::new(policy), "session-test", "preset-test", mode)
            .expect("the loopback gate binds")
    }

    #[test]
    fn an_absolute_url_becomes_an_origin_form_path() {
        assert_eq!(origin_form("http://127.0.0.1:9/a/b?c=1"), "/a/b?c=1");
        assert_eq!(origin_form("https://example.com"), "/");
        assert_eq!(origin_form("https://example.com/x"), "/x");
        assert_eq!(origin_form("http://example.com?q=1"), "/?q=1");
    }

    /// GAP 1's proof without an engine: the proxy itself refuses a denied PATH
    /// with 403 carrying the rule, and the origin's live listener stays at ZERO.
    /// The same origin on an allowed path then answers 200 - the discrimination
    /// that makes the zero mean something.
    #[test]
    fn a_denied_http_path_opens_no_origin_socket_and_an_allowed_one_does() {
        let origin = Origin::start();
        let proxy = gate(origin.port, &["/secret/"], NetworkMode::Proxied);

        let refused = through_proxy(
            proxy.port(),
            &format!(
                "GET http://127.0.0.1:{}/secret/notes.txt HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                origin.port, origin.port
            ),
        );
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
        assert!(refused.contains("egress-path-denied"), "{refused}");
        assert!(
            refused.to_lowercase().contains("deny list"),
            "the refusal must carry the policy's prose: {refused}"
        );
        assert_eq!(
            origin.hits(),
            0,
            "a denied path must not open an origin socket"
        );
        assert!(proxy.refused() >= 1, "the gate records the refusal");
        assert_eq!(
            proxy.last().and_then(|denial| denial.rule).as_deref(),
            Some("egress-path-denied")
        );

        let allowed = through_proxy(
            proxy.port(),
            &format!(
                "GET http://127.0.0.1:{}/public/index.html HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                origin.port, origin.port
            ),
        );
        assert!(allowed.starts_with("HTTP/1.1 200"), "{allowed}");
        assert_eq!(
            origin.hits(),
            1,
            "an allowed path on the SAME origin must reach it"
        );
    }

    /// Strict mode is a CHOICE, not silent behaviour: the SAME gate that opened
    /// a tunnel in Proxied refuses one after the switch, naming the rule, and
    /// still lets plain http through.
    #[test]
    fn strict_mode_refuses_the_connect_tunnel_and_keeps_http_working() {
        let origin = Origin::start();
        let proxy = gate(origin.port, &[], NetworkMode::Proxied);
        let connect = format!(
            "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            origin.port, origin.port
        );
        let proxied = through_proxy(proxy.port(), &connect);
        assert!(
            proxied.starts_with("HTTP/1.1 200 Connection Established"),
            "Proxied must tunnel: {proxied}"
        );

        proxy.set_mode(NetworkMode::Strict);
        let strict = through_proxy(proxy.port(), &connect);
        assert!(strict.starts_with("HTTP/1.1 403"), "{strict}");
        assert!(strict.contains(STRICT_RULE), "{strict}");
        assert!(
            strict.contains("does not terminate TLS"),
            "the sentence must say why: {strict}"
        );
        assert_eq!(
            proxy.last().and_then(|denial| denial.rule).as_deref(),
            Some(STRICT_RULE)
        );

        // Plain http still works in Strict: only the un-terminated tunnel is
        // refused, which is what makes this a policy and not a kill switch.
        let before = origin.hits();
        let http = through_proxy(
            proxy.port(),
            &format!(
                "GET http://127.0.0.1:{}/plain HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                origin.port, origin.port
            ),
        );
        assert!(http.starts_with("HTTP/1.1 200"), "{http}");
        assert_eq!(origin.hits(), before + 1);
    }

    #[test]
    fn preflight_names_the_strict_rule_for_https_and_lets_http_through() {
        let origin = Origin::start();
        let policy = Arc::new(PolicySet::compile_baseline(
            agent::policy::PolicyData::new("/tmp")
                .with_egress_hosts(["127.0.0.1"])
                .with_egress_ports([origin.port]),
        ));
        let gate = EgressGate::new(
            policy,
            Arc::new(agent::policy::SystemResolver),
            "s",
            "p",
            "t",
        );
        let refusal = Gate::preflight(&gate, "https://127.0.0.1/private", NetworkMode::Strict)
            .expect_err("strict refuses https");
        assert_eq!(refusal.rule.as_deref(), Some(STRICT_RULE), "{refusal:?}");
        assert!(
            Gate::preflight(
                &gate,
                &format!("http://127.0.0.1:{}/plain", origin.port),
                NetworkMode::Strict
            )
            .is_ok(),
            "strict must still allow plain http"
        );
    }

    #[test]
    fn an_authority_splits_into_host_and_port() {
        assert_eq!(
            split_authority("example.com:443"),
            Some((String::from("example.com"), 443))
        );
        assert_eq!(split_authority("example.com"), None);
    }

    #[test]
    fn prose_is_escaped_into_the_refusal_page() {
        assert_eq!(escape("a < b & c"), "a &lt; b &amp; c");
    }
}
