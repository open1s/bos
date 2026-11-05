//! Process-global tap for the HTTP calls the built-in LLM vendors make.
//!
//! The vendors share one client builder, but not one request object, so the tap
//! wraps the send call itself. Metadata and headers are known as soon as the
//! response arrives; the bodies are not, so the report waits until the caller
//! consumes the response (text, json, or the stream it iterates). Bodies are
//! captured only while a tap is installed, are capped, and are never buffered
//! ahead of the caller's own read.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::{Client, Error, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Most body bytes one exchange carries, per side.
///
/// A body is for reading an exchange, not for archiving one: a prompt or an SSE
/// stream can run to megabytes, and every byte crosses a JSON boundary to reach
/// the package.
pub const TAP_BODY_CAP: usize = 32 * 1024;

/// Header values the tap never publishes, whatever their case.
///
/// A credential in a header is the ordinary shape, and a hook payload is
/// published to a package: masking at capture is the only place every caller
/// inherits it.
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "api-key",
    "x-api-key",
    "x-goog-api-key",
    "cookie",
    "set-cookie",
];

/// One observed LLM HTTP exchange.
#[derive(Debug, Clone, Serialize)]
pub struct HttpExchange {
    /// What made the call, for example "llm".
    pub label: &'static str,
    /// HTTP method.
    pub method: String,
    /// Full URL (query stays; the host redacts what it publishes).
    pub url: String,
    /// Status code, when a response arrived.
    pub status: Option<u16>,
    /// Correlation id of the model request this send belongs to.
    pub request_id: Option<String>,
    /// Request body bytes, when known.
    pub request_bytes: u64,
    /// Response body bytes from content-length, or the captured length.
    pub response_bytes: u64,
    /// Response content type.
    pub content_type: Option<String>,
    /// Time to response headers.
    pub ttfb_ms: u64,
    /// Time to the send call returning.
    pub total_ms: u64,
    /// Transport error, when the call failed.
    pub error: Option<String>,
    /// Request headers, with the secret values masked.
    pub request_headers: BTreeMap<String, String>,
    /// Response headers, with the secret values masked.
    pub response_headers: BTreeMap<String, String>,
    /// Request body, cut at the cap when a tap was installed.
    pub request_body: Option<String>,
    /// Response body, cut at the cap, once the caller consumed it.
    pub response_body: Option<String>,
    /// Whether either body was cut at the cap.
    pub body_truncated: bool,
}

/// A callback installed by the host.
pub type HttpTap = Arc<dyn Fn(HttpExchange) + Send + Sync>;

/// The byte stream a vendor iterates, which is also where the body is captured.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>;

static TAP: OnceLock<Mutex<Option<HttpTap>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<HttpTap>> {
    TAP.get_or_init(|| Mutex::new(None))
}

/// Install or clear the process-global tap.
pub fn set_http_tap(tap: Option<HttpTap>) {
    if let Ok(mut slot) = slot().lock() {
        *slot = tap;
    }
}

fn installed() -> bool {
    slot().lock().map(|slot| slot.is_some()).unwrap_or(false)
}

/// A body and whether the cap cut it.
fn bounded(bytes: &[u8], cap: usize) -> (String, bool) {
    if bytes.len() <= cap {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        (String::from_utf8_lossy(&bytes[..cap]).into_owned(), true)
    }
}

/// The headers worth publishing, with secret values masked.
fn headers_of(headers: &reqwest::header::HeaderMap) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in headers.iter() {
        let key = name.as_str().to_ascii_lowercase();
        let rendered = match value.to_str() {
            Ok(text) => text.to_string(),
            // A header that is not text is published as its byte length: the
            // alternative is dropping it and hiding that it existed.
            Err(_) => format!("<{} bytes>", value.as_bytes().len()),
        };
        let masked = if SECRET_HEADERS.contains(&key.as_str()) {
            "[redacted]".to_string()
        } else {
            rendered
        };
        out.entry(key)
            .and_modify(|existing| {
                existing.push_str(", ");
                existing.push_str(&masked);
            })
            .or_insert(masked);
    }
    out
}

/// What the tap knows before the body is read.
struct Trace {
    label: &'static str,
    method: String,
    url: String,
    status: Option<u16>,
    request_id: Option<String>,
    request_bytes: u64,
    content_type: Option<String>,
    ttfb_ms: u64,
    total_ms: u64,
    request_headers: BTreeMap<String, String>,
    response_headers: BTreeMap<String, String>,
    request_body: Option<String>,
    /// Whether the request half was already cut, so a clean response body does
    /// not clear the flag.
    request_truncated: bool,
}

