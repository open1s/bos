//! Durable retry state for compensation that did not settle remotely.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One compensation that still needs a remote retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingCompensation {
    /// The task that failed.
    pub task_id: String,
    /// The delegation created for that task.
    pub delegation_id: String,
    /// The Node that owns the side effect.
    pub target: String,
    /// How many reconciliation attempts have run.
    pub attempts: u32,
    /// The last refusal or transport error.
    pub last_error: Option<String>,
    /// Last update, in Unix milliseconds.
    pub updated_at_ms: u64,
    /// Earliest time this entry may be retried.
    #[serde(default)]
    pub next_attempt_ms: u64,
}

/// A small durable queue keyed by delegation id.
#[derive(Debug, Clone)]
pub struct CompensationQueue {
    path: PathBuf,
    entries: Vec<PendingCompensation>,
}

impl CompensationQueue {
    /// Load the queue, treating a missing file as empty.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<PendingCompensation>>(&text).ok())
            .unwrap_or_default();
        Self { path, entries }
    }

    /// The live entries.
    pub fn entries(&self) -> &[PendingCompensation] {
        &self.entries
    }

    /// Insert or refresh one entry.
    pub fn enqueue(&mut self, entry: PendingCompensation) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|item| item.delegation_id == entry.delegation_id)
        {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Drop one settled delegation.
    pub fn remove(&mut self, delegation_id: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| entry.delegation_id != delegation_id);
        self.entries.len() != before
    }

    /// Entries whose backoff window has elapsed.
    pub fn due_entries(&self, now_ms: u64) -> Vec<PendingCompensation> {
        self.entries
            .iter()
            .filter(|entry| entry.next_attempt_ms <= now_ms)
            .cloned()
            .collect()
    }

    /// Record a failed retry without losing the entry.
    pub fn record_failure(&mut self, delegation_id: &str, error: String, now_ms: u64) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.delegation_id == delegation_id)
        {
            entry.attempts = entry.attempts.saturating_add(1);
            entry.last_error = Some(error);
            entry.updated_at_ms = now_ms;
            entry.next_attempt_ms = now_ms.saturating_add(backoff_ms(entry.attempts));
        }
    }

    /// Write the queue atomically with private permissions.
    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text =
            serde_json::to_string_pretty(&self.entries).map_err(|error| error.to_string())?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temporary, &self.path).map_err(|error| error.to_string())
    }

    /// The file this queue persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn backoff_ms(attempts: u32) -> u64 {
    let exponent = attempts.min(6);
    (5_000u64.saturating_mul(1u64 << exponent)).min(300_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_survives_a_round_trip_and_removes_settled_entries() {
        let path = std::env::temp_dir().join(format!("bos-comp-{}.json", std::process::id()));
        let mut queue = CompensationQueue::load(&path);
        queue.enqueue(PendingCompensation {
            task_id: "t1".to_string(),
            delegation_id: "d1".to_string(),
            target: "node".to_string(),
            attempts: 0,
            last_error: Some("offline".to_string()),
            updated_at_ms: 1,
            next_attempt_ms: 0,
        });
        queue.save().expect("saved");
        let mut loaded = CompensationQueue::load(&path);
        assert_eq!(loaded.entries().len(), 1);
        loaded.record_failure("d1", "still offline".to_string(), 2);
        assert_eq!(loaded.entries()[0].attempts, 1);
        assert!(loaded.due_entries(5_000).is_empty());
        assert_eq!(loaded.due_entries(10_002).len(), 1);
        assert!(loaded.remove("d1"));
        loaded.save().expect("resaved");
        assert!(CompensationQueue::load(&path).entries().is_empty());
        let _ = std::fs::remove_file(path);
    }
}
