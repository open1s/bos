//! The queue between the model's schedule tools and the shell that owns them.
//!
//! # Why a queue
//!
//! A scheduled task is settings state: the Automation tasks pane edits it, the
//! shell clock fires it, and both read the one `Settings` document the app
//! keeps. An agent tool cannot hold that document - it is built from a snapshot
//! for one turn - so a tool that wrote a task itself would either fork a second
//! source of truth or race the pane's own save.
//!
//! The tool therefore ASKS: it validates the request, pushes it here and tells
//! the model when it lands. The shell drains this queue at the turn's end and
//! on its clock, applies each request to `GlobalSettings`, and saves once. The
//! reader's pane, the scheduler and the model's own next `schedule_list` all
//! then read the same list, because there is still only one.
//!
//! A request is applied at most once: `drain` takes the vector. A request the
//! settings refuse (a duplicate id, a name that is too long) is logged and
//! dropped; the model already validated the same fields, so a refusal here is
//! a state change between the ask and the apply, not a user error.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::settings::global::unix_now;

/// One change the model asked the shell to make to the task list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleRequest {
    /// Add a task. The id is minted by the tool so its result can name it.
    Create {
        /// Stable id; the tool mints it and the settings accept it verbatim.
        id: String,
        /// Display name, bounded by the settings' own limit.
        name: String,
        /// The instruction the task sends to its session.
        prompt: String,
        /// The session the task runs in.
        session_id: String,
        /// Seconds between runs; zero means once.
        every_seconds: u64,
    },
    /// Change fields on an existing task; `None` leaves a field alone.
    Edit {
        /// The task to change.
        id: String,
        /// New display name.
        name: Option<String>,
        /// New instruction.
        prompt: Option<String>,
        /// New cadence.
        every_seconds: Option<u64>,
        /// New enabled state (false pauses, true resumes).
        enabled: Option<bool>,
    },
    /// Drop a task.
    Remove {
        /// The task to drop.
        id: String,
    },
}

impl ScheduleRequest {
    /// A one-line description for a log line.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Create { id, name, .. } => format!("create {id} ({name})"),
            Self::Edit { id, .. } => format!("edit {id}"),
            Self::Remove { id } => format!("remove {id}"),
        }
    }
}

/// The process-wide request queue.
#[derive(Default)]
pub(crate) struct ScheduleQueue {
    pending: Mutex<Vec<ScheduleRequest>>,
    /// Makes a minted id unique even when two calls land in one second.
    seq: AtomicU64,
}

/// The one queue every turn's tools and the shell share.
///
/// A process singleton, like the federation node: the app is one process with
/// one settings document, and a per-build queue would be a queue nobody drains.
static QUEUE: OnceLock<Arc<ScheduleQueue>> = OnceLock::new();

impl ScheduleQueue {
    /// The process-wide queue.
    pub(crate) fn global() -> &'static Arc<ScheduleQueue> {
        QUEUE.get_or_init(|| Arc::new(Self::default()))
    }

    /// Add one request; the shell applies it on its next drain.
    pub(crate) fn push(&self, request: ScheduleRequest) {
        match self.pending.lock() {
            Ok(mut pending) => pending.push(request),
            Err(poisoned) => poisoned.into_inner().push(request),
        }
    }

    /// Take every pending request, oldest first.
    pub(crate) fn drain(&self) -> Vec<ScheduleRequest> {
        match self.pending.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }

    /// Every pending request, without taking them.
    pub(crate) fn snapshot(&self) -> Vec<ScheduleRequest> {
        match self.pending.lock() {
            Ok(pending) => pending.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Mint a task id that no queued request has used.
    ///
    /// The timestamp keeps an id legible ("task-1762...") and the sequence keeps
    /// two tasks created in the same second distinct; the settings refuse a
    /// duplicate id rather than letting one task shadow another.
    pub(crate) fn next_id(&self) -> String {
        let unique = self.seq.fetch_add(1, Ordering::Relaxed);
        format!("task-{}-{unique}", unix_now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_drains_once_and_in_order() {
        let queue = ScheduleQueue::default();
        queue.push(ScheduleRequest::Remove {
            id: "a".to_string(),
        });
        queue.push(ScheduleRequest::Remove {
            id: "b".to_string(),
        });
        assert_eq!(queue.snapshot().len(), 2);
        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        assert!(queue.drain().is_empty(), "a drained queue is empty");
    }

    #[test]
    fn a_minted_id_is_never_shared() {
        let queue = ScheduleQueue::default();
        let first = queue.next_id();
        let second = queue.next_id();
        assert_ne!(first, second);
        assert!(first.starts_with("task-"));
    }
}
