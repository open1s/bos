//! Shared resource limits for bounded workspace scans.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

/// Hard limits shared by every worker participating in one scan.
#[derive(Clone, Debug)]
pub struct ScanLimits {
    /// Maximum directory entries inspected across the entire scan.
    pub max_entries: u64,
    /// Maximum regular files accepted across the entire scan.
    pub max_files: u64,
    /// Maximum aggregate file bytes reserved across the entire scan.
    pub max_total_bytes: u64,
    /// Maximum size of any one file.
    pub max_file_bytes: u64,
    /// Maximum relative directory depth. The workspace root has depth zero.
    pub max_depth: usize,
    /// Maximum wall-clock duration of the scan.
    pub max_duration: Duration,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_files: 20_000,
            max_total_bytes: 512 * 1024 * 1024,
            max_file_bytes: 4 * 1024 * 1024,
            max_depth: 64,
            max_duration: Duration::from_secs(120),
        }
    }
}

/// Reason a scan operation was denied by its shared budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ScanBudgetError {
    /// The scan was explicitly cancelled.
    #[error("scan cancelled")]
    Cancelled,
    /// The scan exceeded its wall-clock deadline.
    #[error("scan deadline exceeded")]
    DeadlineExceeded,
    /// The aggregate directory-entry limit was reached.
    #[error("scan entry limit exceeded")]
    EntryLimitExceeded,
    /// The aggregate accepted-file limit was reached.
    #[error("scan file limit exceeded")]
    FileLimitExceeded,
    /// The aggregate byte limit would be exceeded.
    #[error("scan total byte limit exceeded")]
    TotalBytesLimitExceeded,
    /// A file exceeds the configured per-file size limit.
    #[error("file size limit exceeded")]
    FileTooLarge,
    /// A directory is deeper than the configured maximum.
    #[error("scan depth limit exceeded")]
    DepthLimitExceeded,
}

/// A consistent snapshot of resource consumption for one scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanUsage {
    /// Directory entries inspected.
    pub entries: u64,
    /// Regular files accepted.
    pub files: u64,
    /// File bytes reserved for reading.
    pub total_bytes: u64,
}

/// Thread-safe budget shared by all workers in one workspace scan.
///
/// Entry and file/byte reservations are serialized together so concurrent
/// workers cannot independently pass a limit and collectively exceed it.
/// A reservation is intentionally not refunded: this keeps accounting
/// conservative if a file changes or a worker is cancelled during reading.
#[derive(Clone, Debug)]
pub struct ScanBudget {
    limits: Arc<ScanLimits>,
    started: Instant,
    cancelled: Arc<AtomicBool>,
    usage: Arc<Mutex<ScanUsage>>,
}

impl ScanBudget {
    /// Create a new scan budget. Clones share cancellation and accounting.
    pub fn new(limits: ScanLimits) -> Self {
        Self {
            limits: Arc::new(limits),
            started: Instant::now(),
            cancelled: Arc::new(AtomicBool::new(false)),
            usage: Arc::new(Mutex::new(ScanUsage::default())),
        }
    }

