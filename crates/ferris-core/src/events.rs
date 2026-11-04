//! The bounded extension event bus.
//!
//! Commands are request/response, streams are long-lived output, and events are
//! notifications. This module owns the third primitive only. Every subscriber
//! gets a bounded queue: when it is full the event is dropped and counted, never
//! buffered without limit and never allowed to block the publisher. An event
//! that does not fit the payload ceiling is refused at construction.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{ExtensionError, ExtensionId};

/// The event schema generation.
pub const EVENT_SCHEMA_VERSION: &str = "1.0";

/// The largest serialized event this host accepts.
pub const MAX_EVENT_BYTES: usize = 256 * 1024;

/// The default per-subscriber queue depth.
pub const DEFAULT_QUEUE_DEPTH: usize = 64;

/// The most events one publisher may emit in a second.
pub const MAX_EVENTS_PER_SECOND: u64 = 200;

/// The largest publisher table the per-source rate limiter keeps in one second.
///
/// The table is swept when it fills; a new publisher beyond the sweep is
/// refused rather than allowed to grow the map without bound.
pub const MAX_TRACKED_PUBLISHERS: usize = 4096;

/// Topics the platform itself publishes. A package may subscribe to any
/// declared topic; these are the reserved ones its subscriptions are checked
/// against by documentation, not by a hard allowlist.
pub mod topics {
    /// The host finished starting.
    pub const APP_STARTED: &str = "app.started";
    /// The host is shutting down.
    pub const APP_SHUTDOWN: &str = "app.shutdown";
    /// A workspace opened.
    pub const WORKSPACE_OPENED: &str = "workspace.opened";
    /// A workspace closed.
    pub const WORKSPACE_CLOSED: &str = "workspace.closed";
    /// The active workspace or its open files changed.
    pub const WORKSPACE_CHANGED: &str = "workspace.changed";
    /// An agent turn started.
    pub const AGENT_STARTED: &str = "agent.started";
    /// An agent turn completed.
    pub const AGENT_COMPLETED: &str = "agent.completed";
    /// An agent turn failed.
    pub const AGENT_FAILED: &str = "agent.failed";
    /// A tool was invoked.
    pub const TOOL_INVOKED: &str = "tool.invoked";
    /// A tool completed.
    pub const TOOL_COMPLETED: &str = "tool.completed";
    /// A tool failed.
    pub const TOOL_FAILED: &str = "tool.failed";
    /// An extension became active.
    pub const EXTENSION_LOADED: &str = "extension.loaded";
    /// An extension stopped.
    pub const EXTENSION_UNLOADED: &str = "extension.unloaded";
    /// An extension failed.
    pub const EXTENSION_FAILED: &str = "extension.failed";
    /// A declarative UI action was routed to its owning runtime.
    pub const UI_ACTION: &str = "ui.action";
    /// A model request is about to be sent.
    pub const MODEL_REQUESTED: &str = "model.requested";
    /// A model request finished.
    pub const MODEL_COMPLETED: &str = "model.completed";
    /// The reader changed an extension configuration document.
    pub const CONFIG_CHANGED: &str = "config.changed";
    /// A session was created.
    pub const SESSION_CREATED: &str = "session.created";
    /// A session title or transcript changed.
    pub const SESSION_UPDATED: &str = "session.updated";
    /// A session was deleted.
    pub const SESSION_DELETED: &str = "session.deleted";
    /// One turn settled into the trajectory.
    pub const TRAJECTORY_RECORDED: &str = "trajectory.recorded";
    /// A skill entered the built agent.
    pub const SKILL_LOADED: &str = "skill.loaded";
    /// The agent's instruction set was composed.
    pub const INSTRUCTION_BUILD: &str = "instruction.build";
    /// An extension asked the host for an outbound request.
    pub const NETWORK_REQUEST: &str = "network.request";
    /// The MCP inventory was reconciled.
    pub const MCP_CONNECTED: &str = "mcp.connected";
    /// An audio attachment was captured.
    pub const AUDIO_CAPTURED: &str = "audio.captured";
}

/// One event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionEvent {
    /// Unique event id.
    pub id: String,
    /// Publishing extension or "bsh" for the host.
    pub source: String,
    /// Dotted topic.
    pub event_type: String,
    /// Envelope schema generation.
    pub schema_version: String,
    /// Unix milliseconds.
    pub timestamp: u64,
    /// The request or turn this event belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Event body.
    pub payload: Value,
}

