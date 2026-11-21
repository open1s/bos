//! HTTP transport for the MCP server.
//!
//! Supports POST /mcp (JSON or SSE response), GET /health, optional session
//! headers, and optional bearer-token authentication. It is still a single
//! process-wide session, not a multi-tenant Streamable HTTP deployment.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{Value, json};

use crate::server::McpServer;

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
    let Some(request) = read_request(&mut stream)? else {
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

fn read_request(stream: &mut TcpStream) -> Result<Option<Request>, String> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|error| format!("clone HTTP stream: {error}"))?,
    );
    let mut request_line = String::new();
    if reader
        .read_line(&mut request_line)
        .map_err(|error| format!("read HTTP request line: {error}"))?
        == 0
    {
        return Ok(None);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();

    let mut headers = BTreeMap::new();
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        if reader
            .read_line(&mut header)
            .map_err(|error| format!("read HTTP header: {error}"))?
            == 0
        {
            break;
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                content_length = value
                    .parse()
                    .map_err(|_| "HTTP Content-Length is not a non-negative integer".to_owned())?;
            }
            headers.insert(name, value);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|error| format!("read HTTP body: {error}"))?;
    }
    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
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
