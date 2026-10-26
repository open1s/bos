//! The CDP wire: JSON-RPC 2.0 messages and the WebSocket that carries them.
//!
//! # Why this is its own layer
//!
//! Everything above this file assumes two things and nothing else: a request
//! with an `id` gets exactly one answer with that `id`, and a frame with no
//! `id` is an event some other part of the engine may care about. The
//! WebSocket's own framing (masking, fragmentation, ping/pong) is provided by
//! `tungstenite`, which is already in this build's graph through zenoh, so
//! adding it directly adds no package to `Cargo.lock` and no compile unit.
//!
//! # The error shape
//!
//! A CDP error is a typed value here, not a string that a caller has to parse:
//! one refusal from Obscura (`{"code":-32601,"message":"No page for session"}`)
//! has to be told apart from a transport failure, because the first is a
//! producer that said no and the second is a socket that died.

use std::fmt;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

/// The most bytes one read will accept, so a corrupt length prefix cannot ask
/// for an unbounded allocation inside the engine's thread.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// One CDP error object, as the server sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RpcError {
    /// The JSON-RPC error code (`-32601` is "method not found").
    pub(crate) code: i64,
    /// The server's own sentence.
    pub(crate) message: String,
    /// Optional structured detail, kept as text because CDP's shape varies.
    pub(crate) data: Option<String>,
}

impl RpcError {
    /// Whether the engine does not implement the method at all.
    ///
    /// This is kept apart from every other refusal because it is a fact about
    /// the BUILD (a smaller CDP surface), not about the page, so the pane can
    /// offer a different next step.
    pub(crate) fn unsupported(&self) -> bool {
        self.code == -32601
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.data {
            Some(data) => write!(formatter, "{} (code {}): {data}", self.message, self.code),
            None => write!(formatter, "{} (code {})", self.message, self.code),
        }
    }
}

impl std::error::Error for RpcError {}

/// What a parsed frame turned out to be.
#[derive(Debug, Clone)]
pub(crate) enum Incoming {
    /// The answer to one of this client's requests.
    Response {
        /// The correlation id the request carried.
        id: u64,
        /// The result object, when the call succeeded.
        result: Value,
        /// The error, when it did not.
        error: Option<RpcError>,
    },
    /// An unsolicited event, which the engine routes by method name.
    Event {
        /// The CDP method, for example `Fetch.requestPaused`.
        method: String,
        /// The event parameters.
        params: Value,
    },
}

/// One outgoing request.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// Correlation id, unique for the life of the connection.
    pub(crate) id: u64,
    /// The CDP method to call.
    pub(crate) method: String,
    /// Its parameters.
    pub(crate) params: Value,
    /// The target session, when the connection is the browser endpoint. Every
    /// page command needs it: the browser endpoint refuses an unattributed
    /// page command with `No page for session` (measured).
    pub(crate) session: Option<String>,
}

impl Request {
    /// The JSON text to put on the socket.
    pub(crate) fn encode(&self) -> String {
        let mut object = json!({
            "id": self.id,
            "method": self.method,
            "params": self.params,
        });
        if let Some(session) = &self.session {
            object["sessionId"] = Value::String(session.clone());
        }
        object.to_string()
    }
}

/// Parse one text frame.
///
/// A frame that is neither a response nor an event is an error rather than a
/// silent skip: every frame this client can receive is one of the two, so an
/// unrecognised shape means the producer changed its protocol.
pub(crate) fn decode(text: &str) -> Result<Incoming, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        format!(
            "the engine sent a frame that is not JSON ({error}): {}",
            truncate(text, 200)
        )
    })?;
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        return Ok(Incoming::Event {
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        });
    }
    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return Err(format!(
            "the engine sent a frame with no id and no method: {}",
            truncate(text, 200)
        ));
    };
    let error = value.get("error").map(|error| {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the engine refused the call")
            .to_string();
        let data = error.get("data").map(|data| data.to_string());
        RpcError {
            code,
            message,
            data,
        }
    });
    Ok(Incoming::Response {
        id,
        result: value.get("result").cloned().unwrap_or(Value::Null),
        error,
    })
}

/// The first `limit` characters of `text`, for an error message.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}...")
}

/// A blocking WebSocket to one CDP endpoint.
///
/// Reads take a timeout so the engine's thread can do its other work (drain
/// outgoing requests, notice a stop request) between frames. A read that times
/// out is `Ok(None)`, not an error: the connection is healthy and simply had
/// nothing to say.
pub(crate) struct Socket {
    ws: WebSocket<TcpStream>,
}

