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
pub(crate) const EVENT_SCHEMA_VERSION: &str = "1.0";

/// The largest serialized event this host accepts.
pub(crate) const MAX_EVENT_BYTES: usize = 256 * 1024;

/// The default per-subscriber queue depth.
pub(crate) const DEFAULT_QUEUE_DEPTH: usize = 64;

/// Topics the platform itself publishes. A package may subscribe to any
/// declared topic; these are the reserved ones its subscriptions are checked
/// against by documentation, not by a hard allowlist.
pub(crate) mod topics {
    /// The host finished starting.
    pub(crate) const APP_STARTED: &str = "app.started";
    /// The host is shutting down.
    pub(crate) const APP_SHUTDOWN: &str = "app.shutdown";
    /// A workspace opened.
    pub(crate) const WORKSPACE_OPENED: &str = "workspace.opened";
    /// A workspace closed.
    pub(crate) const WORKSPACE_CLOSED: &str = "workspace.closed";
    /// An agent turn started.
    pub(crate) const AGENT_STARTED: &str = "agent.started";
    /// An agent turn completed.
    pub(crate) const AGENT_COMPLETED: &str = "agent.completed";
    /// An agent turn failed.
    pub(crate) const AGENT_FAILED: &str = "agent.failed";
    /// A tool was invoked.
    pub(crate) const TOOL_INVOKED: &str = "tool.invoked";
    /// A tool completed.
    pub(crate) const TOOL_COMPLETED: &str = "tool.completed";
    /// A tool failed.
    pub(crate) const TOOL_FAILED: &str = "tool.failed";
    /// An extension became active.
    pub(crate) const EXTENSION_LOADED: &str = "extension.loaded";
    /// An extension stopped.
    pub(crate) const EXTENSION_UNLOADED: &str = "extension.unloaded";
    /// An extension failed.
    pub(crate) const EXTENSION_FAILED: &str = "extension.failed";
}

/// One event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ExtensionEvent {
    /// Unique event id.
    pub(crate) id: String,
    /// Publishing extension or "bsh" for the host.
    pub(crate) source: String,
    /// Dotted topic.
    pub(crate) event_type: String,
    /// Envelope schema generation.
    pub(crate) schema_version: String,
    /// Unix milliseconds.
    pub(crate) timestamp: u64,
    /// The request or turn this event belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) correlation_id: Option<String>,
    /// Event body.
    pub(crate) payload: Value,
}

impl ExtensionEvent {
    /// Build a host event.
    pub(crate) fn new(
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
    pub(crate) fn correlated(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    /// Check the topic and the payload ceiling.
    pub(crate) fn validate(&self) -> Result<(), ExtensionError> {
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
        Ok(())
    }

    /// Serialized size in bytes.
    pub(crate) fn size(&self) -> usize {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .unwrap_or(0)
    }
}

/// One dotted topic pattern, with an optional trailing wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopicPattern {
    segments: Vec<String>,
    wildcard: bool,
}

impl TopicPattern {
    /// Parse and validate a topic or pattern.
    pub(crate) fn parse(raw: &str, allow_wildcard: bool) -> Result<Self, String> {
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
    pub(crate) fn matches(&self, topic: &str) -> bool {
        if self.wildcard {
            let prefix = format!("{}.", self.segments.join("."));
            return topic.starts_with(&prefix);
        }
        topic == self.segments.join(".")
    }
}

/// A subscriber's topic filter.
#[derive(Debug, Clone, Default)]
pub(crate) struct EventFilter {
    patterns: Vec<TopicPattern>,
}

impl EventFilter {
    /// Select nothing.
    pub(crate) fn none() -> Self {
        Self::default()
    }

    /// Select everything.
    pub(crate) fn all() -> Self {
        Self {
            patterns: vec![TopicPattern {
                segments: Vec::new(),
                wildcard: true,
            }],
        }
    }

    /// Build from topic strings. A trailing `.*` selects a namespace.
    pub(crate) fn from_topics(topics: &[String]) -> Result<Self, String> {
        let mut patterns = Vec::with_capacity(topics.len());
        for topic in topics {
            patterns.push(TopicPattern::parse(topic, true)?);
        }
        Ok(Self { patterns })
    }

    /// Whether the filter selects `topic`.
    pub(crate) fn matches(&self, topic: &str) -> bool {
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
    pub(crate) fn topics(&self) -> Vec<String> {
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
pub(crate) struct BusMetrics {
    /// Events accepted for publication.
    pub(crate) published: u64,
    /// Deliveries accepted by a subscriber queue.
    pub(crate) delivered: u64,
    /// Deliveries dropped because a queue was full.
    pub(crate) dropped: u64,
    /// Events refused before publication.
    pub(crate) rejected: u64,
}

/// What one publish did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PublishReport {
    /// Subscriber queues that accepted the event.
    pub(crate) delivered: usize,
    /// Subscriber queues that were full.
    pub(crate) dropped: usize,
}

struct Subscriber {
    filter: EventFilter,
    sender: async_channel::Sender<ExtensionEvent>,
}

/// The event bus. Cloneable by Arc, never by value.
pub(crate) struct EventBus {
    subscribers: Mutex<BTreeMap<u64, Subscriber>>,
    next_id: AtomicU64,
    metrics: Mutex<BusMetrics>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            subscribers: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            metrics: Mutex::new(BusMetrics::default()),
        }
    }
}

impl EventBus {
    /// A new bus with no subscribers.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Subscribe with a bounded queue.
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        filter: EventFilter,
        capacity: usize,
    ) -> Subscription {
        let capacity = capacity.clamp(1, 4096);
        let (sender, receiver) = async_channel::bounded(capacity);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.insert(id, Subscriber { filter, sender });
        }
        Subscription {
            id,
            receiver,
            bus: Arc::downgrade(self),
        }
    }

    /// Publish one event.
    pub(crate) fn publish(&self, event: &ExtensionEvent) -> Result<PublishReport, ExtensionError> {
        if let Err(error) = event.validate() {
            if let Ok(mut metrics) = self.metrics.lock() {
                metrics.rejected = metrics.rejected.saturating_add(1);
            }
            return Err(error);
        }
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
    pub(crate) fn metrics(&self) -> BusMetrics {
        self.metrics
            .lock()
            .map(|metrics| *metrics)
            .unwrap_or_default()
    }

    /// The number of live subscribers.
    pub(crate) fn subscriber_count(&self) -> usize {
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
pub(crate) struct Subscription {
    id: u64,
    receiver: async_channel::Receiver<ExtensionEvent>,
    bus: Weak<EventBus>,
}

impl Subscription {
    /// The subscription id.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// Wait for the next event.
    pub(crate) async fn recv(&self) -> Result<ExtensionEvent, async_channel::RecvError> {
        self.receiver.recv().await
    }

    /// Take the next event without waiting.
    pub(crate) fn try_recv(&self) -> Option<ExtensionEvent> {
        self.receiver.try_recv().ok()
    }

    /// The number of queued events.
    pub(crate) fn len(&self) -> usize {
        self.receiver.len()
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
pub(crate) fn lifecycle_event(
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
        let subscription = bus.subscribe(EventFilter::all(), 1);
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
            let _subscription = bus.subscribe(EventFilter::all(), 4);
            assert_eq!(bus.subscriber_count(), 1);
        }
        assert_eq!(bus.subscriber_count(), 0);
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
