//! The Browser pane's rendered engine: an external Obscura process over CDP.
//!
//! # Why an external process
//!
//! The engine is a binary, not a linked crate: nothing here compiles a browser,
//! nothing here adds a dependency to the default build, and the release tarball
//! is not vendored (see `docs/bsh-browser-substrate.md` for the install
//! recipe). The pane talks to it over the standard CDP endpoint it already
//! exposes, so the protocol is one an external tool could drive too.
//!
//! # The module map
//!
//! - `cdp::client` launches the process, learns its port, opens the one
//!   websocket and owns the single reader/writer thread.
//! - `cdp::wire` is JSON-RPC 2.0 plus the RFC 6455 socket.
//! - `cdp` is the high-level page vocabulary the Browser model consumes.
//! - `gate` is the egress gate: the loopback proxy the process is launched
//!   with, which asks the SAME `EgressGate` the tools ask before any origin
//!   socket exists.
//!
//! # Frames
//!
//! `FrameSlot` is the one-way mailbox from the engine's capture pump to the
//! pane. The pump decodes a JPEG into RGBA on its own thread, so the GPUI
//! thread only ever builds a `RenderImage` from bytes that already exist -
//! never a decode of a multi-megabyte PNG on the render path.

pub(crate) mod cdp;
pub(crate) mod gate;
pub(crate) mod reap;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) use cdp::{Engine, EngineError, Navigation, Page};
pub(crate) use gate::Gate;

/// One decoded frame, addressed by a sequence number.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    /// Pixel width.
    pub(crate) width: u32,
    /// Pixel height.
    pub(crate) height: u32,
    /// RGBA8, row-major.
    pub(crate) rgba: Arc<Vec<u8>>,
}

/// The newest frame, and the sequence that says whether it is new.
#[derive(Debug, Default)]
pub(crate) struct FrameSlot {
    seq: AtomicU64,
    frame: Mutex<Option<Frame>>,
}

impl FrameSlot {
    /// Publish a frame.
    pub(crate) fn store(&self, rgba: Arc<Vec<u8>>, width: u32, height: u32) {
        self.seq.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut slot) = self.frame.lock() {
            slot.replace(Frame {
                width,
                height,
                rgba,
            });
        }
    }

    /// The newest frame, with its sequence.
    pub(crate) fn latest(&self) -> Option<Frame> {
        self.frame.lock().ok().and_then(|slot| slot.clone())
    }

    /// The newest sequence number, zero before the first capture.
    pub(crate) fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests;
