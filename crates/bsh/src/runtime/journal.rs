//! Durable Runtime Event Journal (RFC-013).
//!
//! The journal is the source of replayable committed facts. The in-process
//! dispatch to subscribers is best effort: a subscriber that falls behind can
//! always recover committed events by replaying from its last cursor.
//!
//! Every append is written and synced before it becomes visible to memory or
//! to a subscriber, so "published" never means "maybe on disk".

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::runtime::contract::{EventEnvelope, EventPayload, PROTOCOL_VERSION};
use crate::runtime::ids::{EventId, ExecutionId, SessionId, TaskId, WorkspaceId};

/// Default number of events retained for cursor replay. Older events remain on
/// disk but are no longer replayable without a snapshot.
pub const DEFAULT_REPLAY_WINDOW: u64 = 4096;

/// Default per-subscription buffer. Overflow closes the subscription instead of
/// dropping a durable event silently.
pub const DEFAULT_SUBSCRIPTION_BUFFER: usize = 256;

/// Journal failures.
#[derive(Debug, thiserror::Error)]
pub(crate) enum JournalError {
    /// The journal file cannot be read or written.
    #[error("journal {path}: {source}")]
    Io {
        /// Path involved.
        path: String,
        /// OS error.
        #[source]
        source: std::io::Error,
    },
    /// A line is not a valid event envelope.
    #[error("journal {path} line {line}: {detail}")]
    Corrupt {
        /// Path involved.
        path: String,
        /// 1-based line.
        line: usize,
        /// Parse failure.
        detail: String,
    },
    /// Sequence order is broken.
    #[error("journal {path} line {line}: sequence {found} does not follow {expected}")]
    Sequence {
        /// Path involved.
        path: String,
        /// 1-based line.
        line: usize,
        /// Expected sequence.
        expected: u64,
        /// Found sequence.
        found: u64,
    },
}

impl JournalError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// Internal event draft: the Host supplies facts, the journal supplies envelope
/// identity, ordering and timestamps.
#[derive(Debug, Clone)]
pub(crate) struct EventDraft {
    pub payload: EventPayload,
    pub session_id: Option<SessionId>,
    pub execution_id: Option<ExecutionId>,
    pub task_id: Option<TaskId>,
    pub causation_id: Option<String>,
    pub correlation_id: Option<String>,
}

impl EventDraft {
    pub fn new(payload: EventPayload) -> Self {
        Self {
            payload,
            session_id: None,
            execution_id: None,
            task_id: None,
            causation_id: None,
            correlation_id: None,
        }
    }