impl Trace {
    fn finish(self, response_body: Option<String>, response_bytes: u64, truncated: bool) {
        report(HttpExchange {
            label: self.label,
            method: self.method,
            url: self.url,
            status: self.status,
            request_id: self.request_id,
            request_bytes: self.request_bytes,
            response_bytes,
            content_type: self.content_type,
            ttfb_ms: self.ttfb_ms,
            total_ms: self.total_ms,
            error: None,
            request_headers: self.request_headers,
            response_headers: self.response_headers,
            request_body: self.request_body,
            response_body,
            body_truncated: self.request_truncated || truncated,
        });
    }

    fn fail(self, error: String) {
        report(HttpExchange {
            label: self.label,
            method: self.method,
            url: self.url,
            status: self.status,
            request_id: self.request_id,
            request_bytes: self.request_bytes,
            response_bytes: 0,
            content_type: self.content_type,
            ttfb_ms: self.ttfb_ms,
            total_ms: self.total_ms,
            error: Some(error),
            request_headers: self.request_headers,
            response_headers: self.response_headers,
            request_body: self.request_body,
            response_body: None,
            body_truncated: self.request_truncated,
        });
    }
}

/// A response whose body is captured on the way to the caller.
///
/// The wrapper exists so the vendors keep their own reading code: status, text,
/// json and bytes_stream are the four calls they make, and each one is the place
/// the body becomes known.
pub struct TracedResponse {
    response: Response,
    trace: Option<Trace>,
}

impl TracedResponse {
    /// The status code, as reqwest reports it.
    pub fn status(&self) -> StatusCode {
        self.response.status()
    }

    /// Read the body as text, reporting it once.
    pub async fn text(self) -> Result<String, Error> {
        let Self { response, trace } = self;
        let read = response.text().await;
        if let Some(trace) = trace {
            match &read {
                Ok(body) => {
                    let (captured, truncated) = bounded(body.as_bytes(), TAP_BODY_CAP);
                    trace.finish(Some(captured), body.len() as u64, truncated);
                }
                Err(error) => trace.fail(error.to_string()),
            }
        }
        read
    }

    /// Read the body as JSON, reporting the raw bytes it was parsed from.
    pub async fn json<T: DeserializeOwned>(self) -> Result<T, JsonError> {
        let Self { response, trace } = self;
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                if let Some(trace) = trace {
                    trace.fail(error.to_string());
                }
                return Err(JsonError::Transport(error));
            }
        };
        let parsed = serde_json::from_slice::<T>(&bytes);
        if let Some(trace) = trace {
            let (captured, truncated) = bounded(&bytes, TAP_BODY_CAP);
            trace.finish(Some(captured), bytes.len() as u64, truncated);
        }
        parsed.map_err(JsonError::Parse)
    }

    /// Stream the body, capturing a bounded copy of it.
    pub fn bytes_stream(self) -> ByteStream {
        let Self { response, trace } = self;
        let Some(trace) = trace else {
            return Box::pin(response.bytes_stream());
        };
        let inner = response.bytes_stream();
        let mut collected: Vec<u8> = Vec::new();
        let mut truncated = false;
        Box::pin(stream! {
            futures::pin_mut!(inner);
            while let Some(chunk) = inner.next().await {
                if let Ok(bytes) = &chunk {
                    if !truncated {
                        let room = TAP_BODY_CAP.saturating_sub(collected.len());
                        if bytes.len() <= room {
                            collected.extend_from_slice(bytes);
                        } else {
                            collected.extend_from_slice(&bytes[..room]);
                            truncated = true;
                        }
                    }
                }
                yield chunk;
            }
            let (captured, _) = bounded(&collected, TAP_BODY_CAP);
            let total = collected.len() as u64;
            trace.finish(Some(captured), total, truncated);
        })
    }
}

/// Why a JSON body could not be read.
#[derive(Debug)]
pub enum JsonError {
    /// The transport failed before the body arrived.
    Transport(Error),
    /// The body was not the expected JSON.
    Parse(serde_json::Error),
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "{error}"),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for JsonError {}

