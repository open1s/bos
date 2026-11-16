//! Advisory (`flock`-style) lock state shared by storage handlers.
//!
//! Semantics: a resource holds at most one lock *mode* at a time. Shared locks
//! can coexist; an exclusive lock excludes everything. Locking when already
//! locked (by anyone) returns [`ResourceError::Locked`]. Unlocking releases
//! the resource entirely.
//!
//! This is process-local state: locks don't persist, don't survive a restart,
//! and don't coordinate across separate node processes — same as `flock` on
//! a local mount.

use crate::error::{ResourceError, Result};

/// Lifecycle of a single advisory lock.
///
/// Store one per handler instance; the manager's lock serializes updates, so
/// no interior mutability is needed here.
#[derive(Debug, Default)]
pub struct LockState {
    /// Number of active shared holders.
    shared: u32,
    /// Whether an exclusive (write) lock is held.
    exclusive: bool,
}

impl LockState {
    /// Try to acquire. Returns [`ResourceError::Locked`] on conflict.
    pub fn lock(&mut self, exclusive: bool) -> Result<()> {
        if exclusive && (self.shared > 0 || self.exclusive) {
            return Err(ResourceError::Locked("exclusive lock busy".into()));
        }
        if !exclusive && self.exclusive {
            return Err(ResourceError::Locked(
                "shared lock blocked by exclusive".into(),
            ));
        }
        if exclusive {
            self.exclusive = true;
        } else {
            self.shared += 1;
        }
        Ok(())
    }

    /// Release the most recently taken lock. Unconditional: unlock always
    /// succeeds (idempotent, matching flock semantics on close).
    pub fn unlock(&mut self) {
        if self.exclusive {
            self.exclusive = false;
        } else if self.shared > 0 {
            self.shared -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_blocks_all() {
        let mut l = LockState::default();
        l.lock(true).unwrap();
        assert!(l.lock(false).is_err());
        assert!(l.lock(true).is_err());
        l.unlock();
        l.lock(false).unwrap();
        l.lock(false).unwrap();
        assert!(l.lock(true).is_err());
    }
}
