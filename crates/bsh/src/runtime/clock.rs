//! Wall-clock helpers shared by the store, the runtime and both interfaces.
//!
//! A session document, a message timing and an archive stamp are all unix
//! timestamps, and every writer has to agree about which clock unit it means.
//! They are here, not in one interface's state, so the terminal and the window
//! cannot disagree about what "now" is.

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix seconds, the unit a session document and a message timing use.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Unix milliseconds, the unit a tool call's start time uses.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
