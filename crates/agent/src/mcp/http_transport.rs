/// Posts JSON-RPC messages to an MCP server over streamable HTTP.
pub struct HttpTransport {
    base_url: String,
    session_id: std::sync::Mutex<Option<String>>,
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
) -> Result<HttpResponse, String> {
    let mut request = ureq::post(base_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(sid) = session_id {
        request = request.header("Mcp-Session-Id", sid);
    }

    let response = request
        .send(body.as_slice())
        .map_err(|e| format!("Request error: {e}"))?;

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
        let url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url: url,
            session_id: std::sync::Mutex::new(None),
        }
    }

    /// Post one JSON-RPC message and return the response body.
    pub async fn send(&self, msg: &serde_json::Value) -> Result<String, HttpTransportError> {
        let base_url = self.base_url.clone();
        let body = serde_json::to_vec(msg).map_err(|e| HttpTransportError::Http(e.to_string()))?;
        let session_id = self.session_id.lock().unwrap().clone();

        let response = tokio::task::spawn_blocking(move || post_json(&base_url, session_id, body))
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
        let response = post_json(&url, None, b"{}".to_vec()).expect("request succeeds");
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
            post_json(&url, Some("session-1".to_string()), b"{}".to_vec()).expect("request");
        handle.join().expect("server thread");
        assert_eq!(response.body, "{}");
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