    pub fn session(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn execution(mut self, execution_id: ExecutionId) -> Self {
        self.execution_id = Some(execution_id);
        self
    }

    pub fn caused_by(mut self, causation_id: impl ToString) -> Self {
        self.causation_id = Some(causation_id.to_string());
        self
    }

    pub fn correlated_with(mut self, correlation_id: impl ToString) -> Self {
        self.correlation_id = Some(correlation_id.to_string());
        self
    }
}

/// One live subscription: the replay slice plus a bounded live queue.
pub(crate) struct Subscription {
    pub id: u64,
    pub replay: Vec<EventEnvelope>,
    pub next_cursor: u64,
    pub receiver: mpsc::Receiver<EventEnvelope>,
}

struct Inner {
    file: File,
    events: Vec<EventEnvelope>,
    head: u64,
    next_subscription: u64,
    subscribers: HashMap<u64, mpsc::Sender<EventEnvelope>>,
}

/// Durable, append-only, workspace-scoped event journal.
pub(crate) struct Journal {
    path: std::path::PathBuf,
    workspace_id: WorkspaceId,
    stream_id: String,
    replay_window: u64,
    subscription_buffer: usize,
    inner: std::sync::Mutex<Inner>,
}

impl Journal {
    /// Open or create the journal, loading every persisted event.
    pub fn open(
        path: impl AsRef<Path>,
        workspace_id: WorkspaceId,
        replay_window: u64,
        subscription_buffer: usize,
    ) -> Result<Self, JournalError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| JournalError::io(&path, error))?;
            private_dir(parent).map_err(|error| JournalError::io(&path, error))?;
        }
        let file = private_append_file(&path).map_err(|error| JournalError::io(&path, error))?;
        let events = read_events(&path)?;
        let mut head = 0;
        for (index, event) in events.iter().enumerate() {
            let expected = head + 1;
            if event.sequence != expected {
                return Err(JournalError::Sequence {
                    path: path.display().to_string(),
                    line: index + 1,
                    expected,
                    found: event.sequence,
                });
            }
            head = event.sequence;
        }
        Ok(Self {
            path,
            workspace_id,
            stream_id: format!("workspace:{workspace_id}"),
            replay_window,
            subscription_buffer: subscription_buffer.max(1),
            inner: std::sync::Mutex::new(Inner {
                file,
                events,
                head,
                next_subscription: 1,
                subscribers: HashMap::new(),
            }),
        })
    }

    /// The declared stream every event belongs to.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Current head sequence.
    pub fn head(&self) -> u64 {
        self.inner.lock().expect("journal lock").head
    }

    /// Every loaded event, including history older than the replay window.
    pub fn all_events(&self) -> Vec<EventEnvelope> {
        self.inner.lock().expect("journal lock").events.clone()
    }

    /// Drop replay history older than the configured window. Call after the
    /// projection has folded the full history at startup.
    pub fn trim_to_window(&self) {
        let mut inner = self.inner.lock().expect("journal lock");
        trim(&mut inner, self.replay_window);
    }

    /// Append one fact. Returns only after the bytes are synced.
    ///
    /// Publication is a separate step ([`Journal::publish`]) so the Host can
    /// fold the event into its projections first: a client that reacts to an
    /// event by querying must never observe a projection that lags the fact it
    /// just received.
    pub fn append(&self, draft: EventDraft) -> Result<EventEnvelope, JournalError> {
        let mut inner = self.inner.lock().expect("journal lock");
        let sequence = inner.head + 1;
        let envelope = EventEnvelope {
            event_id: EventId::new(),
            workspace_id: self.workspace_id,
            stream_id: self.stream_id.clone(),
            sequence,
            session_id: draft.session_id,
            execution_id: draft.execution_id,
            task_id: draft.task_id,
            causation_id: draft.causation_id,
            correlation_id: draft.correlation_id,
            occurred_at: Utc::now(),
            schema_version: PROTOCOL_VERSION,
            payload: draft.payload,
        };
        let line = serde_json::to_vec(&envelope).map_err(|error| JournalError::Corrupt {
            path: self.path.display().to_string(),
            line: (sequence as usize).saturating_add(1),
            detail: format!("event cannot be serialized: {error}"),
        })?;
        inner
            .file
            .write_all(&line)
            .and_then(|()| inner.file.write_all(b"\n"))
            .and_then(|()| inner.file.sync_data())
            .map_err(|error| JournalError::io(&self.path, error))?;
        inner.events.push(envelope.clone());
        inner.head = sequence;
        trim(&mut inner, self.replay_window);
        Ok(envelope)
    }

    /// Dispatch one already-committed event to live subscribers.
    ///
    /// A slow subscriber is disconnected rather than allowed to make the Host
    /// drop a committed event; it can recover everything by replaying from its
    /// last cursor.
    pub fn publish(&self, envelope: &EventEnvelope) {
        let mut inner = self.inner.lock().expect("journal lock");
        inner
            .subscribers
            .retain(|_, sender| sender.try_send(envelope.clone()).is_ok());
    }

    /// Register a subscriber with a gap-free replay-to-live handoff.
    ///
    /// The replay slice and the live sender are installed under the same lock
    /// that appends take, so no event can fall between them.
    pub fn subscribe(&self, cursor: Option<u64>) -> Result<Subscription, SubscriptionError> {
        let mut inner = self.inner.lock().expect("journal lock");
        let head = inner.head;
        let floor = inner
            .events
            .first()
            .map(|event| event.sequence)
            .unwrap_or(head + 1);
        let (replay, next_cursor) = match cursor {
            None => (Vec::new(), head),
            Some(position) => {
                if position > head {
                    return Err(SubscriptionError::Invalid {
                        requested: position,
                        head,
                    });
                }
                if position + 1 < floor && position != head {
                    return Err(SubscriptionError::Expired {
                        requested: position,
                        retained_from: floor,
                        head,
                    });
                }
                let replay = inner
                    .events
                    .iter()
                    .filter(|event| event.sequence > position)
                    .cloned()
                    .collect();
                (replay, head)
            }
        };
        let id = inner.next_subscription;
        inner.next_subscription += 1;
        let (sender, receiver) = mpsc::channel(self.subscription_buffer);
        inner.subscribers.insert(id, sender);
        Ok(Subscription {
            id,
            replay,
            next_cursor,
            receiver,
        })
    }

    /// Remove one subscription.
    pub fn unsubscribe(&self, id: u64) {
        self.inner
            .lock()
            .expect("journal lock")
            .subscribers
            .remove(&id);
    }

    /// Drop all subscribers (used while draining).
    pub fn close_subscribers(&self) {
        self.inner.lock().expect("journal lock").subscribers.clear();
    }

    /// Number of live subscribers, for tests and diagnostics.
    #[cfg(test)]
    pub fn subscriber_count(&self) -> usize {
        self.inner.lock().expect("journal lock").subscribers.len()
    }
}

