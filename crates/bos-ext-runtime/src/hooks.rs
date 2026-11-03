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

/// Build one HTTP exchange observation.
///
/// Header values and bodies are deliberately absent: this layer records the
/// shape of the exchange (status, sizes, timing), and capturing a body is a
/// separate, explicit decision.
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
    let parsed = url.split_once("://").map(|(scheme, rest)| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = host.rsplit('@').next().unwrap_or(host);
        let host = host.split(':').next().unwrap_or(host);
        let path = rest
            .find('/')
            .map(|index| rest[index..].split(['?', '#']).next().unwrap_or("/"))
            .unwrap_or("/");
        (scheme.to_string(), host.to_string(), path.to_string())
    });
    json!({
        "source": source,
        "request_id": Value::Null,
        "method": method,
        "host": parsed.as_ref().map(|(_, host, _)| host.as_str()),
        "path": parsed.as_ref().map(|(_, _, path)| path.as_str()),
        "status": status,
        "request_bytes": request_bytes,
        "response_bytes": response_bytes,
        "content_type": content_type,
        "ttfb_ms": ttfb_ms,
        "total_ms": total_ms,
        "error": error,
    })
}

/// Redact credentials and bound one hook field before it leaves the host.
pub fn redact(raw: &str) -> String {
    let mut text = raw.to_string();
    for marker in ["sk-", "ghp_", "Bearer "] {
        text = mask_after(&text, marker);
    }
    text.chars().take(8 * 1024).collect()
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
    fn an_http_exchange_carries_shape_not_bodies() {
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
        assert!(event.get("headers").is_none());
        assert!(event.get("body").is_none());
    }

}