impl ExtensionEvent {
    /// Build a host event.
    pub fn new(
        source: impl Into<String>,
        event_type: impl Into<String>,
        payload: Value,
    ) -> Result<Self, ExtensionError> {
        let event = Self {
            id: uuid::Uuid::new_v4().to_string(),
            source: source.into(),
            event_type: event_type.into(),
            schema_version: EVENT_SCHEMA_VERSION.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or_default(),
            correlation_id: None,
            payload,
        };
        event.validate()?;
        Ok(event)
    }

    /// Attach a correlation id.
    pub fn correlated(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    /// Check the topic and the payload ceiling.
    pub fn validate(&self) -> Result<(), ExtensionError> {
        TopicPattern::parse(&self.event_type, false).map_err(|reason| {
            ExtensionError::Protocol {
                extension: self.source.clone(),
                message: format!("invalid event topic {:?}: {reason}", self.event_type),
            }
        })?;
        let bytes = serde_json::to_vec(self).map_err(|error| ExtensionError::Protocol {
            extension: self.source.clone(),
            message: error.to_string(),
        })?;
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(ExtensionError::Limit {
                extension: self.source.clone(),
                message: format!(
                    "event is {} bytes; the ceiling is {MAX_EVENT_BYTES}",
                    bytes.len()
                ),
            });
        }
        if !json_depth_ok(&self.payload, 0) {
            return Err(ExtensionError::Protocol {
                extension: self.source.clone(),
                message: "event payload nests deeper than 32 levels".to_string(),
            });
        }
        Ok(())
    }

    /// Serialized size in bytes.
    pub fn size(&self) -> usize {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .unwrap_or(0)
    }
}

/// One dotted topic pattern, with an optional trailing wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicPattern {
    segments: Vec<String>,
    wildcard: bool,
}

impl TopicPattern {
    /// Parse and validate a topic or pattern.
    pub fn parse(raw: &str, allow_wildcard: bool) -> Result<Self, String> {
        if raw.is_empty() || raw.len() > 128 {
            return Err("a topic must be 1..=128 bytes".to_string());
        }
        let segments: Vec<&str> = raw.split('.').collect();
        let mut wildcard = false;
        let mut parsed = Vec::with_capacity(segments.len());
        for (index, segment) in segments.iter().enumerate() {
            if *segment == "*" {
                if !allow_wildcard || index != segments.len() - 1 {
                    return Err("'*' is only allowed as the final segment of a pattern".to_string());
                }
                wildcard = true;
                continue;
            }
            if segment.is_empty() {
                return Err("topic segments must not be empty".to_string());
            }
            if !segment.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'
                    || character == '_'
            }) {
                return Err(format!(
                    "topic segment {segment:?} may only contain lowercase letters, digits, '-' and '_'"
                ));
            }
            parsed.push((*segment).to_string());
        }
        if parsed.is_empty() {
            return Err("a pattern needs at least one non-wildcard segment".to_string());
        }
        Ok(Self {
            segments: parsed,
            wildcard,
        })
    }

    /// Whether this pattern selects `topic`.
    pub fn matches(&self, topic: &str) -> bool {
        if self.wildcard {
            let prefix = format!("{}.", self.segments.join("."));
            return topic.starts_with(&prefix);
        }
        topic == self.segments.join(".")
    }
}

/// A subscriber's topic filter.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    patterns: Vec<TopicPattern>,
}

impl EventFilter {
    /// Select nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// Select everything.
    pub fn all() -> Self {
        Self {
            patterns: vec![TopicPattern {
                segments: Vec::new(),
                wildcard: true,
            }],
        }
    }

    /// Build from topic strings. A trailing `.*` selects a namespace.
    pub fn from_topics(topics: &[String]) -> Result<Self, String> {
        let mut patterns = Vec::with_capacity(topics.len());
        for topic in topics {
            patterns.push(TopicPattern::parse(topic, true)?);
        }
        Ok(Self { patterns })
    }

    /// Whether the filter selects `topic`.
    pub fn matches(&self, topic: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        if self
            .patterns
            .iter()
            .any(|pattern| pattern.segments.is_empty() && pattern.wildcard)
        {
            return true;
        }
        self.patterns.iter().any(|pattern| pattern.matches(topic))
    }

    /// The patterns as written.
    pub fn topics(&self) -> Vec<String> {
        self.patterns
            .iter()
            .map(|pattern| {
                let mut text = pattern.segments.join(".");
                if pattern.wildcard {
                    if !text.is_empty() {
                        text.push('.');
                    }
                    text.push('*');
                }
                text
            })
            .collect()
    }
}