/// Why a subscription could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SubscriptionError {
    /// The cursor is inside the retained window.
    #[error("cursor {requested} expired; retained from {retained_from}, head {head}")]
    Expired {
        /// Requested position.
        requested: u64,
        /// First retained event.
        retained_from: u64,
        /// Current head.
        head: u64,
    },
    /// The cursor names an event the Host has not committed.
    #[error("cursor {requested} is ahead of head {head}")]
    Invalid {
        /// Requested position.
        requested: u64,
        /// Current head.
        head: u64,
    },
}

fn trim(inner: &mut Inner, replay_window: u64) {
    if replay_window == 0 || inner.events.len() <= replay_window as usize {
        return;
    }
    let excess = inner.events.len() - replay_window as usize;
    inner.events.drain(0..excess);
}

fn read_events(path: &Path) -> Result<Vec<EventEnvelope>, JournalError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(JournalError::io(path, error)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|error| JournalError::Corrupt {
        path: path.display().to_string(),
        line: 1,
        detail: format!("journal is not UTF-8: {error}"),
    })?;
    let chunks: Vec<&str> = text.split_inclusive('\n').collect();
    let mut events = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let line = chunk.trim_end_matches('\n');
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<EventEnvelope>(line) {
            Ok(event) => events.push(event),
            // A crash between the event bytes and the trailing newline leaves
            // an unterminated last line. That event was never acknowledged to
            // its writer, so dropping it is the honest recovery; the tail is
            // truncated so the next append starts from a clean line.
            Err(error) if index + 1 == chunks.len() && !chunk.ends_with('\n') => {
                log::warn!(
                    "journal {}: dropping torn trailing line: {error}",
                    path.display()
                );
                truncate_torn_tail(path, text.len() - line.len())?;
                break;
            }
            Err(error) => {
                return Err(JournalError::Corrupt {
                    path: path.display().to_string(),
                    line: index + 1,
                    detail: error.to_string(),
                });
            }
        }
    }
    Ok(events)
}

/// Drop everything from `keep` onward, keeping the committed prefix byte-exact.
fn truncate_torn_tail(path: &Path, keep: usize) -> Result<(), JournalError> {
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| JournalError::io(path, error))?;
    file.set_len(keep as u64)
        .and_then(|()| file.sync_data())
        .map_err(|error| JournalError::io(path, error))
}