/// Send one request and report the exchange to the tap.
///
/// The builder is sent by this function rather than by the caller, because the
/// request body is only readable once it is built: handing over a future would
/// make the payload impossible to see.
pub(crate) async fn traced_send(
    label: &'static str,
    method: &str,
    url: &str,
    request_id: Option<&str>,
    client: &Client,
    builder: RequestBuilder,
) -> Result<TracedResponse, Error> {
    if !installed() {
        return Ok(TracedResponse {
            response: builder.send().await?,
            trace: None,
        });
    }
    let request = builder.build()?;
    let body_bytes = request.body().and_then(|body| body.as_bytes());
    let request_bytes = body_bytes.map_or(0, |bytes| bytes.len() as u64);
    let (request_body, request_truncated) = match body_bytes {
        Some(bytes) => {
            let (text, cut) = bounded(bytes, TAP_BODY_CAP);
            (Some(text), cut)
        }
        None => (None, false),
    };
    let request_headers = headers_of(request.headers());
    let started = Instant::now();
    match client.execute(request).await {
        Ok(response) => {
            let ttfb_ms = started.elapsed().as_millis() as u64;
            let total_ms = ttfb_ms;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let response_headers = headers_of(response.headers());
            Ok(TracedResponse {
                response,
                trace: Some(Trace {
                    label,
                    method: method.to_string(),
                    url: url.to_string(),
                    status: Some(status),
                    request_id: request_id.map(str::to_string),
                    request_bytes,
                    content_type,
                    ttfb_ms,
                    total_ms,
                    request_headers,
                    response_headers,
                    request_body,
                    request_truncated,
                }),
            })
        }
        Err(error) => {
            report(HttpExchange {
                label,
                method: method.to_string(),
                url: url.to_string(),
                status: None,
                request_id: request_id.map(str::to_string),
                request_bytes,
                response_bytes: 0,
                content_type: None,
                ttfb_ms: 0,
                total_ms: started.elapsed().as_millis() as u64,
                error: Some(error.to_string()),
                request_headers,
                response_headers: BTreeMap::new(),
                request_body,
                response_body: None,
                body_truncated: request_truncated,
            });
            Err(error)
        }
    }
}

fn report(exchange: HttpExchange) {
    let tap = slot().lock().ok().and_then(|slot| slot.clone());
    if let Some(tap) = tap {
        tap(exchange);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The tap is process-global, so these tests take turns: two of them
    /// installing a sink at once would each capture the other's exchange.
    static ONE_TAP_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        ONE_TAP_AT_A_TIME
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// A one-shot server that answers with a fixed body.
    async fn serve(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut socket, _) = listener.accept().await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Trace: abc\r\nSet-Cookie: secret=1\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{addr}/thing")
    }

    async fn captured_once(url: &str) -> HttpExchange {
        let captured: Arc<Mutex<Vec<HttpExchange>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        set_http_tap(Some(Arc::new(move |exchange| {
            sink.lock().unwrap().push(exchange);
        })));
        let client = Client::new();
        let builder = client
            .post(url)
            .header("X-Api-Key", "sk-live-1")
            .header("Authorization", "Bearer sk-live-1");
        let response = traced_send("test", "POST", url, Some("req-1"), &client, builder)
            .await
            .expect("response");
        assert_eq!(response.status().as_u16(), 200);
        let _ = response.text().await.expect("body");
        let event = captured
            .lock()
            .unwrap()
            .pop()
            .expect("one exchange once the body is read");
        set_http_tap(None);
        event
    }

    // The guard is the point: it serialises these tests around a process-global
    // sink. The runtime is current-thread, so holding it across an await cannot
    // stall another test's task.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_exchange_carries_both_bodies_and_both_header_sets() {
        let _turn = exclusive();
        let url = serve("hello").await;
        let event = captured_once(&url).await;
        assert_eq!(event.status, Some(200));
        assert_eq!(event.response_body.as_deref(), Some("hello"));
        assert_eq!(event.response_bytes, 5);
        assert_eq!(
            event
                .request_headers
                .get("authorization")
                .map(String::as_str),
            Some("[redacted]"),
            "a credential header is masked at capture"
        );
        assert_eq!(
            event.request_headers.get("x-api-key").map(String::as_str),
            Some("[redacted]")
        );
        assert_eq!(
            event.response_headers.get("x-trace").map(String::as_str),
            Some("abc"),
            "an ordinary header survives"
        );
        assert_eq!(
            event.response_headers.get("set-cookie").map(String::as_str),
            Some("[redacted]")
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_body_over_the_cap_is_cut_and_flagged() {
        let _turn = exclusive();
        let big: &'static str = Box::leak("x".repeat(TAP_BODY_CAP + 16).into_boxed_str());
        let url = serve(big).await;
        let event = captured_once(&url).await;
        let body = event.response_body.expect("body");
        assert_eq!(body.len(), TAP_BODY_CAP, "the capture stops at the cap");
        assert!(event.body_truncated, "and says so");
    }
}