/// Counters for the bus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BusMetrics {
    /// Events accepted for publication.
    pub published: u64,
    /// Deliveries accepted by a subscriber queue.
    pub delivered: u64,
    /// Deliveries dropped because a queue was full.
    pub dropped: u64,
    /// Events refused before publication.
    pub rejected: u64,
}

/// What one publish did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishReport {
    /// Subscriber queues that accepted the event.
    pub delivered: usize,
    /// Subscriber queues that were full.
    pub dropped: usize,
}

struct Subscriber {
    filter: EventFilter,
    sender: async_channel::Sender<ExtensionEvent>,
}

/// The event bus. Cloneable by Arc, never by value.
pub struct EventBus {
    subscribers: Mutex<BTreeMap<u64, Subscriber>>,
    next_id: AtomicU64,
    metrics: Mutex<BusMetrics>,
    /// publisher -> (unix second, count in that second)
    rate: Mutex<BTreeMap<String, (u64, u64)>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            subscribers: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            metrics: Mutex::new(BusMetrics::default()),
            rate: Mutex::new(BTreeMap::new()),
        }
    }
}

impl EventBus {
    /// A new bus with no subscribers.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Subscribe with a bounded queue.
    ///
    /// A poisoned subscriber map is an error, not a subscription that silently
    /// never receives anything: the caller must know its handler is not wired.
    pub fn subscribe(
        self: &Arc<Self>,
        filter: EventFilter,
        capacity: usize,
    ) -> Result<Subscription, ExtensionError> {
        let capacity = capacity.clamp(1, 4096);
        let (sender, receiver) = async_channel::bounded(capacity);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| ExtensionError::Protocol {
                extension: "bus".to_string(),
                message: "event bus subscriber map is poisoned".to_string(),
            })?;
        subscribers.insert(id, Subscriber { filter, sender });
        Ok(Subscription {
            id,
            receiver,
            bus: Arc::downgrade(self),
        })
    }

    /// Publish one event.
    pub fn publish(&self, event: &ExtensionEvent) -> Result<PublishReport, ExtensionError> {
        if let Err(error) = event.validate() {
            if let Ok(mut metrics) = self.metrics.lock() {
                metrics.rejected = metrics.rejected.saturating_add(1);
            }
            return Err(error);
        }
        let second = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let mut rate = match self.rate.lock() {
            Ok(rate) => rate,
            Err(_) => {
                if let Ok(mut metrics) = self.metrics.lock() {
                    metrics.rejected = metrics.rejected.saturating_add(1);
                }
                return Err(ExtensionError::Protocol {
                    extension: event.source.clone(),
                    message: "event bus rate limiter lock is poisoned".to_string(),
                });
            }
        };
        if !rate.contains_key(&event.source) && rate.len() >= MAX_TRACKED_PUBLISHERS {
            // The table only grows when distinct sources publish. Sweep the
            // previous second's windows before refusing a new source, so an
            // idle publisher's entry is reclaimed instead of accumulating.
            rate.retain(|_, (started, _)| *started == second);
            if rate.len() >= MAX_TRACKED_PUBLISHERS {
                if let Ok(mut metrics) = self.metrics.lock() {
                    metrics.rejected = metrics.rejected.saturating_add(1);
                }
                return Err(ExtensionError::Limit {
                    extension: event.source.clone(),
                    message: format!(
                        "more than {MAX_TRACKED_PUBLISHERS} distinct publishers in one second"
                    ),
                });
            }
        }
        let entry = rate.entry(event.source.clone()).or_insert((second, 0));
        if entry.0 != second {
            *entry = (second, 0);
        }
        if entry.1 >= MAX_EVENTS_PER_SECOND {
            if let Ok(mut metrics) = self.metrics.lock() {
                metrics.rejected = metrics.rejected.saturating_add(1);
            }
            return Err(ExtensionError::Limit {
                extension: event.source.clone(),
                message: format!("more than {MAX_EVENTS_PER_SECOND} events in one second"),
            });
        }
        entry.1 = entry.1.saturating_add(1);
        drop(rate);
        let subscribers = match self.subscribers.lock() {
            Ok(subscribers) => subscribers,
            Err(_) => {
                return Err(ExtensionError::Protocol {
                    extension: event.source.clone(),
                    message: "event bus lock is poisoned".to_string(),
                });
            }
        };
        let mut report = PublishReport::default();
        let mut dropped = 0u64;
        for subscriber in subscribers.values() {
            if !subscriber.filter.matches(&event.event_type) {
                continue;
            }
            if subscriber.sender.try_send(event.clone()).is_ok() {
                report.delivered += 1;
            } else {
                report.dropped += 1;
                dropped += 1;
            }
        }
        drop(subscribers);
        if let Ok(mut metrics) = self.metrics.lock() {
            metrics.published = metrics.published.saturating_add(1);
            metrics.delivered = metrics.delivered.saturating_add(report.delivered as u64);
            metrics.dropped = metrics.dropped.saturating_add(dropped);
        }
        Ok(report)
    }

    /// A snapshot of the counters.
    pub fn metrics(&self) -> BusMetrics {
        self.metrics
            .lock()
            .map(|metrics| *metrics)
            .unwrap_or_default()
    }

    /// The number of publisher windows currently tracked, for the reclamation
    /// test.
    #[cfg(test)]
    fn tracked_publishers(&self) -> usize {
        self.rate.lock().map(|rate| rate.len()).unwrap_or(0)
    }

    /// The number of live subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .map(|subscribers| subscribers.len())
            .unwrap_or(0)
    }

    fn unsubscribe(&self, id: u64) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.remove(&id);
        }
    }
}

