//! Unified debug logging.
//!
//! Library code uses standard `log` crate macros (`debug!`, `info!`, etc.).
//! The CLI uses a simple `eprintln!`-based macro for reliable stderr output.

use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG: AtomicBool = AtomicBool::new(false);

/// Enable or disable debug output globally.
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}

/// Whether debug output is currently enabled.
pub fn is_debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Emit a `[debug] …` line to stderr when debug mode is on.
#[macro_export]
macro_rules! d {
    ($($arg:tt)*) => {
        if resource::debug::is_debug() {
            eprintln!("[debug] {}", format_args!($($arg)*));
        }
    };
}

#[cfg(feature = "cli")]
pub use logging::auto_init_tracing;

/// Initialize logging for the CLI. Should be called once at startup.
pub fn init_logging(debug: bool) {
    set_debug(debug);
    #[cfg(feature = "cli")]
    logging::auto_init_tracing();
}

/// Re-export standard log macros for library code.
pub use log::{debug, error, info, trace, warn};