    /// Signal cancellation to this budget and all of its clones.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Check cancellation and wall-clock deadline without consuming budget.
    pub fn check(&self) -> Result<(), ScanBudgetError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(ScanBudgetError::Cancelled);
        }
        if self.started.elapsed() >= self.limits.max_duration {
            return Err(ScanBudgetError::DeadlineExceeded);
        }
        Ok(())
    }

    /// Record one inspected directory entry at the supplied root-relative depth.
    ///
    /// Every entry counts, including ignored symlinks and special files.
    pub fn record_entry(&self, depth: usize) -> Result<(), ScanBudgetError> {
        self.check()?;
        if depth > self.limits.max_depth {
            return Err(ScanBudgetError::DepthLimitExceeded);
        }

        let mut usage = self
            .usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.check()?;
        let next = usage.entries.saturating_add(1);
        if next > self.limits.max_entries {
            return Err(ScanBudgetError::EntryLimitExceeded);
        }
        usage.entries = next;
        Ok(())
    }

    /// Record one accepted regular file without reserving its content bytes.
    ///
    /// This is used by inventory-only scans that do not read source contents.
    pub fn record_file(&self) -> Result<(), ScanBudgetError> {
        self.check()?;
        let mut usage = self
            .usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.check()?;
        let next_files = usage.files.saturating_add(1);
        if next_files > self.limits.max_files {
            return Err(ScanBudgetError::FileLimitExceeded);
        }
        usage.files = next_files;
        Ok(())
    }

    /// Reserve content bytes for a file already accepted by the scan.
    ///
    /// The caller must cap the actual read to the configured per-file limit
    /// and remaining aggregate byte allowance. Reservations are conservative
    /// and are not refunded when a read fails or a file changes.
    pub fn reserve_file_bytes(&self, size_bytes: u64) -> Result<(), ScanBudgetError> {
        self.check()?;
        if size_bytes > self.limits.max_file_bytes {
            return Err(ScanBudgetError::FileTooLarge);
        }
        let mut usage = self
            .usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.check()?;
        let next_bytes = usage.total_bytes.saturating_add(size_bytes);
        if next_bytes > self.limits.max_total_bytes {
            return Err(ScanBudgetError::TotalBytesLimitExceeded);
        }
        usage.total_bytes = next_bytes;
        Ok(())
    }

    /// Reserve one regular file and its known size before reading it.
    ///
    /// This convenience method atomically records the file and reserves its
    /// bytes. Use record_file plus reserve_file_bytes when inventory and content
    /// retrieval are separate stages.
    pub fn reserve_file(&self, size_bytes: u64) -> Result<(), ScanBudgetError> {
        self.check()?;
        if size_bytes > self.limits.max_file_bytes {
            return Err(ScanBudgetError::FileTooLarge);
        }

        let mut usage = self
            .usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.check()?;
        let next_files = usage.files.saturating_add(1);
        if next_files > self.limits.max_files {
            return Err(ScanBudgetError::FileLimitExceeded);
        }
        let next_bytes = usage.total_bytes.saturating_add(size_bytes);
        if next_bytes > self.limits.max_total_bytes {
            return Err(ScanBudgetError::TotalBytesLimitExceeded);
        }
        usage.files = next_files;
        usage.total_bytes = next_bytes;
        Ok(())
    }

    /// Return a consistent snapshot of aggregate usage.
    pub fn usage(&self) -> ScanUsage {
        *self
            .usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Return the configured limits.
    pub fn limits(&self) -> &ScanLimits {
        &self.limits
    }

    /// Return the remaining aggregate directory-entry allowance.
    pub fn remaining_entries(&self) -> u64 {
        self.limits.max_entries.saturating_sub(self.usage().entries)
    }

    /// Return the remaining aggregate byte allowance.
    pub fn remaining_bytes(&self) -> u64 {
        self.limits
            .max_total_bytes
            .saturating_sub(self.usage().total_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn limits() -> ScanLimits {
        ScanLimits {
            max_entries: 3,
            max_files: 2,
            max_total_bytes: 10,
            max_file_bytes: 8,
            max_depth: 2,
            max_duration: Duration::from_secs(10),
        }
    }

    #[test]
    fn enforces_aggregate_entry_and_depth_limits() {
        let budget = ScanBudget::new(limits());
        budget.record_entry(0).unwrap();
        budget.record_entry(2).unwrap();
        budget.record_entry(1).unwrap();
        assert_eq!(
            budget.record_entry(1),
            Err(ScanBudgetError::EntryLimitExceeded)
        );
        assert_eq!(
            budget.record_entry(3),
            Err(ScanBudgetError::DepthLimitExceeded)
        );
        assert_eq!(budget.usage().entries, 3);
    }

    #[test]
    fn reserves_file_count_and_aggregate_bytes_atomically() {
        let budget = ScanBudget::new(limits());
        budget.reserve_file(6).unwrap();
        assert_eq!(budget.remaining_bytes(), 4);
        assert_eq!(
            budget.reserve_file(5),
            Err(ScanBudgetError::TotalBytesLimitExceeded)
        );
        budget.reserve_file(4).unwrap();
        assert_eq!(
            budget.reserve_file(0),
            Err(ScanBudgetError::FileLimitExceeded)
        );
        assert_eq!(
            budget.usage(),
            ScanUsage {
                entries: 0,
                files: 2,
                total_bytes: 10,
            }
        );
    }

    #[test]
    fn rejects_oversized_files_before_reservation() {
        let budget = ScanBudget::new(limits());
        assert_eq!(budget.reserve_file(9), Err(ScanBudgetError::FileTooLarge));
        assert_eq!(budget.usage(), ScanUsage::default());
    }

    #[test]
    fn cancellation_is_shared_by_clones() {
        let budget = ScanBudget::new(limits());
        let worker_budget = budget.clone();
        budget.cancel();
        assert_eq!(worker_budget.check(), Err(ScanBudgetError::Cancelled));
        assert_eq!(
            worker_budget.record_entry(0),
            Err(ScanBudgetError::Cancelled)
        );
    }

    #[test]
    fn deadline_is_enforced() {
        let budget = ScanBudget::new(ScanLimits {
            max_duration: Duration::ZERO,
            ..limits()
        });
        assert_eq!(budget.check(), Err(ScanBudgetError::DeadlineExceeded));
    }

    #[test]
    fn concurrent_workers_cannot_exceed_entry_limit() {
        let budget = ScanBudget::new(limits());
        let workers = (0..8)
            .map(|_| {
                let budget = budget.clone();
                thread::spawn(move || budget.record_entry(0).is_ok())
            })
            .collect::<Vec<_>>();
        let accepted = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker join"))
            .filter(|accepted| *accepted)
            .count();
        assert_eq!(accepted, 3);
        assert_eq!(budget.usage().entries, 3);
    }

    #[test]
    fn tracks_remaining_entries_and_inventory_files_separately_from_bytes() {
        let budget = ScanBudget::new(limits());
        assert_eq!(budget.remaining_entries(), 3);
        budget.record_entry(0).unwrap();
        assert_eq!(budget.remaining_entries(), 2);

        budget.record_file().unwrap();
        assert_eq!(budget.usage().files, 1);
        assert_eq!(budget.usage().total_bytes, 0);

        budget.reserve_file_bytes(6).unwrap();
        assert_eq!(budget.remaining_bytes(), 4);
        assert_eq!(budget.usage().files, 1);
    }
}
