//! The one policy-gated HTTP GET.
//!
//! A page fetch is an egress decision followed by a request, never a request
//! that happens to be checked. The fetch function asks the gate about the
//! resolved
//! address BEFORE the client is built, so a refused URL opens no socket, sends
//! no bytes and leaves no half-made connection - and the refusal names the
//! policy rule that produced it.
//!
//! The gate is not re-implemented here: the same EgressGate a shell or MCP
//! transport installs is the one asked, and PolicyResolver is installed in the
//! client as well, so the name-to-address step inside the client is gated too.
//! The explicit check first is what makes the refusal REPORTABLE: ureq answers
//! a denied resolution with a bare HostNotFound, while the gate still remembers
//! the denial it cached.
//!
//! Redirects are followed here rather than by the client. Each hop is a new
//! URL, so each hop is a new decision: a redirect to a host the policy refuses
//! is refused with its own rule, and cannot ride the first hop's approval. The
//! client is configured with zero redirects for the same reason - a client that
//! followed one itself would decide without the policy.
//!
//! What this does NOT do is guaranteed by construction: it reads a body and
//! returns it. It runs no script, keeps no cookie jar, resolves no URL against
//! a page's own JavaScript and performs no download.

use std::sync::Arc;
use std::time::Duration;

use super::egress::{host_and_port, EgressDenial, EgressGate, PolicyResolver};

/// How much a fetch may read, and how far it may follow.
#[derive(Debug, Clone, Copy)]
pub struct FetchLimits {
    /// Most body bytes kept. A longer body is truncated, never buffered whole.
    pub max_body: usize,
    /// Most redirects followed. Each one is a fresh policy decision.
    pub max_redirects: u8,
    /// Wall-clock ceiling for one request, so a stalled server cannot wedge a
    /// caller forever.
    pub timeout: Duration,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            max_body: 2 * 1024 * 1024,
            max_redirects: 5,
            timeout: Duration::from_secs(30),
        }
    }
}

/// One page as the client saw it, after redirects.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// The URL that actually answered - the last hop, not the one asked for.
    pub url: String,
    /// The HTTP status. A 404 is a page here, not an error: its body is what a
    /// reader (or a model) needs to see.
    pub status: u16,
    /// The response's Content-Type, empty when the server sent none.
    pub content_type: String,
    /// The body, decoded as UTF-8 with replacement characters.
    pub body: String,
    /// Whether the body was cut at FetchLimits::max_body.
    pub body_truncated: bool,
}

/// Why a fetch did not produce a page.
///
/// The variants are separated because the reader's next move differs: a scheme
/// refusal is the caller's bug, a policy refusal is the workspace's allowlist,
/// a resolution failure is DNS, and a connection failure is the remote end.
#[derive(Debug, Clone)]
pub enum FetchError {
    /// The URL is not http or https, or has no host.
    Scheme(String),
    /// The policy refused the resolved address. No request was made.
    Refused(EgressDenial),
    /// The name did not resolve. No request was made.
    Unresolved(String),
    /// The TLS handshake failed: a bad certificate, a protocol mismatch or a
    /// server that does not speak TLS on that port.
    Tls(String),
    /// The connection was refused, reset or timed out.
    Connect(String),
    /// Too many redirects.
    Redirect(String),
    /// The response arrived but could not be read.
    Body(String),
}

