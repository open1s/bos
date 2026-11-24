//! HTTP transport for the MCP server.
//!
//! Supports POST /mcp (JSON or SSE response), GET /health, optional session
//! headers, and optional bearer-token authentication. It is still a single
//! process-wide session, not a multi-tenant Streamable HTTP deployment.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::{Value, json};

use crate::server::McpServer;

/// Largest accepted request line, including its line terminator.
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
/// Largest accepted single header line, including its line terminator.
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
/// Largest accepted header block, including line terminators.
const MAX_HEADER_BYTES: usize = 8 * 1024;
/// Largest accepted number of headers.
const MAX_HEADER_COUNT: usize = 100;
/// Largest accepted request body: the codel tool budget already used for an
/// answer (`MAX_ANSWER_TOKENS` tokens at `BYTES_PER_TOKEN`).
const MAX_BODY_BYTES: usize = codel::MAX_ANSWER_TOKENS * codel::BYTES_PER_TOKEN;
/// Read and write timeout for one HTTP connection.
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// Serve HTTP until the listener fails.
pub fn run(address: &str) -> Result<(), String> {
    let listener = TcpListener::bind(address)
        .map_err(|error| format!("bind HTTP listener {address}: {error}"))?;
    let local = listener
        .local_addr()
        .map_err(|error| format!("read HTTP listener address: {error}"))?;
    eprintln!("codel-mcp: HTTP listening on http://{local}/mcp");

    let mut server = McpServer::new(Vec::new());
    server.disable_roots();
    server.enable_watch();
    if let Some(host) = crate::workspace::host_from_env() {
        server.set_host(host);
    }
    let token = std::env::var("CODEL_HTTP_TOKEN").ok();
    let mut session = None;

    // `std::net::TcpListener` exposes no read/write timeout; each accepted
    // stream gets both in `handle_connection`, which bounds this loop.
    for stream in listener.incoming() {
        let stream = stream.map_err(|error| format!("accept HTTP connection: {error}"))?;
        if let Err(error) = handle_connection(stream, &mut server, &mut session, token.as_deref()) {
            eprintln!("codel-mcp: HTTP request failed: {error}");
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    server: &mut McpServer<Vec<u8>>,
    session: &mut Option<String>,
    token: Option<&str>,
) -> Result<(), String> {
    // A stalled client must not hold the accept loop forever: both directions
    // of every accepted stream are bounded by `HTTP_IO_TIMEOUT`.
    stream
        .set_read_timeout(Some(HTTP_IO_TIMEOUT))
        .map_err(|error| format!("set HTTP read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(HTTP_IO_TIMEOUT))
        .map_err(|error| format!("set HTTP write timeout: {error}"))?;
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|error| format!("clone HTTP stream: {error}"))?,
    );
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };
    if let Some(token) = token
        && request.path == "/mcp"
        && !authorized(&request, token)
    {
        return respond(
            &mut stream,
            "401 Unauthorized",
            "application/json",
            br#"{"error":"missing or invalid bearer token"}"#,
            None,
        );
    }
    if let Some(expected) = session.as_deref()
        && let Some(supplied) = request.headers.get("mcp-session-id")
        && supplied != expected
    {
        return respond(
            &mut stream,
            "404 Not Found",
            "application/json",
            br#"{"error":"unknown MCP session"}"#,
            None,
        );
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => respond(
            &mut stream,
            "200 OK",
            "application/json",
            br#"{"status":"ok"}"#,
            None,
        ),
        ("POST", "/mcp") => {
            let initialize = serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|value| {
                    value
                        .get("method")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .as_deref()
                == Some("initialize");
            match handle_body(server, &request.body) {
                Ok(response) if response.is_empty() => respond(
                    &mut stream,
                    "202 Accepted",
                    "text/plain",
                    b"",
                    session.as_deref(),
                ),
                Ok(response) => {
                    if initialize && session.is_none() {
                        *session = Some(new_session_id());
                    }
                    if request
                        .headers
                        .get("accept")
                        .is_some_and(|value| value.contains("text/event-stream"))
                    {
                        let text = String::from_utf8_lossy(&response);
                        let sse = format!("event: message\ndata: {text}\n\n");
                        respond(
                            &mut stream,
                            "200 OK",
                            "text/event-stream",
                            sse.as_bytes(),
                            session.as_deref(),
                        )
                    } else {
                        respond(
                            &mut stream,
                            "200 OK",
                            "application/json",
                            &response,
                            session.as_deref(),
                        )
                    }
                }
                Err(error) => {
                    let body = serde_json::to_vec(&json!({"error": error})).unwrap_or_default();
                    respond(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &body,
                        session.as_deref(),
                    )
                }
            }
        }
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain",
            b"not found",
            None,
        ),
    }
}

fn authorized(request: &Request, token: &str) -> bool {
    request
        .headers
        .get("authorization")
        .is_some_and(|value| value == &format!("Bearer {token}"))
}

fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}-{:x}", std::process::id())
}