/// One live subscription. Dropping it unsubscribes.
pub struct Subscription {
    id: u64,
    receiver: async_channel::Receiver<ExtensionEvent>,
    bus: Weak<EventBus>,
}

impl Subscription {
    /// The subscription id.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Wait for the next event.
    pub async fn recv(&self) -> Result<ExtensionEvent, async_channel::RecvError> {
        self.receiver.recv().await
    }

    /// Take the next event without waiting.
    pub fn try_recv(&self) -> Option<ExtensionEvent> {
        self.receiver.try_recv().ok()
    }

    /// The number of queued events.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    /// Whether no event is queued.
    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(bus) = self.bus.upgrade() {
            bus.unsubscribe(self.id);
        }
    }
}

/// Build an extension lifecycle event.
/// Reject a JSON value nested more than 32 levels deep.
fn json_depth_ok(value: &Value, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    match value {
        Value::Array(items) => items.iter().all(|item| json_depth_ok(item, depth + 1)),
        Value::Object(map) => map.values().all(|item| json_depth_ok(item, depth + 1)),
        _ => true,
    }
}

/// Build one lifecycle event for an extension state transition.
pub fn lifecycle_event(
    extension: &ExtensionId,
    topic: &str,
    state: &str,
    error: Option<&str>,
) -> Result<ExtensionEvent, ExtensionError> {
    let mut payload = serde_json::json!({
        "extension": extension.as_str(),
        "state": state,
    });
    if let Some(error) = error {
        payload["error"] = Value::String(error.to_string());
    }
    ExtensionEvent::new("bsh", topic, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_match_exact_prefix_and_wildcards() {
        let filter =
            EventFilter::from_topics(&["workspace.opened".to_string(), "tool.*".to_string()])
                .expect("filter");
        assert!(filter.matches("workspace.opened"));
        assert!(!filter.matches("workspace.closed"));
        assert!(filter.matches("tool.invoked"));
        assert!(filter.matches("tool.failed.again"));
        assert!(!filter.matches("agent.started"));
        assert!(EventFilter::all().matches("anything.at.all"));
        assert!(!EventFilter::none().matches("anything"));
    }

    #[test]
    fn malformed_topics_are_refused() {
        for topic in ["", "Workspace", "a..b", "*", "a.*.b"] {
            assert!(TopicPattern::parse(topic, true).is_err(), "{topic}");
        }
        assert!(TopicPattern::parse("a.*", true).is_ok());
        assert!(TopicPattern::parse("a.*", false).is_err());
    }

    #[test]
    fn a_full_queue_drops_and_counts_instead_of_blocking() {
        let bus = EventBus::new();
        let subscription = bus.subscribe(EventFilter::all(), 1).expect("subscribe");
        let first =
            ExtensionEvent::new("bsh", "app.started", serde_json::json!({})).expect("event");
        let second =
            ExtensionEvent::new("bsh", "app.started", serde_json::json!({})).expect("event");
        assert_eq!(bus.publish(&first).expect("publish").delivered, 1);
        assert_eq!(bus.publish(&second).expect("publish").delivered, 0);
        assert_eq!(bus.metrics().dropped, 1);
        assert_eq!(
            subscription.try_recv().expect("first").event_type,
            "app.started"
        );
        assert!(subscription.try_recv().is_none());
    }

    #[test]
    fn one_publisher_cannot_flood_the_bus() {
        let bus = EventBus::new();
        let mut last = None;
        for _ in 0..=MAX_EVENTS_PER_SECOND {
            let event =
                ExtensionEvent::new("flood", "app.started", serde_json::json!({})).expect("event");
            last = Some(bus.publish(&event));
        }
        let error = last.expect("a publish").expect_err("over the rate");
        assert_eq!(error.category(), "limit");
        assert!(bus.metrics().rejected >= 1);
    }

    #[test]
    fn every_hook_topic_is_a_valid_reserved_topic() {
        for topic in [
            topics::SESSION_CREATED,
            topics::SESSION_UPDATED,
            topics::SESSION_DELETED,
            topics::TRAJECTORY_RECORDED,
            topics::SKILL_LOADED,
            topics::INSTRUCTION_BUILD,
            topics::NETWORK_REQUEST,
            topics::MCP_CONNECTED,
            topics::AUDIO_CAPTURED,
        ] {
            ExtensionEvent::new("bsh", topic, serde_json::json!({})).expect(topic);
        }
    }

    #[test]
    fn a_deeply_nested_event_is_refused() {
        let mut payload = serde_json::json!(0);
        for _ in 0..40 {
            payload = serde_json::json!([payload]);
        }
        let error = ExtensionEvent::new("bsh", "app.started", payload).expect_err("depth");
        assert_eq!(error.category(), "runtime");
    }

    #[test]
    fn an_oversized_event_is_refused() {
        let bus = EventBus::new();
        let payload = serde_json::json!({ "text": "x".repeat(MAX_EVENT_BYTES) });
        let error = ExtensionEvent::new("bsh", "app.started", payload).expect_err("oversized");
        assert_eq!(error.category(), "limit");
        assert_eq!(bus.metrics().rejected, 0);
    }

    #[test]
    fn dropping_a_subscription_unsubscribes_it() {
        let bus = EventBus::new();
        {
            let _subscription = bus.subscribe(EventFilter::all(), 4).expect("subscribe");
            assert_eq!(bus.subscriber_count(), 1);
        }
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn a_poisoned_rate_limiter_fails_closed() {
        let bus = EventBus::new();
        let poison = Arc::clone(&bus);
        let _ = std::thread::spawn(move || {
            let _guard = poison.rate.lock().expect("rate lock");
            panic!("poison the rate limiter");
        })
        .join();
        let event =
            ExtensionEvent::new("bsh", "app.started", serde_json::json!({})).expect("event");
        let error = bus.publish(&event).expect_err("poisoned rate limiter");
        assert!(matches!(error, ExtensionError::Protocol { .. }), "{error}");
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn a_poisoned_subscriber_map_refuses_a_subscription() {
        let bus = EventBus::new();
        let poison = Arc::clone(&bus);
        let _ = std::thread::spawn(move || {
            let _guard = poison.subscribers.lock().expect("subscriber lock");
            panic!("poison the subscriber map");
        })
        .join();
        let error = match bus.subscribe(EventFilter::all(), 4) {
            Ok(_) => panic!("a poisoned subscriber map must not return a subscription"),
            Err(error) => error,
        };
        assert!(matches!(error, ExtensionError::Protocol { .. }), "{error}");
    }

    #[test]
    fn stale_publisher_windows_are_reclaimed() {
        let bus = EventBus::new();
        {
            let mut rate = bus.rate.lock().expect("rate");
            for index in 0..MAX_TRACKED_PUBLISHERS {
                rate.insert(format!("stale-{index}"), (0, 1));
            }
        }
        let event =
            ExtensionEvent::new("fresh", "app.started", serde_json::json!({})).expect("event");
        bus.publish(&event)
            .expect("a fresh publisher reclaims the table");
        assert_eq!(bus.tracked_publishers(), 1);
    }

    #[test]
    fn lifecycle_events_carry_the_extension_and_state() {
        let id = ExtensionId::parse("com.example.a").expect("id");
        let event = lifecycle_event(&id, topics::EXTENSION_LOADED, "active", None).expect("event");
        assert_eq!(event.source, "bsh");
        assert_eq!(event.payload["extension"], "com.example.a");
        assert_eq!(event.payload["state"], "active");
        assert!(event.correlation_id.is_none());
        assert_eq!(
            event.correlated("turn-1").correlation_id.as_deref(),
            Some("turn-1")
        );
    }
}