impl FetchError {
    /// The refusal prose for a policy denial, in the policy's own locale.
    ///
    /// The rule id travels with it as a stable, language-neutral fact; the
    /// prose is the policy's, so a shell and a tool cannot word one decision
    /// two ways.
    pub fn refusal(&self, gate: &EgressGate) -> Option<String> {
        match self {
            FetchError::Refused(denial) => Some(gate.policy().refusal(&denial.decision)),
            _ => None,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Scheme(url) => write!(
                f,
                "{url} is not an http or https URL; only those schemes can be fetched"
            ),
            FetchError::Refused(denial) => write!(f, "refused by policy: {denial}"),
            FetchError::Unresolved(host) => write!(f, "{host} did not resolve to an address"),
            FetchError::Tls(message) => write!(f, "the TLS handshake failed: {message}"),
            FetchError::Connect(message) => write!(f, "the connection failed: {message}"),
            FetchError::Redirect(message) => write!(f, "{message}"),
            FetchError::Body(message) => {
                write!(f, "the response body could not be read: {message}")
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// The scheme and authority a URL names, refusing anything but http(s).
///
/// This is the browser's own rule and the transport's floor: a URL with another
/// scheme is refused here, before a gate is even asked, because a policy about
/// hosts has nothing to say about a file: or javascript: target.
pub fn http_host(url: &str) -> Result<(String, u16), FetchError> {
    let uri: ureq::http::Uri = url
        .trim()
        .parse()
        .map_err(|_| FetchError::Scheme(url.trim().to_string()))?;
    let scheme = uri.scheme_str().unwrap_or_default().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(FetchError::Scheme(url.trim().to_string()));
    }
    match host_and_port(url.trim()) {
        // A scheme check that passed but a URL with no host: "http:/path".
        Some((host, port)) if !host.is_empty() => Ok((host, port)),
        _ => Err(FetchError::Scheme(url.trim().to_string())),
    }
}

/// Fetch a URL through a gate, following redirects one decision at a time.
///
/// Blocking: the caller decides which thread pays for it. Returns the last
/// hop's page, so a caller that shows a URL shows where the reader ended up.
pub fn fetch(
    gate: &Arc<EgressGate>,
    url: &str,
    limits: &FetchLimits,
) -> Result<FetchedPage, FetchError> {
    let mut current = url.trim().to_string();
    let mut hops = 0u8;
    loop {
        let (host, port) = http_host(&current)?;
        // The decision comes first, and it is the reported one: a denial here
        // returns the gate's own cached refusal rather than a client error.
        if let Err(denial) = gate.authorize(&host, port) {
            return Err(FetchError::Refused(denial));
        }
        let config = ureq::config::Config::builder()
            // The client must not decide about a redirect: every hop is a URL
            // the policy has to see, so following happens below.
            .max_redirects(0)
            // A 404's body is the page. The status is reported, not raised.
            .http_status_as_error(false)
            .user_agent("BOS-Harness browser")
            .timeout_global(Some(limits.timeout))
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::default(),
            PolicyResolver::new(gate.clone()),
        );
        let response = agent
            .get(&current)
            .call()
            .map_err(|error| classify(error, gate, &host, port))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            if location.is_empty() {
                return Err(FetchError::Redirect(format!(
                    "the server answered {status} with no Location header"
                )));
            }
            hops += 1;
            if hops > limits.max_redirects {
                return Err(FetchError::Redirect(format!(
                    "more than {} redirects; the last one pointed at {location}",
                    limits.max_redirects
                )));
            }
            current = join(&current, &location);
            continue;
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .into_body()
            .with_config()
            // One byte over the cap is enough to know it was cut.
            .limit(limits.max_body as u64 + 1)
            .lossy_utf8(true)
            .read_to_string()
            .map_err(|error| FetchError::Body(error.to_string()))?;
        let truncated = body.len() > limits.max_body;
        let body = if truncated {
            let mut cut = limits.max_body;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            body[..cut].to_string()
        } else {
            body
        };
        return Ok(FetchedPage {
            url: current,
            status,
            content_type,
            body,
            body_truncated: truncated,
        });
    }
}

/// Map a client failure onto the reason a reader can act on.
///
/// A resolution failure is where the gate's cache is consulted: ureq reports a
/// denied resolution as HostNotFound with no detail, while the gate still holds
/// the decision and the rule that produced it.
fn classify(error: ureq::Error, gate: &EgressGate, host: &str, port: u16) -> FetchError {
    if let Some(denial) = gate.denial(host, port) {
        return FetchError::Refused(denial);
    }
    match error {
        ureq::Error::HostNotFound => FetchError::Unresolved(host.to_string()),
        ureq::Error::Timeout(_) => {
            FetchError::Connect(format!("{host}:{port} did not answer in time"))
        }
        ureq::Error::StatusCode(code) => {
            FetchError::Connect(format!("{host}:{port} answered {code}"))
        }
        other => {
            let text = other.to_string();
            let lower = text.to_ascii_lowercase();
            if lower.contains("tls")
                || lower.contains("certificate")
                || lower.contains("handshake")
                || lower.contains("invalid peer")
            {
                FetchError::Tls(text)
            } else {
                FetchError::Connect(text)
            }
        }
    }
}

/// Resolve a redirect Location against its base, the way a browser resolves it.
///
/// Deliberately small and predictable: an absolute URL wins, a double-slash URL
/// keeps the base scheme, an absolute path replaces the path, and anything else
/// is relative to the base's directory. Dot segments are folded so the URL a
/// reader sees is the URL that was asked for.
pub fn join(base: &str, location: &str) -> String {
    let location = location.trim();
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return location.to_string();
    }
    let (scheme, rest) = match base.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => return location.to_string(),
    };
    if let Some(rest) = location.strip_prefix("//") {
        return format!("{scheme}://{rest}");
    }
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if let Some(absolute) = location.strip_prefix('/') {
        return format!("{scheme}://{authority}/{}", fold(absolute));
    }
    if location.is_empty() {
        return base.to_string();
    }
    // The base's directory: everything up to and including its last slash.
    let directory = match path.rfind('/') {
        Some(index) => &path[..=index],
        None => "/",
    };
    // The dot segments are resolved against the COMBINED path, so ".." can climb
    // out of the base's directory: folding only the location made "../up" land
    // inside "/docs/" instead of beside it.
    let mut target = String::with_capacity(directory.len() + location.len());
    target.push_str(directory);
    target.push_str(location);
    format!("{scheme}://{authority}{}", fold(&target))
}