impl Socket {
    /// Open a socket to a `ws://host:port/path` endpoint.
    pub(crate) fn connect(url: &str, timeout: Duration) -> Result<Self, String> {
        let (host, port, path) = parse_ws_url(url)?;
        let addrs = (host.as_str(), port).to_socket_addrs().map_err(|error| {
            format!("the engine's address {host}:{port} did not resolve: {error}")
        })?;
        let mut last = None;
        let mut stream = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(error) => last = Some(error),
            }
        }
        let Some(stream) = stream else {
            return Err(format!(
                "the engine's socket at {host}:{port} could not be opened: {}",
                last.map(|error| error.to_string())
                    .unwrap_or_else(|| String::from("no address"))
            ));
        };
        stream
            .set_nodelay(true)
            .map_err(|error| format!("the engine's socket could not be tuned: {error}"))?;
        let request = format!("ws://{host}:{port}{path}")
            .into_client_request()
            .map_err(|error| {
                format!("the engine's endpoint {url} is not a websocket URL: {error}")
            })?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| format!("the engine's socket could not be tuned: {error}"))?;
        let (ws, _response) = tungstenite::client::client(request, stream).map_err(|error| {
            format!("the engine refused the websocket handshake at {url}: {error}")
        })?;
        Ok(Self { ws })
    }

    /// Send one text frame.
    pub(crate) fn send(&mut self, text: &str) -> Result<(), String> {
        self.ws
            .send(Message::Text(text.to_string().into()))
            .map_err(|error| format!("the engine's socket refused a request: {error}"))
    }

    /// Read one text frame, waiting at most `timeout`.
    ///
    /// `Ok(None)` means the wait expired with nothing to read. Ping frames are
    /// answered here so a long capture does not lose the connection.
    pub(crate) fn read(&mut self, timeout: Duration) -> Result<Option<String>, String> {
        let stream = self.ws.get_mut();
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| format!("the engine's socket could not be tuned: {error}"))?;
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => {
                    let text = text.to_string();
                    if text.len() > MAX_FRAME {
                        return Err(format!(
                            "the engine sent a {} byte frame, above the {} byte bound",
                            text.len(),
                            MAX_FRAME
                        ));
                    }
                    return Ok(Some(text));
                }
                Ok(Message::Binary(bytes)) => {
                    return Err(format!(
                        "the engine sent a {} byte binary frame; CDP is text only",
                        bytes.len()
                    ));
                }
                Ok(Message::Ping(payload)) => {
                    self.ws
                        .send(Message::Pong(payload))
                        .map_err(|error| format!("the engine's socket refused a pong: {error}"))?;
                    continue;
                }
                Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => continue,
                Ok(Message::Close(_)) => {
                    return Err(String::from("the engine closed the websocket"));
                }
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(format!("the engine's socket failed: {error}")),
            }
        }
    }

    /// Say goodbye, best effort.
    pub(crate) fn close(&mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

/// Decode standard base64, which is how a captured frame travels.
///
/// The alphabet is RFC 4648's standard one (with `+` and `/`), padding is
/// optional, and whitespace is ignored: CDP sends one long line, but a test
/// fixture or a wrapped capture should still decode.
pub(crate) fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        buffer = (buffer << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Split a `ws://host:port/path` URL into its parts.
fn parse_ws_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("only ws:// endpoints are spoken here, not {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| format!("{url} has no numeric port"))?,
        ),
        None => return Err(format!("{url} has no port")),
    };
    if host.is_empty() {
        return Err(format!("{url} has no host"));
    }
    Ok((host, port, path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_carries_the_session_only_when_there_is_one() {
        let request = Request {
            id: 7,
            method: String::from("Page.navigate"),
            params: json!({ "url": "https://example.com" }),
            session: Some(String::from("page-1-session")),
        };
        let encoded: Value = serde_json::from_str(&request.encode()).unwrap();
        assert_eq!(encoded["id"], json!(7));
        assert_eq!(encoded["sessionId"], json!("page-1-session"));
        assert_eq!(encoded["params"]["url"], json!("https://example.com"));

        let bare = Request {
            session: None,
            ..request
        };
        let encoded: Value = serde_json::from_str(&bare.encode()).unwrap();
        assert!(encoded.get("sessionId").is_none());
    }

    #[test]
    fn a_response_and_an_event_are_told_apart_by_their_shape() {
        let response = decode(r#"{"id":3,"result":{"frameId":"page-1"}}"#).unwrap();
        match response {
            Incoming::Response { id, result, error } => {
                assert_eq!(id, 3);
                assert_eq!(result["frameId"], json!("page-1"));
                assert!(error.is_none());
            }
            other => panic!("expected a response, got {other:?}"),
        }
        let event =
            decode(r#"{"method":"Page.loadEventFired","params":{"timestamp":1.5}}"#).unwrap();
        match event {
            Incoming::Event { method, params } => {
                assert_eq!(method, "Page.loadEventFired");
                assert_eq!(params["timestamp"], json!(1.5));
            }
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn an_error_object_is_surfaced_typed() {
        let decoded = decode(
            r#"{"id":9,"error":{"code":-32601,"message":"No page for session","data":"detail"}}"#,
        )
        .unwrap();
        match decoded {
            Incoming::Response { error, .. } => {
                let error = error.expect("the error must survive decoding");
                assert_eq!(error.code, -32601);
                assert_eq!(error.message, "No page for session");
                assert_eq!(error.data.as_deref(), Some("\"detail\""));
                assert!(error.unsupported());
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_with_neither_id_nor_method_is_refused_by_name() {
        let error = decode(r#"{"result":{}}"#).unwrap_err();
        assert!(error.contains("no id and no method"), "{error}");
        assert!(decode("not json").unwrap_err().contains("not JSON"));
    }

    #[test]
    fn base64_decodes_the_rfc_vectors() {
        assert_eq!(decode_base64("").as_deref(), Some(&b""[..]));
        assert_eq!(decode_base64("Zg==").as_deref(), Some(&b"f"[..]));
        assert_eq!(decode_base64("Zm8=").as_deref(), Some(&b"fo"[..]));
        assert_eq!(decode_base64("Zm9v").as_deref(), Some(&b"foo"[..]));
        assert_eq!(decode_base64("Zm9vYmFy").as_deref(), Some(&b"foobar"[..]));
        assert_eq!(decode_base64("Zm9v\nYmFy").as_deref(), Some(&b"foobar"[..]));
        assert!(decode_base64("!!!!").is_none());
    }

    #[test]
    fn a_ws_url_splits_into_host_port_path() {
        let (host, port, path) = parse_ws_url("ws://127.0.0.1:9222/devtools/browser").unwrap();
        assert_eq!(
            (host.as_str(), port, path.as_str()),
            ("127.0.0.1", 9222, "/devtools/browser")
        );
        assert!(parse_ws_url("wss://example.com/x").is_err());
        assert!(parse_ws_url("ws://127.0.0.1/x").is_err());
    }
}
