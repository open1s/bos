//! Observe hooks: the host pushes selected seam events to attached runtimes.
//!
//! A hook is not an event. An event is published after the fact and is visible
//! to any subscriber; a hook is delivered to the package that declared that
//! point, from a bounded queue, in batches, and never blocks the seam that
//! produced it. Intercept hooks (which may deny or replace) are a separate
//! class and are not implemented by this module yet.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

/// The hook points this build dispatches. Adding one is a standard change.
pub const HOOK_POINTS: [&str; 5] = [
    "model.request",
    "model.response",
    "tool.before",
    "tool.after",
    "http.exchange",
];

/// Observe queue depth. A full queue drops and counts; it never blocks.
const QUEUE_CAPACITY: usize = 1024;

/// The most events one batch may carry, so one guest call cannot grow without
/// bound while a stream is running.
const MAX_BATCH: usize = 64;

struct Queue {
    events: VecDeque<(&'static str, Value)>,
    dropped: u64,
}

static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
static WORKER: OnceLock<()> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

fn queue() -> &'static Mutex<Queue> {
    QUEUE.get_or_init(|| {
        Mutex::new(Queue {
            events: VecDeque::new(),
            dropped: 0,
        })
    })
}

/// Queue one observe hook event for every attached runtime.
///
/// This is called from the seam that owns the work (today the model observer)
/// and returns immediately: the worker batches and dispatches off-thread.
pub fn observe(point: &'static str, payload: Value) {
    if !HOOK_POINTS.contains(&point) {
        return;
    }
    match queue().lock() {
        Ok(mut queue) => {
            if queue.events.len() >= QUEUE_CAPACITY {
                queue.dropped = queue.dropped.saturating_add(1);
                DROPPED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            queue.events.push_back((point, payload));
        }
        Err(_) => return,
    }
    start_worker();
}

/// How many observe events this process dropped.
#[cfg(test)]
pub fn dropped() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

fn start_worker() {
    WORKER.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("bsh-hook-observe".to_string())
            .spawn(worker);
    });
}

fn worker() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    loop {
        let batch = take_batch();
        if batch.is_empty() {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        // Group by point so one guest call carries one point's batch.
        let mut by_point: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
        for (point, payload) in batch {
            by_point.entry(point).or_default().push(payload);
        }
        for (point, events) in by_point {
            let targets = crate::hook::targets(point);
            if targets.is_empty() {
                continue;
            }
            let payload = json!({ "point": point, "batch": events });
            let Ok(runtime) = &runtime else {
                continue;
            };
            runtime.block_on(async {
                for target in targets {
                    // One package must not stop delivery to the others, but a
                    // silent failure would make an unattached observer look
                    // healthy: name the point and the package.
                    if let Err(error) = target.runtime.hook(point, payload.clone()).await {
                        log::warn!("extension {} hook {point} failed: {error}", target.id);
                    }
                }
            });
        }
    }
}

fn take_batch() -> Vec<(&'static str, Value)> {
    let Ok(mut queue) = queue().lock() else {
        return Vec::new();
    };
    let count = queue.events.len().min(MAX_BATCH);
    queue.events.drain(..count).collect()
}

/// One HTTP exchange as the host observed it.
///
/// The headers and the bodies are optional and bounded: a host that captures
/// nothing still publishes the shape, and a host that captures a body publishes
/// at most what its cap allows. Secret header VALUES are masked before they get
/// here; a body is passed through [redact] so a credential inside a payload is
/// masked too.
#[derive(Default)]
pub struct HttpEvent<'a> {
    /// What made the call, for example "llm".
    pub source: &'a str,
    /// HTTP method.
    pub method: &'a str,
    /// Full URL.
    pub url: &'a str,
    /// Status code, when a response arrived.
    pub status: Option<u16>,
    /// Correlation id of the model request.
    pub request_id: Option<&'a str>,
    /// Request body bytes.
    pub request_bytes: u64,
    /// Response body bytes.
    pub response_bytes: u64,
    /// Response content type.
    pub content_type: Option<&'a str>,
    /// Time to response headers.
    pub ttfb_ms: u64,
    /// Time to the send call returning.
    pub total_ms: u64,
    /// Transport error, when the call failed.
    pub error: Option<&'a str>,
    /// Request headers, already masked.
    pub request_headers: &'a [(String, String)],
    /// Response headers, already masked.
    pub response_headers: &'a [(String, String)],
    /// Request body, cut at the host cap.
    pub request_body: Option<&'a str>,
    /// Response body, cut at the host cap.
    pub response_body: Option<&'a str>,
    /// Whether either body was cut.
    pub body_truncated: bool,
}

/// Build one HTTP exchange observation with headers and payloads.
pub fn http_exchange_event(event: HttpEvent<'_>) -> Value {
    let parsed = event.url.split_once("://").map(|(scheme, rest)| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = host.rsplit('@').next().unwrap_or(host);
        let host = host.split(':').next().unwrap_or(host);
        let path = rest
            .find('/')
            .map(|index| rest[index..].split(['?', '#']).next().unwrap_or("/"))
            .unwrap_or("/");
        (scheme.to_string(), host.to_string(), path.to_string())
    });
    let headers = |pairs: &[(String, String)]| {
        let mut map = serde_json::Map::new();
        for (name, value) in pairs {
            map.insert(name.clone(), Value::String(value.clone()));
        }
        Value::Object(map)
    };
    json!({
        "source": event.source,
        "request_id": event.request_id,
        "method": event.method,
        "host": parsed.as_ref().map(|(_, host, _)| host.as_str()),
        "path": parsed.as_ref().map(|(_, _, path)| path.as_str()),
        "status": event.status,
        "request_bytes": event.request_bytes,
        "response_bytes": event.response_bytes,
        "content_type": event.content_type,
        "ttfb_ms": event.ttfb_ms,
        "total_ms": event.total_ms,
        "error": event.error,
        "request_headers": headers(event.request_headers),
        "response_headers": headers(event.response_headers),
        "request_body": event.request_body,
        "response_body": event.response_body,
        "body_truncated": event.body_truncated,
    })
}