fn handle_body(server: &mut McpServer<Vec<u8>>, body: &[u8]) -> Result<Vec<u8>, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))?;
    server.handle(value)?;
    let mut output = server.take_output();
    while output
        .last()
        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
    {
        output.pop();
    }
    Ok(output)
}

fn read_request<R: BufRead>(reader: &mut R) -> Result<Option<Request>, String> {
    let Some(request_line) = read_bounded_line(reader, MAX_REQUEST_LINE_BYTES, "request line")?
    else {
        return Ok(None);
    };
    let request_line = std::str::from_utf8(&request_line)
        .map_err(|_| "HTTP request line is not valid UTF-8".to_owned())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();

    let mut headers = BTreeMap::new();
    let mut content_length = 0usize;
    let mut header_bytes = 0usize;
    let mut header_count = 0usize;
    while let Some(header) = read_bounded_line(reader, MAX_HEADER_LINE_BYTES, "header line")? {
        header_bytes += header.len();
        if header_bytes > MAX_HEADER_BYTES {
            return Err(format!(
                "HTTP headers exceed the {MAX_HEADER_BYTES}-byte limit"
            ));
        }
        let header = std::str::from_utf8(&header)
            .map_err(|_| "HTTP header is not valid UTF-8".to_owned())?;
        if header == "\r\n" || header == "\n" {
            break;
        }
        header_count += 1;
        if header_count > MAX_HEADER_COUNT {
            return Err(format!(
                "HTTP request has more than {MAX_HEADER_COUNT} headers"
            ));
        }
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                let length = value
                    .parse::<u64>()
                    .map_err(|_| "HTTP Content-Length is not a non-negative integer".to_owned())?;
                if length > MAX_BODY_BYTES as u64 {
                    return Err(format!(
                        "HTTP Content-Length {length} exceeds the {MAX_BODY_BYTES}-byte body limit"
                    ));
                }
                content_length = length as usize;
            }
            headers.insert(name, value);
        }
    }

    // Grow the buffer as body bytes arrive instead of reserving the declared
    // length up front; the chunk loop also bounds every single read.
    let mut body = Vec::with_capacity(content_length.min(8 * 1024));
    let mut remaining = content_length;
    let mut chunk = [0u8; 8 * 1024];
    while remaining > 0 {
        let take = remaining.min(chunk.len());
        reader
            .read_exact(&mut chunk[..take])
            .map_err(|error| format!("read HTTP body: {error}"))?;
        body.extend_from_slice(&chunk[..take]);
        remaining -= take;
    }
    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
}

/// Read one newline-terminated line without ever buffering more than `limit`
/// bytes. A line that reaches the cap is rejected rather than truncated, and
/// `None` means the input ended before the first byte.
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
    what: &str,
) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("read HTTP {what}: {error}"))?;
        if available.is_empty() {
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if line.len() + take > limit {
            return Err(format!("HTTP {what} exceeds the {limit}-byte limit"));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }
    if line.is_empty() {
        Ok(None)
    } else {
        Ok(Some(line))
    }
}

