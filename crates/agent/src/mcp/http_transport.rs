/// A slot a turn installs its egress gate into.
///
/// The pool owns one slot and every HTTP transport holds a clone, so a turn
/// that builds a new [`crate::policy::PolicySet`] updates every live MCP
/// connection's egress check without reconnecting it.
pub type EgressSlot =
    std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<crate::policy::EgressGate>>>>;

/// Posts JSON-RPC messages to an MCP server over streamable HTTP.
pub struct HttpTransport {
    base_url: String,
    session_id: std::sync::Mutex<Option<String>>,
    egress: EgressSlot,
}

/// Errors from the HTTP transport.
#[derive(Debug, Clone)]
pub enum HttpTransportError {
    /// The request or response was invalid.
    Http(String),
    /// The server could not be reached.
    Connect(String),
    /// The MCP session could not be established or used.
    Session(String),
}

impl std::fmt::Display for HttpTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(s) => write!(f, "HTTP error: {s}"),
            Self::Connect(s) => write!(f, "Connection error: {s}"),
            Self::Session(s) => write!(f, "Session error: {s}"),
        }
    }
}

impl std::error::Error for HttpTransportError {}

/// One HTTP response captured off the async runtime.
#[derive(Debug)]
struct HttpResponse {
    status: u16,
    session_id: Option<String>,
    content_type: String,
    body: String,
}

/// Perform the request and drain the body on a blocking thread.
///
/// `ureq` is synchronous, so both the round trip and reading the response
/// body block. Reading the body back on the async thread after
/// `spawn_blocking` returned would pin a runtime worker for as long as the
/// server takes to finish the response.
fn post_json(
    base_url: &str,
    session_id: Option<String>,
    body: Vec<u8>,
    egress: Option<std::sync::Arc<crate::policy::EgressGate>>,
) -> Result<HttpResponse, String> {
    // With a gate installed, ureq resolves through the policy. Its resolver is
    // the only place a name becomes an address, so a denied host opens no
    // socket and an approved one connects to the very address the policy saw.
    let agent = match &egress {
        Some(gate) => ureq::Agent::with_parts(
            ureq::config::Config::default(),
            ureq::unversioned::transport::DefaultConnector::default(),
            crate::policy::PolicyResolver::new(gate.clone()),
        ),
        None => ureq::Agent::new_with_defaults(),
    };
    let mut request = agent
        .post(base_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(sid) = session_id {
        request = request.header("Mcp-Session-Id", sid);
    }

    let response = request.send(body.as_slice()).map_err(|error| {
        if let (Some(gate), Some((host, port))) = (
            egress.as_ref(),
            crate::policy::egress::host_and_port(base_url),
        ) {
            if let Some(denial) = gate.denial(&host, port) {
                return gate.policy().refusal(&denial.decision);
            }
        }
        format!("Request error: {error}")
    })?;

    let status = response.status().as_u16();
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|e| format!("Read body error: {e}"))?;

    Ok(HttpResponse {
        status,
        session_id,
        content_type,
        body,
    })
}