/// Build one HTTP exchange observation from metadata alone.
///
/// The shape-only form the sandbox runtimes report: they have no body to hand
/// the host, so the payload fields stay absent rather than empty.
#[allow(clippy::too_many_arguments)]
pub fn http_exchange(
    source: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    request_bytes: u64,
    response_bytes: u64,
    content_type: Option<&str>,
    ttfb_ms: u64,
    total_ms: u64,
    error: Option<&str>,
) -> Value {
    http_exchange_event(HttpEvent {
        source,
        method,
        url,
        status,
        request_bytes,
        response_bytes,
        content_type,
        ttfb_ms,
        total_ms,
        error,
        ..Default::default()
    })
}

/// Redact credentials and bound one hook field before it leaves the host.
pub fn redact(raw: &str) -> String {
    redact_bounded(raw, 8 * 1024)
}

/// Redact one PAYLOAD field: the same masking, with room for a body.
///
/// A metadata value is a sentence; a body is a request. The 8 KiB metadata cap
/// would cut a prompt in half, so a payload gets the tap's own cap.
pub fn redact_payload(raw: &str) -> String {
    redact_bounded(raw, 32 * 1024)
}

fn redact_bounded(raw: &str, cap: usize) -> String {
    let mut text = raw.to_string();
    for marker in ["sk-", "ghp_", "Bearer "] {
        text = mask_after(&text, marker);
    }
    text.chars().take(cap).collect()
}

fn mask_after(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find(marker) {
        out.push_str(&rest[..position + marker.len()]);
        let tail = &rest[position + marker.len()..];
        let end = tail
            .find(|character: char| {
                !(character.is_ascii_alphanumeric()
                    || matches!(character, '_' | '-' | '/' | '+' | '='))
            })
            .unwrap_or(tail.len());
        if end > 0 {
            out.push_str("[redacted]");
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_masked_before_a_hook_sees_them() {
        let redacted =
            redact("Authorization: Bearer abc123 sk-live-999 ghp_secretvalue and keep this.");
        assert!(!redacted.contains("abc123"), "{redacted}");
        assert!(!redacted.contains("live-999"), "{redacted}");
        assert!(!redacted.contains("secretvalue"), "{redacted}");
        assert!(redacted.contains("keep this."), "{redacted}");
    }

    #[test]
    fn a_hook_field_is_bounded() {
        let redacted = redact(&"x".repeat(20_000));
        assert_eq!(redacted.chars().count(), 8 * 1024);
    }

    #[test]
    fn every_declared_point_is_dispatchable() {
        assert!(HOOK_POINTS.contains(&"http.exchange"));
        assert!(HOOK_POINTS.contains(&"model.request"));
        assert_eq!(HOOK_POINTS.len(), 5);
    }

    #[test]
    fn a_shape_only_exchange_omits_the_payload_it_never_had() {
        let event = http_exchange(
            "com.example.source",
            "POST",
            "https://api.example.com/v1/chat?key=secret",
            Some(200),
            120,
            3400,
            Some("application/json"),
            41,
            90,
            None,
        );
        assert_eq!(event["host"], "api.example.com");
        assert_eq!(event["path"], "/v1/chat");
        assert_eq!(event["status"], 200);
        assert_eq!(
            event["request_body"],
            Value::Null,
            "a runtime with no body reports none rather than an empty string"
        );
        assert_eq!(
            event["request_headers"].as_object().map(|map| map.len()),
            Some(0)
        );
    }

    #[test]
    fn an_http_exchange_carries_headers_and_bodies_when_it_has_them() {
        let request_headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("authorization".to_string(), "[redacted]".to_string()),
        ];
        let response_headers = vec![("content-type".to_string(), "text/event-stream".to_string())];
        let body = redact_payload("{\"prompt\":\"hello\"}");
        let event = http_exchange_event(HttpEvent {
            source: "llm",
            method: "POST",
            url: "https://api.example.com/v1/chat?key=secret",
            status: Some(200),
            request_id: Some("req-1"),
            request_bytes: 20,
            response_bytes: 40,
            content_type: Some("text/event-stream"),
            ttfb_ms: 12,
            total_ms: 400,
            error: None,
            request_headers: &request_headers,
            response_headers: &response_headers,
            request_body: Some(&body),
            response_body: Some("data: {}\n\n"),
            body_truncated: false,
        });
        assert_eq!(event["request_id"], "req-1");
        assert_eq!(event["request_headers"]["authorization"], "[redacted]");
        assert_eq!(
            event["response_headers"]["content-type"],
            "text/event-stream"
        );
        assert_eq!(event["request_body"], body);
        assert!(event["response_body"]
            .as_str()
            .unwrap_or_default()
            .contains("data"));
        assert_eq!(event["body_truncated"], false);
    }
}
