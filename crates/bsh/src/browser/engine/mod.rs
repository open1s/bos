//! The Browser pane's rendered engine: an external Chromium process over CDP.
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
//!
//! # Frames
//!
//! `FrameSlot` is the one-way mailbox from the engine's capture pump to the
//! pane. The pump probes the engine with a PNG capture, decodes it and swaps
//! the channels to BGRA on its own thread, so the GPUI thread only ever builds
//! a `RenderImage` from ready bytes - never a decode or a per-pixel pass on
//! the render path. A still page is probed without a decode at all, and the
//! cadence adapts: fast while the page changes, idle once it does not.

pub(crate) mod cdp;
pub(crate) mod reap;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) use cdp::{Engine, EngineError, Navigation, Page, Settle};

/// One decoded frame, addressed by a sequence number.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    /// Pixel width.
    pub(crate) width: u32,
    /// Pixel height.
    pub(crate) height: u32,
    /// BGRA8, row-major - the byte order GPUI's `RenderImage` draws. The
    /// channel swap from the capture's RGBA was paid on the pump thread, so
    /// the pane's frame build is a copy of ready bytes.
    pub(crate) bgra: Arc<Vec<u8>>,
}

/// The newest frame, and the sequence that says whether it is new.
#[derive(Debug, Default)]
pub(crate) struct FrameSlot {
    seq: AtomicU64,
    frame: Mutex<Option<Frame>>,
    /// Coalescing wake for the pane's clock. A published frame asks the UI to
    /// sync NOW instead of waiting for its next poll: the pump publishes about
    /// thirty frames a second while a page moves, and a 120 ms poll would show
    /// eight of them. The channel is bounded to one and the send is a try, so
    /// a reader slower than the pump collapses a burst into the one sync that
    /// reads the newest frame anyway.
    wake: Mutex<Option<async_channel::Sender<()>>>,
}

impl FrameSlot {
    /// Publish a frame.
    pub(crate) fn store(&self, bgra: Arc<Vec<u8>>, width: u32, height: u32) {
        if let Ok(mut slot) = self.frame.lock() {
            slot.replace(Frame {
                width,
                height,
                bgra,
            });
            // Publish the sequence only after the corresponding pixels are
            // readable. Keeping both operations under this lock lets a UI sync
            // take the matching frame atomically instead of remembering a
            // sequence whose pixels are not available yet.
            self.seq.fetch_add(1, Ordering::Relaxed);
        }
        // Signalled AFTER the frame is readable, so a woken sync always finds
        // the frame that woke it - or a newer one.
        if let Ok(guard) = self.wake.lock() {
            if let Some(wake) = guard.as_ref() {
                let _ = wake.try_send(());
            }
        }
    }

    /// Install the wake the pane's clock listens on. The engine never needs
    /// one: without it a store simply publishes into the slot for the next
    /// poll, which is what the tests and a hidden pane want.
    pub(crate) fn set_wake(&self, wake: async_channel::Sender<()>) {
        if let Ok(mut guard) = self.wake.lock() {
            *guard = Some(wake);
        }
    }

    /// The newest frame, with its sequence.
    #[cfg(test)]
    pub(crate) fn latest(&self) -> Option<Frame> {
        self.frame.lock().ok().and_then(|slot| slot.clone())
    }

    /// Take a newer frame and its sequence under one lock.
    ///
    /// The frame pump publishes both while holding `frame`, so the pane cannot
    /// observe a new sequence before the matching pixels are available. Taking
    /// ownership lets it move those pixels into the render image without a
    /// full viewport copy.
    pub(crate) fn take_newer(&self, after: u64) -> Option<(u64, Frame)> {
        let mut slot = self.frame.lock().ok()?;
        let seq = self.seq.load(Ordering::Relaxed);
        if seq == after {
            return None;
        }
        Some((seq, slot.take()?))
    }

    /// The newest sequence number, zero before the first capture.
    #[cfg(test)]
    pub(crate) fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests;