/// Fold dot segments out of a path, keeping any query string.
fn fold(path: &str) -> String {
    let (path, query) = match path.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path, None),
    };
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    let mut joined = out.join("/");
    if path.ends_with('/') && !joined.ends_with('/') {
        joined.push('/');
    }
    match query {
        Some(query) => format!("{joined}?{query}"),
        None => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-http scheme never reaches a policy question.
    #[test]
    fn only_http_and_https_urls_get_a_host() {
        assert!(http_host("https://example.com/a").is_ok());
        assert!(matches!(
            http_host("file:///etc/passwd"),
            Err(FetchError::Scheme(_))
        ));
        assert!(matches!(
            http_host("javascript:alert(1)"),
            Err(FetchError::Scheme(_))
        ));
        assert!(matches!(http_host("not a url"), Err(FetchError::Scheme(_))));
    }

    /// A redirect resolves like a browser: absolute, scheme-relative, rooted,
    /// relative and dot-segment.
    #[test]
    fn a_redirect_resolves_against_its_base() {
        let base = "https://example.com/docs/page.html?x=1";
        assert_eq!(join(base, "https://other.test/a"), "https://other.test/a");
        assert_eq!(join(base, "//other.test/a"), "https://other.test/a");
        assert_eq!(join(base, "/root"), "https://example.com/root");
        assert_eq!(join(base, "next"), "https://example.com/docs/next");
        assert_eq!(join(base, "../up"), "https://example.com/up");
        assert_eq!(join(base, "./same"), "https://example.com/docs/same");
        assert_eq!(
            join("http://127.0.0.1:8080", "/a"),
            "http://127.0.0.1:8080/a"
        );
    }

    /// A refused connect names the deciding rule, not the client's error.
    ///
    /// The gate is built over a resolver that answers a fixed address and a
    /// policy that refuses everything, so the assertion is about the ORDER: the
    /// decision is recorded and reported before any request exists.
    #[test]
    fn a_refusal_is_the_policy_denial_not_a_client_error() {
        struct Fixed(std::net::SocketAddr);
        impl super::super::egress::Resolver for Fixed {
            fn resolve(
                &self,
                _host: &str,
                _port: u16,
            ) -> std::io::Result<Vec<std::net::SocketAddr>> {
                Ok(vec![self.0])
            }
        }
        let policy = Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new("/tmp").with_egress_allowlist(["192.0.2.1"]),
        ));
        let gate = Arc::new(EgressGate::new(
            policy.clone(),
            Arc::new(Fixed("198.51.100.7:80".parse().expect("addr"))),
            "session",
            "preset",
            "browser_open",
        ));
        let error = fetch(&gate, "http://example.test/", &FetchLimits::default())
            .expect_err("the policy refuses this address");
        match error {
            FetchError::Refused(denial) => {
                assert_eq!(
                    denial.decision.rule.as_deref(),
                    Some("egress-not-allowlisted"),
                    "the refusal names the rule: {denial:?}"
                );
            }
            other => panic!("expected the policy refusal, got {other:?}"),
        }
        // The decision is in the ring the audit pane reads, under this tool.
        let denials = policy.denials();
        assert_eq!(denials.len(), 1, "one decision, recorded: {denials:?}");
        assert_eq!(denials[0].tool, "browser_open");
    }
}