fn private_append_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    hostkit::fspriv::private_permissions(path)?;
    Ok(file)
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::runtime::contract::EventPayload;
    use crate::runtime::ids::{CommandId, PayloadHash, SessionId, WorkspaceId};

    use super::*;

    fn created(title: &str) -> (SessionId, EventDraft) {
        let session = SessionId::new();
        let draft = EventDraft::new(EventPayload::SessionCreated {
            title: title.to_string(),
            config: crate::runtime::contract::SessionConfig::default(),
            forked_from: None,
            command_id: CommandId::new(),
            payload_hash: PayloadHash::of(&title),
        })
        .session(session);
        (session, draft)
    }

    fn append(journal: &Journal, draft: EventDraft) -> EventEnvelope {
        let event = journal.append(draft).unwrap();
        journal.publish(&event);
        event
    }

    #[tokio::test]
    async fn append_persists_and_reopens_with_monotonic_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        let workspace = WorkspaceId::new();
        {
            let journal = Journal::open(&path, workspace, 100, 8).unwrap();
            let first = append(&journal, created("one").1);
            let second = append(&journal, created("two").1);
            assert_eq!(first.sequence, 1);
            assert_eq!(second.sequence, 2);
            assert_eq!(journal.head(), 2);
        }
        let reopened = Journal::open(&path, workspace, 100, 8).unwrap();
        assert_eq!(reopened.head(), 2);
        assert_eq!(reopened.all_events().len(), 2);
    }

    #[tokio::test]
    async fn replay_then_live_does_not_gap_or_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(dir.path().join("journal.jsonl"), WorkspaceId::new(), 100, 8).unwrap();
        append(&journal, created("one").1);
        append(&journal, created("two").1);

        let mut subscription = journal.subscribe(Some(1)).unwrap();
        assert_eq!(subscription.replay.len(), 1);
        assert_eq!(subscription.replay[0].sequence, 2);
        assert_eq!(subscription.next_cursor, 2);

        let live = append(&journal, created("three").1);
        let received = subscription.receiver.recv().await.unwrap();
        assert_eq!(received.event_id, live.event_id);
        assert_eq!(received.sequence, 3);
    }

    #[tokio::test]
    async fn cursor_before_window_is_expired_and_head_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(dir.path().join("journal.jsonl"), WorkspaceId::new(), 2, 8).unwrap();
        for index in 0..5 {
            append(&journal, created(&format!("title-{index}")).1);
        }
        assert_eq!(journal.head(), 5);
        assert!(matches!(
            journal.subscribe(Some(0)),
            Err(SubscriptionError::Expired {
                retained_from: 4,
                ..
            })
        ));
        let at_head = journal.subscribe(Some(5)).unwrap();
        assert!(at_head.replay.is_empty());
        assert!(matches!(
            journal.subscribe(Some(9)),
            Err(SubscriptionError::Invalid { .. })
        ));
    }

    #[tokio::test]
    async fn slow_subscriber_is_disconnected_without_dropping_events_silently() {
        let dir = tempfile::tempdir().unwrap();
        let journal =
            Journal::open(dir.path().join("journal.jsonl"), WorkspaceId::new(), 100, 1).unwrap();
        let subscription = journal.subscribe(None).unwrap();
        // Buffer capacity is one; the first event fills it, the second closes
        // the subscription instead of dropping an event silently.
        append(&journal, created("one").1);
        append(&journal, created("two").1);
        assert_eq!(journal.subscriber_count(), 0);
        assert_eq!(journal.head(), 2);
        assert_eq!(subscription.next_cursor, 0);
    }

    #[test]
    fn a_torn_trailing_line_is_dropped_and_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        let workspace = WorkspaceId::new();
        {
            let journal = Journal::open(&path, workspace, 100, 8).unwrap();
            append(&journal, created("one").1);
        }
        // Simulate a crash between the event bytes and the newline.
        let committed = std::fs::read_to_string(&path).unwrap();
        let torn = committed.trim_end_matches('\n').to_string() + "\n{\"event_id\":";
        std::fs::write(&path, &torn).unwrap();
        let reopened = Journal::open(&path, workspace, 100, 8).unwrap();
        assert_eq!(reopened.head(), 1);
        // The committed prefix is kept byte-exact, the torn tail is gone.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), committed);
    }

    #[test]
    fn a_corrupt_committed_line_is_an_error_not_a_silent_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        std::fs::write(&path, "{\"not\":5}\n").unwrap();
        assert!(matches!(
            Journal::open(&path, WorkspaceId::new(), 100, 8),
            Err(JournalError::Corrupt { .. })
        ));
    }
}