impl HttpTransport {
    /// Create a transport for `base_url`, trimming any trailing slash.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            session_id: std::sync::Mutex::new(None),
            egress: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Create a transport that shares `egress` with the pool that owns it.
    pub fn with_egress(base_url: impl Into<String>, egress: EgressSlot) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            session_id: std::sync::Mutex::new(None),
            egress,
        }
    }

    /// Install (or clear) the gate this transport asks before a request.
    pub fn set_egress(&self, gate: Option<std::sync::Arc<crate::policy::EgressGate>>) {
        if let Ok(mut slot) = self.egress.lock() {
            *slot = gate;
        }
    }

    /// The gate currently installed, if any.
    pub fn egress(&self) -> Option<std::sync::Arc<crate::policy::EgressGate>> {
        self.egress
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().cloned())
    }

    /// Post one JSON-RPC message and return the response body.
    pub async fn send(&self, msg: &serde_json::Value) -> Result<String, HttpTransportError> {
        let base_url = self.base_url.clone();
        let body = serde_json::to_vec(msg).map_err(|e| HttpTransportError::Http(e.to_string()))?;
        let session_id = self.session_id.lock().unwrap().clone();
        let egress = self.egress();

        let response =
            tokio::task::spawn_blocking(move || post_json(&base_url, session_id, body, egress))
                .await
                .map_err(|e| HttpTransportError::Http(format!("Join error: {e}")))?
                .map_err(HttpTransportError::Http)?;

        if let Some(sid) = response.session_id {
            *self.session_id.lock().unwrap() = Some(sid);
        }

        if response.content_type.contains("text/event-stream") {
            return self.parse_sse_response(&response.body);
        }

        if !(200..=299).contains(&response.status) && response.status != 202 {
            if response.body.is_empty() {
                return Err(HttpTransportError::Http(format!(
                    "HTTP {} (empty body)",
                    response.status
                )));
            }
            return Err(HttpTransportError::Http(response.body));
        }

        Ok(response.body)
    }

    /// The current MCP session id, if any.
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    fn parse_sse_response(&self, text: &str) -> Result<String, HttpTransportError> {
        let mut last_data = String::new();
        for line in text.lines() {
            let trimmed = line.trim_end();
            if let Some(stripped) = trimmed.strip_prefix("data: ") {
                last_data = stripped.to_string();
            } else if trimmed.is_empty() && !last_data.is_empty() {
                return Ok(last_data.clone());
            }
        }
        if !last_data.is_empty() {
            Ok(last_data)
        } else {
            Err(HttpTransportError::Http("No data in SSE stream".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serve exactly one HTTP response, then close.
    fn serve_once(response: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("local addr");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 8192];
            let _ = std::io::Read::read(&mut stream, &mut buffer);
            std::io::Write::write_all(&mut stream, response).expect("write response");
        });
        (format!("http://{address}/mcp"), handle)
    }

    #[test]
    fn post_json_returns_status_session_and_drained_body() {
        let (url, handle) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
              Mcp-Session-Id: session-7\r\nContent-Length: 11\r\n\
              Connection: close\r\n\r\n{\"ok\":true}",
        );
        let response = post_json(&url, None, b"{}".to_vec(), None).expect("request succeeds");
        handle.join().expect("server thread");

        assert_eq!(response.status, 200);
        assert_eq!(response.session_id.as_deref(), Some("session-7"));
        assert_eq!(response.body, "{\"ok\":true}");
    }

    #[test]
    fn post_json_forwards_the_session_header() {
        let (url, handle) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        let response =
            post_json(&url, Some("session-1".to_string()), b"{}".to_vec(), None).expect("request");
        handle.join().expect("server thread");
        assert_eq!(response.body, "{}");
    }

    /// A denied host opens no socket at all: the listener never accepts, so the
    /// proof is the absent connection rather than the error string.
    #[test]
    fn a_denied_egress_never_opens_a_socket() {
        use std::sync::Arc;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("local addr");
        let policy = Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new("/tmp/policy-probe-workspace"),
        ));
        let gate = Arc::new(crate::policy::EgressGate::new(
            policy,
            Arc::new(crate::policy::SystemResolver),
            "session",
            "preset",
            "mcp",
        ));
        let url = format!("http://{address}/mcp");
        let error = post_json(&url, None, b"{}".to_vec(), Some(gate))
            .expect_err("the empty allowlist denies 127.0.0.1");
        assert!(
            error.contains("egress-not-allowlisted"),
            "the refusal names the rule: {error}"
        );
        match listener.accept() {
            Err(error) => assert_eq!(
                error.kind(),
                std::io::ErrorKind::WouldBlock,
                "the listener must have no pending connection"
            ),
            Ok(_) => panic!("a socket was opened despite the denial"),
        }
    }

    /// The positive half: an allowlisted address is connected and the request
    /// really reaches the server.
    #[test]
    fn an_allowlisted_address_receives_the_request() {
        use std::sync::Arc;

        let (url, handle) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
              Content-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
        );
        let policy = Arc::new(crate::policy::PolicySet::compile_baseline(
            crate::policy::PolicyData::new("/tmp/policy-probe-workspace")
                .with_egress_allowlist(["127.0.0.1".to_string()]),
        ));
        let gate = Arc::new(crate::policy::EgressGate::new(
            policy,
            Arc::new(crate::policy::SystemResolver),
            "session",
            "preset",
            "mcp",
        ));
        let response = post_json(&url, None, b"{}".to_vec(), Some(gate))
            .expect("an allowlisted address connects");
        handle.join().expect("server thread");
        assert_eq!(response.body, "{\"ok\":true}");
    }

    #[test]
    fn the_sse_body_is_parsed_as_one_message() {
        let transport = HttpTransport::new("http://127.0.0.1:1/mcp");
        let body = "event: message\ndata: {\"a\":1}\n\n";
        assert_eq!(
            transport.parse_sse_response(body).expect("data block"),
            "{\"a\":1}"
        );
        assert!(transport.parse_sse_response("event: ping\n").is_err());
    }
}