/// Whether every socket address an HTTP bind string resolves to is loopback.
pub(crate) fn resolves_to_loopback(address: &str) -> Result<bool, String> {
    use std::net::ToSocketAddrs;

    let addresses = address
        .to_socket_addrs()
        .map_err(|error| format!("resolve HTTP address {address}: {error}"))?;
    let mut resolved = false;
    for address in addresses {
        resolved = true;
        if !address.ip().is_loopback() {
            return Ok(false);
        }
    }
    if resolved {
        Ok(true)
    } else {
        Err(format!(
            "HTTP address {address} resolved to no socket address"
        ))
    }
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    session: Option<&str>,
) -> Result<(), String> {
    let session = session
        .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
        .unwrap_or_default();
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{session}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .map_err(|error| format!("write HTTP response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn parse(request: &[u8]) -> Result<Option<Request>, String> {
        read_request(&mut Cursor::new(request.to_vec()))
    }

    #[test]
    fn a_normal_request_parses() {
        let request =
            parse(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}")
                .expect("parsed")
                .expect("request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/mcp");
        assert_eq!(
            request.headers.get("host").map(String::as_str),
            Some("localhost")
        );
        assert_eq!(request.body, b"{}");
    }

    #[test]
    fn an_oversized_request_line_is_rejected() {
        let mut request = vec![b'a'; MAX_REQUEST_LINE_BYTES + 1];
        request.extend_from_slice(b"\r\n\r\n");
        let error = parse(&request).expect_err("rejected");
        assert!(error.contains("request line"), "{error}");
        assert!(
            error.contains(&MAX_REQUEST_LINE_BYTES.to_string()),
            "{error}"
        );
    }

    #[test]
    fn too_many_headers_are_rejected() {
        let mut request = String::from("POST /mcp HTTP/1.1\r\n");
        for _ in 0..=MAX_HEADER_COUNT {
            request.push_str("X: y\r\n");
        }
        request.push_str("\r\n");
        let error = parse(request.as_bytes()).expect_err("rejected");
        assert!(error.contains("more than"), "{error}");
    }

    #[test]
    fn an_oversized_header_line_is_rejected() {
        let mut request = b"POST /mcp HTTP/1.1\r\nX: ".to_vec();
        request.extend(std::iter::repeat_n(b'a', MAX_HEADER_LINE_BYTES));
        request.extend_from_slice(b"\r\n\r\n");
        let error = parse(&request).expect_err("rejected");
        assert!(error.contains("header line"), "{error}");
    }

    #[test]
    fn an_oversized_header_block_is_rejected() {
        let mut request = String::from("POST /mcp HTTP/1.1\r\n");
        for _ in 0..9 {
            request.push_str(&format!("X: {}\r\n", "a".repeat(1000)));
        }
        request.push_str("\r\n");
        let error = parse(request.as_bytes()).expect_err("rejected");
        assert!(error.contains("headers exceed"), "{error}");
    }

    #[test]
    fn an_oversized_content_length_is_rejected_before_the_body() {
        let request = format!("POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n", u64::MAX);
        let error = parse(request.as_bytes()).expect_err("rejected");
        assert!(error.contains("Content-Length"), "{error}");
        assert!(error.contains(&MAX_BODY_BYTES.to_string()), "{error}");
    }

    #[test]
    fn a_loopback_bind_is_recognized() {
        assert!(resolves_to_loopback("127.0.0.1:8765").expect("resolved"));
        assert!(resolves_to_loopback("[::1]:8765").expect("resolved"));
        assert!(!resolves_to_loopback("0.0.0.0:8765").expect("resolved"));
    }

    #[test]
    fn initialize_over_http_body_returns_a_json_response() {
        let mut server = McpServer::new(Vec::new());
        server.disable_roots();
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}},
        }))
        .expect("json");

        let response = handle_body(&mut server, &body).expect("response");
        let value: Value = serde_json::from_slice(&response).expect("response JSON");
        assert_eq!(value["result"]["serverInfo"]["name"], "codel-mcp");
    }

    #[test]
    fn a_notification_produces_no_http_body() {
        let mut server = McpServer::new(Vec::new());
        server.disable_roots();
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }))
        .expect("json");

        assert!(handle_body(&mut server, &body).expect("handled").is_empty());
    }
}
