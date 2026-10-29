//! The rendered browser engine: Chromium over CDP.
//!
//! This module is the PRODUCER half of the Browser pane. The model, the nine
//! `browser_*` tools, the `e1..` refs, the caps and the stale-ref sentence
//! live in `crate::browser` and are deliberately unchanged; what changes here
//! is where a page's title, text and elements come from: the LIVE DOM of a real
//! engine instead of a fetched HTML string.
//!
//! # The operations, and the CDP method behind each
//!
//! - navigate: `Page.enable` (once), `Page.navigate`, wait for DOM content in
//!   the pane or the full load event for tools.
//! - snapshot: `Runtime.evaluate` over the live DOM, stamping `data-bsh-ref`.
//! - click: `Input.dispatchMouseEvent` (moved, pressed, released) at the
//!   element's centre, after `scrollIntoView`.
//! - type: focus plus `Input.insertText`, or a select's own change events.
//! - press: `Input.dispatchKeyEvent`; the PAGE moves focus, not this client.
//! - back/forward: `Page.getNavigationHistory` plus `Page.navigateToHistoryEntry`.
//! - reload: `Page.reload`.
//! - close: `Target.closeTarget`, then the child process.
//! - frame: `Page.captureScreenshot` on a bounded pump.
//! - resize: `Emulation.setDeviceMetricsOverride`.
//!
//! # Refs
//!
//! The browser keeps refs MONOTONIC. This module only stamps them: each
//! interactive element gets `data-bsh-ref="eN"` in the page, so an element
//! that survives two snapshots keeps one ref, and a new document starts a new
//! number range because the stamps went with the old DOM. That is what makes a
//! stale ref detectable without a token the model has to echo back.

pub(crate) mod client;
pub(crate) mod wire;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

use super::FrameSlot;
use client::{Client, NETWORK_IDLE};
use wire::RpcError;

/// How long a navigation waits for `Page.loadEventFired` before it settles
/// with whatever the page has.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the pane waits for document structure, without waiting for
/// images, analytics, fonts or other resources to finish loading.
const PAINT_READY_TIMEOUT: Duration = Duration::from_millis(250);
/// How long after the load event a snapshot waits, so a script that runs on
/// `DOMContentLoaded` lands before the text is read.
const SETTLE: Duration = Duration::from_millis(120);
/// The capture format the pump probes with.
///
/// JPEG, not PNG. MEASURED on this engine at the pane's real 480x744@2x, a
/// still page's capture round trip is format-independent (~50 ms either way,
/// because the cost is the compositor readback and not the encoder), but the
/// DECODE that follows it is not: PNG decodes in 16.8 ms and JPEG in 8.2 ms.
/// At a large viewport the gap widens, because PNG decode scales with pixels
/// (40.3 ms at 960x1488@2x against 25.9 ms for JPEG).
///
/// Losslessness is not needed here: the frame's only other use is being
/// compared as text against the previous probe to decide whether the page
/// changed, and a JPEG's bytes are just as comparable. What the pane shows is
/// a screen capture either way.
const FRAME_FORMAT: &str = "jpeg";
/// The quality for [`FRAME_FORMAT`].
///
/// High enough that text and UI edges stay clean at the pane's size, low
/// enough to keep the payload small. MEASURED: q80 and q90 capture in the same
/// time, so this picks the better-looking of two equally cheap options.
const FRAME_QUALITY: u32 = 80;
/// How many captures the pump keeps outstanding.
///
/// MORE THAN ONE, because the engine's capture is a queue wait and not work.
/// MEASURED at the pane's real size, issuing captures without waiting for
/// each reply costs: 1 in flight ~50.1 ms per capture, 2 ~25.4 ms, 4 ~17.3 ms,
/// 8 ~10.4 ms. Two already halves the wait, and the curve flattens after
/// that, so this takes the cheap part of the win and leaves the engine its
/// headroom. The cost of a deeper pipe is latency: every extra capture in
/// flight is one more frame of delay between a change and the pane showing
/// it, which is exactly what a reader notices.
const PIPELINE_DEPTH: usize = 2;

/// The frame budget while the page is changing: about thirty frames a second,
/// and a BUDGET rather than a sleep.
///
/// MEASURED on this engine: one `Page.captureScreenshot` costs ~50 ms at the
/// pane's real size and is REMARKABLY CONSTANT - a 240x372@2x capture costs the
/// same as a 480x744@2x one, a `clip` to a quarter of the page costs the same
/// as the whole page, and `--disable-gpu` does not move it. The cost is the
/// compositor readback round trip, not the pixels, so it does not shrink with
/// the format, the scale or the region.
///
/// What DOES stack on top of it is the decode, which is why it runs on its own
/// thread (see `spawn_decoder`): the pump starts its next capture as soon as
/// the bytes are handed over, so the ~50 ms round trip overlaps the ~12 ms
/// decode instead of following it. A still page's captures are byte-identical,
/// which is what makes the text comparison worth doing at all.
///
/// The pump sleeps only what is LEFT of this budget after the work it just did:
/// a fixed sleep on top would make the cadence the work plus the sleep, and a
/// page would be displayed slower than the machine can decode it.
const FRAME_FAST: Duration = Duration::from_millis(33);
/// The frame budget while the page moves on its OWN (a carousel, a video).
///
/// Half the input cadence, because the cost is not: MEASURED at the pane's real
/// size (480x744 at 2x), one capture cycle of a changing page saturates a core
/// on this hardware, and a page's own motion driving that at the input rate is
/// what the owner reported as a laggy window. Half the probes keeps motion
/// visible and gives the machine its headroom back.
const FRAME_MOTION: Duration = Duration::from_millis(66);
/// How recently the activity stamp must have moved for the input cadence.
///
/// Long enough to cover a burst of typing or a scroll gesture, short enough
/// that a page left alone returns to its own cadence.
const FRAME_INPUT_WINDOW: Duration = Duration::from_millis(600);
/// Bounded wait once the page has stopped changing, and the interval at which
/// a hidden pane notices that it is wanted again.
const FRAME_IDLE: Duration = Duration::from_millis(120);
/// Unchanged captures in a row before the pump drops to the idle cadence.
///
/// Four is MEASURED, not chosen: a freshly navigated page keeps re-rendering
/// for a few probes (layout, fonts, a script), so a lower count falls back to
/// the idle rate mid-load and the pane appears to arrive in two lumps.
const FRAME_QUIET_TICKS: u32 = 4;
/// How long a navigation waits before its one retry.
const RETRY_PAUSE: Duration = Duration::from_millis(150);

/// `Input.dispatchKeyEvent`'s modifier bits, as the protocol defines them.
///
/// The pane composes this mask from the keystroke it received and the engine
/// puts it on the wire unchanged; keeping the numbers here means the bit that
/// reaches the page is a protocol fact, not a UI convention.
pub(crate) const ALT: u32 = 1;
/// The control bit. On Windows and Linux this is the secondary chord modifier.
pub(crate) const CTRL: u32 = 2;
/// The command/super bit, which is the secondary chord modifier on macOS.
pub(crate) const META: u32 = 4;
/// The shift bit, which turns Cmd/Ctrl-Z into redo.
pub(crate) const SHIFT: u32 = 8;

/// The pointer button an input event belongs to.
///
/// CDP names the button on every event AND reports the held set as a bit mask
/// (left 1, right 2, middle 4). The pane forwards the distinction it received:
/// a press that carried `button: "left"` for every event can never raise a
/// page's `contextmenu`, which is why a right click used to do nothing at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointerButton {
    /// The primary button: selection, links, ordinary clicks.
    Left,
    /// The wheel button: `auxclick` and the platform's middle-click actions.
    Middle,
    /// The secondary button: the page's `contextmenu`.
    Right,
}

impl PointerButton {
    /// The protocol's name for the button, as `button` carries it.
    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
        }
    }

    /// The protocol's bit for the button, as `buttons` carries it.
    fn mask(self) -> u32 {
        match self {
            Self::Left => 1,
            Self::Middle => 4,
            Self::Right => 2,
        }
    }
}

/// Whether a CDP failure is a transport reset rather than the page's own error.
fn is_transport_error(error: &EngineError) -> bool {
    match error {
        EngineError::Rpc { error, .. } => is_transport_text(&error.message),
        EngineError::Transport(_) => true,
        _ => false,
    }
}

/// The words Chromium uses for a dropped connection.
fn is_transport_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("connection reset")
        || lower.contains("error sending request")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
}

/// Why the engine could not do what was asked, in terms the pane can show.
#[derive(Debug, Clone)]
pub(crate) enum EngineError {
    /// No binary was found, and every place that was searched is named.
    MissingBinary {
        /// The places that were searched.
        searched: String,
        /// What to do about it.
        hint: String,
    },
    /// The process did not start.
    Spawn(String),
    /// The process started and died, or never reported a port.
    Startup {
        /// How it ended.
        status: String,
        /// The tail of its own output.
        log: String,
    },
    /// It started but the CDP handshake did not complete.
    Handshake(String),
    /// It refused a method by name, which means this build speaks less CDP.
    Unsupported {
        /// The method this client called.
        method: String,
        /// Its own error.
        error: RpcError,
    },
    /// It answered with an error.
    Rpc {
        /// The method this client called.
        method: String,
        /// Its own error.
        error: RpcError,
    },
    /// The socket failed.
    Transport(String),
    /// The socket ended.
    Closed(String),
    /// It did not answer in time.
    Timeout(String),
}

impl EngineError {
    /// A stable id for the failure, which the pane turns into a label.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            EngineError::MissingBinary { .. } => "binary",
            EngineError::Spawn(_) => "spawn",
            EngineError::Startup { .. } => "startup",
            EngineError::Handshake(_) => "handshake",
            EngineError::Unsupported { .. } => "protocol",
            EngineError::Rpc { .. } => "protocol",
            EngineError::Transport(_) => "transport",
            EngineError::Closed(_) => "closed",
            EngineError::Timeout(_) => "timeout",
        }
    }

    /// The sentence the pane shows.
    pub(crate) fn message(&self) -> String {
        match self {
            EngineError::MissingBinary { searched, .. } => {
                format!("The rendered view needs Chrome or Chromium; it is not at {searched}.")
            }
            EngineError::Spawn(message) => format!("The rendered view could not start: {message}"),
            EngineError::Startup { status, log } => {
                let tail = log.trim();
                if tail.is_empty() {
                    format!("The rendered view's engine stopped before it was ready ({status}).")
                } else {
                    format!(
                        "The rendered view's engine stopped before it was ready ({status}): {}",
                        wire::truncate(tail, 400)
                    )
                }
            }
            EngineError::Handshake(message) => {
                format!("The rendered view's engine started but did not answer CDP: {message}")
            }
            EngineError::Unsupported { method, error } => {
                format!("The rendered view's engine does not implement {method}: {error}")
            }
            EngineError::Rpc { method, error } => {
                if method.is_empty() {
                    format!("The rendered view's engine refused the call: {error}")
                } else {
                    format!("The rendered view's engine refused {method}: {error}")
                }
            }
            EngineError::Transport(message) => {
                format!("The rendered view's connection failed: {message}")
            }
            EngineError::Closed(message) => format!("The rendered view stopped: {message}"),
            EngineError::Timeout(message) => format!("The rendered view timed out: {message}"),
        }
    }

    /// What to do next, the pane's second line.
    pub(crate) fn hint(&self) -> String {
        match self {
            EngineError::MissingBinary { hint, .. } => hint.clone(),
            EngineError::Unsupported { .. } => String::from(
                "This Chromium build does not support the required CDP method; the text view works meanwhile.",
            ),
            _ => String::from(
                "The page's text view is still usable. Close the tab and reopen it to start a new engine.",
            ),
        }
    }
}

/// How long a navigation waits for the page to settle after its load event.
///
/// Two callers share one seam with two needs. A TOOL reads the page's text, so
/// it waits the full networkidle window: a subresource that starts just after
/// load has to land before the text is read. The PANE reads the frame, which
/// is already live while that window would run; it lands its state on one
/// paint beat instead, so the toolbar, the address bar and the history
/// buttons keep up with what is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settle {
    /// Tools: the full networkidle window (load + quiet).
    Idle,
    /// The pane: DOM content, without waiting for subresources to finish.
    Paint,
}

/// What a navigation decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Navigation {
    /// The page loaded.
    Loaded,
}

/// One element as the live DOM describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElementData {
    /// Its ref number.
    pub(crate) id: u64,
    /// link, button, submit, input, textarea or select.
    pub(crate) kind: &'static str,
    /// Its visible label.
    pub(crate) label: String,
    /// A link's absolute target.
    pub(crate) href: Option<String>,
    /// A field's value.
    pub(crate) value: Option<String>,
    /// A field's type.
    pub(crate) input_type: Option<String>,
    /// A select's options.
    pub(crate) options: Vec<String>,
}

/// One page, read from the live DOM.
#[derive(Debug, Clone, Default)]
pub(crate) struct Page {
    /// The final URL.
    pub(crate) url: String,
    /// The document title, empty when it has none.
    pub(crate) title: String,
    /// The rendered text.
    pub(crate) text: String,
    /// The interactive elements, in document order.
    pub(crate) elements: Vec<ElementData>,
    /// The ref the page has focused, when it has one.
    pub(crate) focused: Option<u64>,
    /// The highest ref this read minted.
    pub(crate) max_id: u64,
}

/// What a click decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Activation {
    /// The element is a field and now has focus.
    Focused {
        /// Its ref.
        reference: String,
        /// Its kind.
        kind: String,
        /// Its label.
        label: String,
        /// How the activation was delivered: `pointer` or `script`.
        via: &'static str,
    },
    /// The click ran; the page may have changed.
    Clicked {
        /// How the click was delivered: `pointer` (a synthesized mouse event at
        /// a real box) or `script` (the DOM's own `click()`, used only when the
        /// element genuinely has no box to hit).
        via: &'static str,
    },
}

/// What typing stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Typed {
    /// The ref that was filled.
    pub(crate) reference: String,
    /// Its kind.
    pub(crate) kind: String,
    /// The value it now holds.
    pub(crate) value: String,
    /// Whether it is a password field.
    pub(crate) secret: bool,
    /// Why the type was refused, when it was: `not-on-page` or `not-an-option`.
    pub(crate) error: Option<String>,
    /// The options a select refused, when it refused.
    pub(crate) offered: Vec<String>,
}

/// The live engine.
pub(crate) struct Engine {
    /// The process and its socket.
    client: Arc<Client>,
    /// The viewport the page is laid out at.
    viewport: Mutex<(u32, u32)>,
    /// The device scale factor captures are taken at, which the pane sets to
    /// its window's own so a frame is one texture pixel per screen pixel.
    scale: Mutex<f32>,
    /// Where captured frames go.
    frames: Arc<FrameSlot>,
    /// Whether the frame pump should capture.
    streaming: Arc<AtomicBool>,
    /// The pump thread.
    pump: Mutex<Option<JoinHandle<()>>>,
    /// Told to stop when the engine drops.
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("port", &self.client.port())
            .field("target", &self.client.target())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Start the engine and publish its frames into `frames`.
    pub(crate) fn launch(
        frames: Arc<FrameSlot>,
        viewport: (u32, u32),
    ) -> Result<Self, EngineError> {
        Self::launch_in(frames, viewport, &client::default_state_dir())
    }

    /// Launch with an explicit pidfile directory.
    ///
    /// Tests use a scratch path so the engine's crash bookkeeping never touches
    /// `~/.bos`; production uses the harness data directory.
    pub(crate) fn launch_in(
        frames: Arc<FrameSlot>,
        viewport: (u32, u32),
        state_dir: &std::path::Path,
    ) -> Result<Self, EngineError> {
        let client = Arc::new(Client::launch_at(viewport, state_dir)?);
        let engine = Self {
            client,
            viewport: Mutex::new((viewport.0.max(1), viewport.1.max(1))),
            scale: Mutex::new(1.0),
            frames,
            streaming: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
        };
        // No pump yet: a pre-warmed engine that never shows a frame costs no
        // capture thread at all. set_streaming(true) starts it on first paint.
        Ok(engine)
    }

    /// The client. Tests read its own counters; nothing else needs it.
    #[cfg(test)]
    pub(crate) fn client(&self) -> &Arc<Client> {
        &self.client
    }

    /** Navigate with an explicit settle: [`Settle::Idle`] for a tool that
    reads the page, [`Settle::Paint`] for the pane whose frame is already
    live while the idle window would run. One entry point, so a caller has
    to say which need it has.

    DIRECT, on purpose (owner decision): the browser asks no egress policy,
    before or after the socket. The shell tool and the MCP transport are the
    paths the workspace allowlist governs; this pane reaches the network the
    way any browser does. */
    pub(crate) fn navigate_settling(
        &self,
        url: &str,
        settle: Settle,
    ) -> Result<Navigation, EngineError> {
        self.load(url, settle)
    }

    /// Navigate and wait for the load event.
    fn load(&self, url: &str, settle: Settle) -> Result<Navigation, EngineError> {
        // The request seam's idle clock starts with the navigation, not with
        // the engine's launch: a page that made no request at all is idle as
        // soon as the load event settles.
        self.client.shared().touch();
        // A navigation is a GET: retrying one after a TRANSPORT failure is what
        // a browser does, and it is the one place a reset the gate's proxy
        // cannot avoid (a client that reuses a connection the proxy has already
        // closed) would otherwise reach the reader as a dead page.
        let mut last = None;
        for attempt in 0..2 {
            let before = self.client.shared().loads();
            let before_dom = self.client.shared().dom_contents();
            let command = match settle {
                Settle::Idle => self.client.call("Page.navigate", json!({ "url": url })),
                // Chromium returns from Page.navigate as soon as navigation
                // starts; the explicit DOM event wait below keeps this path
                // responsive while resources continue loading.
                Settle::Paint => self.client.call("Page.navigate", json!({ "url": url })),
            };
            match command {
                Ok(answer) => {
                    if let Some(error) = answer.get("errorText").and_then(Value::as_str) {
                        if attempt == 0 && is_transport_text(error) {
                            last = Some(error.to_string());
                            std::thread::sleep(RETRY_PAUSE);
                            continue;
                        }
                        return Err(EngineError::Rpc {
                            method: String::from("Page.navigate"),
                            error: RpcError {
                                code: 0,
                                message: error.to_string(),
                                data: None,
                            },
                        });
                    }
                    match settle {
                        // A tool reads the page's text, so it waits the real
                        // networkidle when the Fetch seam is counting: the page
                        // is settled when no request has been seen for
                        // NETWORK_IDLE and at most two are in flight, capped at
                        // the navigation timeout. Without the counter this is
                        // load + settle.
                        Settle::Idle => {
                            self.client.wait_load(before, LOAD_TIMEOUT);
                            if !self
                                .client
                                .shared()
                                .wait_network_idle(NETWORK_IDLE, LOAD_TIMEOUT)
                            {
                                std::thread::sleep(SETTLE);
                            }
                        }
                        // The pane can show the live frame before every
                        // subresource finishes. Wait only for parsed DOM so
                        // the text snapshot is useful too, with a short cap
                        // for pages whose parser is blocked by a resource.
                        Settle::Paint => {
                            self.client
                                .wait_dom_content(before_dom, PAINT_READY_TIMEOUT);
                        }
                    }
                    return Ok(Navigation::Loaded);
                }
                Err(error) if attempt == 0 && is_transport_error(&error) => {
                    last = Some(error.message());
                    std::thread::sleep(RETRY_PAUSE);
                }
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Rpc {
            method: String::from("Page.navigate"),
            error: RpcError {
                code: 0,
                message: last.unwrap_or_else(|| String::from("the navigation failed twice")),
                data: None,
            },
        })
    }

    /// Re-read the live DOM, minting refs from `start`.
    pub(crate) fn snapshot(&self, start: u64) -> Result<Page, EngineError> {
        let value = self.evaluate(&snapshot_script(start))?;
        let text = value.as_str().ok_or_else(|| EngineError::Rpc {
            method: String::from("Runtime.evaluate"),
            error: RpcError {
                code: 0,
                message: String::from("the snapshot script did not return a JSON string"),
                data: Some(value.to_string()),
            },
        })?;
        let parsed: Value = serde_json::from_str(text).map_err(|error| EngineError::Rpc {
            method: String::from("Runtime.evaluate"),
            error: RpcError {
                code: 0,
                message: format!("the snapshot JSON did not parse: {error}"),
                data: Some(wire::truncate(text, 300)),
            },
        })?;
        Ok(page_from_json(&parsed))
    }

    /// Click the element a ref names.
    pub(crate) fn click(&self, reference: &str) -> Result<Activation, EngineError> {
        let point = self.evaluate(&point_script(reference))?;
        if let Some(error) = point.get("error").and_then(Value::as_str) {
            return Err(EngineError::Rpc {
                method: String::from("Runtime.evaluate"),
                error: RpcError {
                    code: 0,
                    message: error.to_string(),
                    data: None,
                },
            });
        }
        let kind = point
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("button")
            .to_string();
        let label = point
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let x = point.get("x").and_then(Value::as_f64).unwrap_or(0.0);
        let y = point.get("y").and_then(Value::as_f64).unwrap_or(0.0);
        let zero = point.get("zero").and_then(Value::as_bool).unwrap_or(false);
        if zero {
            // A genuinely boxless target: even a Range over its contents has no
            // client rect (the page script may hide it, or the engine may not
            // lay it out at all). The DOM's own activation is the only route
            // left, and the result says so rather than claiming a pointer click.
            let reference = serde_json::to_string(reference).unwrap_or_default();
            self.evaluate(&format!(
                "(() => {{ const el = document.querySelector('[data-bsh-ref=' + {reference} + ']'); if (el) {{ el.click(); return true; }} return false; }})()"
            ))?;
            std::thread::sleep(SETTLE);
            if matches!(kind.as_str(), "input" | "textarea" | "select") {
                return Ok(Activation::Focused {
                    reference: reference.to_string(),
                    kind,
                    label,
                    via: "script",
                });
            }
            return Ok(Activation::Clicked { via: "script" });
        }
        // A tool-driven activation carries no chord: it is not a reader's
        // gesture and nothing asked for a modified click.
        self.mouse(false, mouse_params("mouseMoved", x, y, None, "none", 0, 0))?;
        self.mouse(
            false,
            mouse_params(
                "mousePressed",
                x,
                y,
                Some(1),
                PointerButton::Left.name(),
                PointerButton::Left.mask(),
                0,
            ),
        )?;
        self.mouse(
            false,
            mouse_params("mouseReleased", x, y, Some(1), "left", 0, 0),
        )?;
        if matches!(kind.as_str(), "input" | "textarea" | "select") {
            // A field takes focus and stops there, which is the behaviour the
            // text view already promised: browser_type is the next call.
            return Ok(Activation::Focused {
                reference: reference.to_string(),
                kind,
                label,
                via: "pointer",
            });
        }
        std::thread::sleep(SETTLE);
        Ok(Activation::Clicked { via: "pointer" })
    }

    /// Fill a field, or choose a select option.
    pub(crate) fn type_into(&self, reference: &str, text: &str) -> Result<Typed, EngineError> {
        let prepare = self.evaluate(&type_script(reference, text))?;
        if let Some(error) = prepare.get("error").and_then(Value::as_str) {
            return Ok(Typed {
                reference: reference.to_string(),
                kind: prepare
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("input")
                    .to_string(),
                value: String::new(),
                secret: false,
                error: Some(error.to_string()),
                offered: prepare
                    .get("offered")
                    .and_then(Value::as_array)
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
        let kind = prepare
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("input")
            .to_string();
        let secret = prepare
            .get("secret")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if prepare
            .get("selected")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(Typed {
                reference: reference.to_string(),
                kind,
                value: prepare
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                secret,
                error: None,
                offered: Vec::new(),
            });
        }
        // A text field: the page has focus and its old text selected, so the
        // insert replaces it exactly.
        self.client
            .call("Input.insertText", json!({ "text": text }))?;
        let value = self.evaluate(&read_value_script(reference))?;
        Ok(Typed {
            reference: reference.to_string(),
            kind,
            value: value
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            secret,
            error: None,
            offered: Vec::new(),
        })
    }

    /// Press one key on the page's focused element.
    ///
    /// With `fire` the events are queued and `Ok(None)` returned: the pane
    /// reads no reply, and a wait per key on the UI thread is the input
    /// path's whole cost. The blocking form returns the ref the page focused
    /// afterwards, when the key moved focus - the tool path's answer.
    pub(crate) fn press(
        &self,
        key: &str,
        fire: bool,
    ) -> Result<Option<(String, String)>, EngineError> {
        self.press_with(key, 0, fire)
    }

    /// Press one key with the physical modifiers it arrived with.
    ///
    /// A bare key name cannot carry a chord, and the difference is not
    /// cosmetic: Shift+Arrow EXTENDS a selection, Cmd/Ctrl+Arrow jumps by a
    /// word or a line, and Alt+Backspace deletes one. Forwarding those as the
    /// plain key (which is what a caller without this seam had to do) sends the
    /// wrong edit to the page. `shift+tab` keeps its spelling because the tool
    /// path names a chord rather than carrying a mask.
    pub(crate) fn press_with(
        &self,
        key: &str,
        modifiers: u32,
        fire: bool,
    ) -> Result<Option<(String, String)>, EngineError> {
        let (name, code, vk, implied) = match key {
            "tab" => ("Tab", "Tab", 9, 0),
            "shift+tab" => ("Tab", "Tab", 9, SHIFT),
            "arrowdown" => ("ArrowDown", "ArrowDown", 40, 0),
            "arrowup" => ("ArrowUp", "ArrowUp", 38, 0),
            "arrowleft" => ("ArrowLeft", "ArrowLeft", 37, 0),
            "arrowright" => ("ArrowRight", "ArrowRight", 39, 0),
            "escape" => ("Escape", "Escape", 27, 0),
            "enter" => ("Enter", "Enter", 13, 0),
            "backspace" => ("Backspace", "Backspace", 8, 0),
            "delete" => ("Delete", "Delete", 46, 0),
            "home" => ("Home", "Home", 36, 0),
            "end" => ("End", "End", 35, 0),
            "pageup" => ("PageUp", "PageUp", 33, 0),
            "pagedown" => ("PageDown", "PageDown", 34, 0),
            other => {
                return Err(EngineError::Rpc {
                    method: String::from("browser_press"),
                    error: RpcError {
                        code: 0,
                        message: format!(
                            "{other} is not a key this view supports. Use Enter, Tab, Shift+Tab, the arrow keys, Backspace, Delete, Home, End, PageUp, PageDown or Escape."
                        ),
                        data: None,
                    },
                })
            }
        };
        let modifiers = i64::from(modifiers | implied);
        let shifted = modifiers & i64::from(SHIFT) != 0;
        self.send(
            fire,
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyDown",
                "key": name,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            }),
        )?;
        // A real browser follows keyDown with a CHAR event for keys that
        // produce text; a page's keypress/submit handler listens for that, not
        // the DOM keydown alone. MEASURED against Baidu: without it the first
        // Enter only closed the suggestion list.
        if let Some(text) = char_text(name) {
            self.send(
                fire,
                "Input.dispatchKeyEvent",
                json!({
                    "type": "char",
                    "key": name,
                    "text": text,
                    "unmodifiedText": text,
                    "windowsVirtualKeyCode": vk,
                    "nativeVirtualKeyCode": vk,
                    "modifiers": modifiers,
                }),
            )?;
        }
        self.send(
            fire,
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": name,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            }),
        )?;
        if fire {
            // The events are queued; the focused-ref read below is the tool
            // path's answer and needs the key to have landed, which a fired
            // key has not yet.
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(40));
        if key == "enter" {
            std::thread::sleep(SETTLE);
            return Ok(None);
        }
        if key == "escape" {
            return Ok(None);
        }
        let focused = self.evaluate(&focus_script(shifted || key == "arrowup"))?;
        let reference = focused.get("ref").and_then(Value::as_str);
        let label = focused
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(reference.map(|reference| (reference.to_string(), label.to_string())))
    }

    /// Go back one entry in the engine's own history.
    pub(crate) fn back(&self) -> Result<bool, EngineError> {
        self.move_history(-1)
    }

    /// Go forward one entry.
    pub(crate) fn forward(&self) -> Result<bool, EngineError> {
        self.move_history(1)
    }

    /// Move the history cursor by `step`, when that entry exists.
    fn move_history(&self, step: i64) -> Result<bool, EngineError> {
        let history = self.client.call("Page.getNavigationHistory", json!({}))?;
        let current = history
            .get("currentIndex")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let wanted = current + step;
        let entries = history
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if wanted < 0 || wanted as usize >= entries.len() {
            return Ok(false);
        }
        let Some(entry) = entries[wanted as usize].get("id").and_then(Value::as_i64) else {
            return Ok(false);
        };
        let before = self.client.shared().loads();
        self.client
            .call("Page.navigateToHistoryEntry", json!({ "entryId": entry }))?;
        self.client.wait_load(before, LOAD_TIMEOUT);
        std::thread::sleep(SETTLE);
        Ok(true)
    }

    /// Reload as a new request. Direct, like every navigation here.
    pub(crate) fn reload(&self) -> Result<(), EngineError> {
        let before = self.client.shared().loads();
        self.client.call("Page.reload", json!({}))?;
        self.client.wait_load(before, LOAD_TIMEOUT);
        std::thread::sleep(SETTLE);
        Ok(())
    }

    /// The current URL, as the page sees it.
    #[cfg(test)]
    pub(crate) fn url(&self) -> Result<String, EngineError> {
        let value = self.evaluate("location.href")?;
        Ok(value.as_str().unwrap_or_default().to_string())
    }

    /// One PNG frame, for a proof or a saved capture.
    #[cfg(test)]
    pub(crate) fn screenshot(&self) -> Result<Vec<u8>, EngineError> {
        capture(&self.client, "png", None)
    }

    /// Resize the page viewport and set the capture's device scale factor.
    ///
    /// The scale is the window's, so the frame's pixel size is the viewport's
    /// CSS size times the screen's scale and the pane draws it one texture
    /// pixel per screen pixel - the difference between a sharp page and a
    /// blurry one on any scaled display.
    pub(crate) fn resize(&self, viewport: (u32, u32), scale: f32) -> Result<(), EngineError> {
        let clamped = (viewport.0.clamp(320, 4096), viewport.1.clamp(240, 4096));
        let scale = if (0.5..=4.0).contains(&scale) {
            scale
        } else {
            1.0
        };
        if let Ok(mut slot) = self.viewport.lock() {
            *slot = clamped;
        }
        if let Ok(mut slot) = self.scale.lock() {
            *slot = scale;
        }
        self.client.resize(clamped, scale)
    }

    /// The device scale factor the page is captured at.
    pub(crate) fn scale(&self) -> f32 {
        self.scale.lock().map(|slot| *slot).unwrap_or(1.0)
    }

    /// The viewport the page is laid out at.
    pub(crate) fn viewport(&self) -> (u32, u32) {
        self.viewport
            .lock()
            .map(|slot| *slot)
            .unwrap_or((1000, 700))
    }

    /// Dispatch one page command, waiting for the answer or not.
    ///
    /// The pane's input path passes `fire`: a key, a click or a wheel tick
    /// never reads a reply, and waiting for one on the UI thread queues behind
    /// whatever the engine's single worker is doing - MEASURED, a capture at
    /// the pane's own 30 frames a second is 8-21 ms, so a printable key's
    /// three round trips could each block a frame. The socket thread writes
    /// jobs in queue order, so fired events keep their order against each
    /// other and against blocking calls.
    fn send(&self, fire: bool, method: &str, params: Value) -> Result<(), EngineError> {
        if fire {
            self.client.notify(method, params)
        } else {
            self.client.call(method, params).map(|_| ())
        }
    }

    /// Dispatch one mouse event.
    fn mouse(&self, fire: bool, params: Value) -> Result<(), EngineError> {
        self.send(fire, "Input.dispatchMouseEvent", params)
    }

    /// A press at a point: move there, then press and HOLD.
    ///
    /// The release is a separate call. Sending press+release on the down event
    /// is what a click needs, but it makes every drag a burst of clicks and
    /// leaves no way to select text, so the pane forwards the three events it
    /// actually receives. `button` rides the event, so a right press is a
    /// secondary click the page can turn into `contextmenu`.
    /// A press with no chord, the shorthand the tests drive.
    ///
    /// Test-only because the pane always has the reader's chord in hand and
    /// must forward it: an unmodified press is a fact about the GESTURE, not a
    /// convenience the production path should be able to fall back to.
    #[cfg(test)]
    pub(crate) fn pointer_down(
        &self,
        x: f32,
        y: f32,
        clicks: u32,
        button: PointerButton,
        fire: bool,
    ) -> Result<(), EngineError> {
        self.pointer_down_with(x, y, clicks, button, 0, fire)
    }

    /// A press with the chord the reader was holding.
    ///
    /// `modifiers` is what a page reads as `shiftKey`/`ctrlKey`/`metaKey` on
    /// the event: Shift+click extends a selection instead of replacing it, and
    /// a modified click reaches the page's own handler. Without it the page
    /// sees every press as unmodified, which is not what a reader who held
    /// Shift did.
    pub(crate) fn pointer_down_with(
        &self,
        x: f32,
        y: f32,
        clicks: u32,
        button: PointerButton,
        modifiers: u32,
        fire: bool,
    ) -> Result<(), EngineError> {
        let (x, y) = (f64::from(x), f64::from(y));
        self.mouse(
            fire,
            mouse_params("mouseMoved", x, y, None, "none", 0, modifiers),
        )?;
        self.mouse(
            fire,
            mouse_params(
                "mousePressed",
                x,
                y,
                Some(clicks.max(1)),
                button.name(),
                button.mask(),
                modifiers,
            ),
        )
    }

    /// A pointer move: hover when no button is held, a drag while one is.
    ///
    /// Fire-and-forget: a move has no answer worth waiting for, and blocking
    /// the UI thread on every ~16 ms of hover would be the input path's whole
    /// cost. Queue order still preserves move-then-press.
    ///
    /// A drag move names the button it is dragging WITH, not `"none"`: MEASURED,
    /// Chromium only runs its default selection (the word under a double click,
    /// a dragged range, a textarea's own selection) when a move arrives while a
    /// button is down AND the event names that button. `buttons` alone is not
    /// enough - the same drag with `button: "none"` and `buttons: 1` selected
    /// nothing here, in the page, mid-drag and after the release.
    /// A move with no chord, the shorthand the tests drive.
    #[cfg(test)]
    pub(crate) fn pointer_move(
        &self,
        x: f32,
        y: f32,
        pressed: Option<PointerButton>,
    ) -> Result<(), EngineError> {
        self.pointer_move_with(x, y, pressed, 0)
    }

    /// A move with the chord the reader was holding.
    ///
    /// A move carries the chord too, because that is how a page distinguishes a
    /// plain drag (replace the selection) from Shift+drag (extend it).
    pub(crate) fn pointer_move_with(
        &self,
        x: f32,
        y: f32,
        pressed: Option<PointerButton>,
        modifiers: u32,
    ) -> Result<(), EngineError> {
        self.client.notify(
            "Input.dispatchMouseEvent",
            mouse_params(
                "mouseMoved",
                f64::from(x),
                f64::from(y),
                None,
                pressed.map_or("none", PointerButton::name),
                pressed.map_or(0, PointerButton::mask),
                modifiers,
            ),
        )
    }

    /// The release that completes a press.
    ///
    /// `clicks` is the same count the press carried, so the page's `click` and
    /// `dblclick` events see one number. Chromium turns that count into its own
    /// DEFAULT selection - a double click selects the word, a triple click the
    /// line - so nothing here synthesizes one: MEASURED, a press and release
    /// carrying `clickCount: 2` select exactly the word under the pointer, and
    /// `3` selects the line, with no script of ours involved.
    /// A release with no chord, the shorthand the tests drive.
    #[cfg(test)]
    pub(crate) fn pointer_up(
        &self,
        x: f32,
        y: f32,
        clicks: u32,
        button: PointerButton,
        fire: bool,
    ) -> Result<(), EngineError> {
        self.pointer_up_with(x, y, clicks, button, 0, fire)
    }

    /// The release with the chord the reader was holding.
    ///
    /// The `click` event carries the chord too, which is where a page reads
    /// `shiftKey` for Shift+click and where a link's default is skipped when
    /// a modifier asks for a new window.
    pub(crate) fn pointer_up_with(
        &self,
        x: f32,
        y: f32,
        clicks: u32,
        button: PointerButton,
        modifiers: u32,
        fire: bool,
    ) -> Result<(), EngineError> {
        // A release reports no held buttons: `buttons` is the set STILL down,
        // and the one being released has just left it.
        self.mouse(
            fire,
            mouse_params(
                "mouseReleased",
                f64::from(x),
                f64::from(y),
                Some(clicks.max(1)),
                button.name(),
                0,
                modifiers,
            ),
        )
    }

    /// Type one printable character the way a browser does.
    ///
    /// A real browser sends `rawKeyDown` (the page's keydown handler sees the
    /// key), then `char` (the text is inserted once), then `keyUp`. Text that
    /// has no US-keyboard identity - a composed CJK character, an emoji - is
    /// delivered through `Input.insertText` instead, because that is the
    /// composition route and inventing a key identity for it would lie to the
    /// page.
    pub(crate) fn type_key(
        &self,
        text: &str,
        modifiers: u32,
        fire: bool,
    ) -> Result<(), EngineError> {
        let Some((key, code, vk)) = printable_identity(text) else {
            return self.insert_text(text, fire);
        };
        let modifiers = i64::from(modifiers);
        self.send(
            fire,
            "Input.dispatchKeyEvent",
            json!({
                "type": "rawKeyDown",
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            }),
        )?;
        self.send(
            fire,
            "Input.dispatchKeyEvent",
            json!({
                "type": "char",
                "key": key,
                "code": code,
                "text": text,
                "unmodifiedText": text,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            }),
        )?;
        self.send(
            fire,
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            }),
        )?;
        Ok(())
    }

    /// Forward a scroll wheel event.
    ///
    /// Both deltas are in CSS pixels and in CDP's own sign: positive scrolls
    /// the page RIGHT and DOWN. The window gives the opposite sign for the
    /// same motion (its delta moves the content, not the viewport), so the
    /// caller negates; doing it here would hide a convention the two sides
    /// disagree about.
    /// A wheel with no chord, the shorthand the tests drive.
    #[cfg(test)]
    pub(crate) fn scroll(
        &self,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        fire: bool,
    ) -> Result<(), EngineError> {
        self.scroll_with(x, y, delta_x, delta_y, 0, fire)
    }

    /// Forward a wheel event with the chord the reader was holding.
    ///
    /// A page reads the chord on a wheel too: Ctrl/Cmd+wheel is the zoom that
    /// a map, an image viewer or an editor implements itself, and a
    /// Shift+wheel is often a horizontal scroll. Without the mask the page
    /// sees every wheel as a plain one.
    pub(crate) fn scroll_with(
        &self,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        modifiers: u32,
        fire: bool,
    ) -> Result<(), EngineError> {
        // Wheel events arrive at trackpad momentum rates; a blocking round
        // trip per tick would stutter the whole window, and no tick's reply
        // carries information the pane reads.
        self.send(
            fire,
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": f64::from(x),
                "y": f64::from(y),
                "deltaX": f64::from(delta_x),
                "deltaY": f64::from(delta_y),
                "modifiers": i64::from(modifiers),
            }),
        )
    }

    /// Forward typed text.
    pub(crate) fn insert_text(&self, text: &str, fire: bool) -> Result<(), EngineError> {
        self.send(fire, "Input.insertText", json!({ "text": text }))
    }

    /// Send one editing chord as a key event the page can see.
    ///
    /// `rawKeyDown` + `keyUp` with the modifier mask is what a real browser
    /// delivers for Cmd/Ctrl+letter: the page's `keydown` handler runs and sees
    /// `metaKey` (or `ctrlKey`), and no text is produced by the key itself.
    fn chord(
        &self,
        key: &'static str,
        code: &'static str,
        vk: i64,
        modifiers: u32,
        fire: bool,
    ) -> Result<(), EngineError> {
        let modifiers = i64::from(modifiers);
        self.send(
            fire,
            "Input.dispatchKeyEvent",
            json!({
                "type": "rawKeyDown",
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            }),
        )?;
        self.send(
            fire,
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            }),
        )?;
        Ok(())
    }

    /// Copy the page's selection.
    ///
    /// Route: the chord is forwarded, then the page's own `copy` event is
    /// dispatched with a `clipboardData` object so a page that writes its own
    /// clipboard text is honoured. The returned string is the text the PANE
    /// then writes to the system clipboard - bsh never reads the clipboard
    /// here, and never invents contents for it.
    pub(crate) fn copy(&self, modifiers: u32) -> Result<Option<String>, EngineError> {
        self.chord("c", "KeyC", 67, modifiers, false)?;
        let value = self.evaluate(COPY_JS)?;
        Ok(value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(String::from))
    }

    /// Cut the page's selection: the copy route, then the selection is removed
    /// (the page's `cut` event may have done it itself; an accepted event is
    /// the one bsh deletes for).
    pub(crate) fn cut(&self, modifiers: u32) -> Result<Option<String>, EngineError> {
        self.chord("x", "KeyX", 88, modifiers, false)?;
        let value = self.evaluate(CUT_JS)?;
        Ok(value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(String::from))
    }

    /// Paste `text` into the page, once.
    ///
    /// Route chosen and stated: the pane reads the system clipboard only when
    /// the reader presses the paste chord, and hands the string here. The page
    /// receives a real `paste` event carrying that text; if the page accepts
    /// the event (does not prevent default), the same text is inserted through
    /// `Input.insertText`, which is one insertion and therefore cannot double
    /// the field. A page that prevents default owns the insertion itself.
    pub(crate) fn paste(&self, text: &str, modifiers: u32) -> Result<(), EngineError> {
        // Blocking on purpose: the accepted answer decides whether the text is
        // inserted here, so firing the chord would lose that branch. A paste
        // is one deliberate chord, not a per-keystroke event.
        self.chord("v", "KeyV", 86, modifiers, false)?;
        // A JSON string literal is the safe way to embed clipboard text in a
        // script: quotes, newlines and backslashes in the text stay data.
        let json = serde_json::to_string(text).unwrap_or_default();
        let accepted = self.evaluate(&PASTE_JS.replace("%TEXT%", &json))?;
        if accepted.as_bool().unwrap_or(true) {
            self.insert_text(text, false)?;
        }
        Ok(())
    }

    /// Select the focused field, or the whole document.
    ///
    /// The chord is forwarded first, so a page that implements its own
    /// select-all sees the key. CDP does not synthesize a default action for a
    /// chord (MEASURED: neither `commands` nor `execCommand` does anything),
    /// so the selection is made here through the DOM.
    pub(crate) fn select_all(&self, modifiers: u32, fire: bool) -> Result<(), EngineError> {
        self.chord("a", "KeyA", 65, modifiers, fire)?;
        // The selection is made by the script, whose value the pane never
        // reads: fired it still runs, in queue order behind the chord.
        self.send(
            fire,
            "Runtime.evaluate",
            json!({ "expression": SELECT_ALL_JS, "returnByValue": true, "awaitPromise": true }),
        )?;
        Ok(())
    }

    /// Undo (or redo, with SHIFT in the mask).
    ///
    /// Forwarded only: the engine has no editing history to walk, and bsh will
    /// not keep a second copy of the page's text. A page that implements undo
    /// itself receives the chord; a plain field does not undo (stated in the
    /// substrate doc as a residual).
    pub(crate) fn undo(&self, modifiers: u32, fire: bool) -> Result<(), EngineError> {
        self.chord("z", "KeyZ", 90, modifiers, fire)
    }

    /// Forward the find chord. There is no find bar in this pane; the page's own
    /// handler is what receives it.
    pub(crate) fn find(&self, modifiers: u32, fire: bool) -> Result<(), EngineError> {
        self.chord("f", "KeyF", 70, modifiers, fire)
    }

    /// Whether frames are being captured.
    ///
    /// The pump thread is started on the FIRST true, not at launch: a
    /// pre-warmed engine nobody looks at has no capture thread to observe.
    pub(crate) fn set_streaming(&self, on: bool) {
        self.streaming.store(on, Ordering::Relaxed);
        if on {
            self.ensure_pump();
        }
    }

    /// Start the frame pump once, if it is not running.
    fn ensure_pump(&self) {
        let mut slot = match self.pump.lock() {
            Ok(slot) => slot,
            Err(_) => return,
        };
        if slot.is_some() {
            return;
        }
        *slot = self.spawn_pump();
    }

    /// Spawn the capture thread: adopt a popup, probe the page, publish what
    /// changed.
    ///
    /// The probe is `Page.captureScreenshot` compared as TEXT against the last
    /// one, so an unchanged page costs one round trip and no decode, and a
    /// changed one hands its text to the decoder thread rather than decoding
    /// it here (see [`spawn_decoder`]). Three cadences, because the work each
    /// one buys is different:
    ///
    /// - [`FRAME_FAST`] while the reader is INTERACTING (activity within
    ///   [`FRAME_INPUT_WINDOW`]): a keystroke or a click has to be on screen
    ///   before the hand moves on.
    /// - [`FRAME_MOTION`] while the page moves on its OWN (a carousel, a
    ///   video): motion stays visible, at half the capture cost. MEASURED at the
    ///   pane's real size (480x744 at 2x), a capture cycle of a changing page
    ///   saturates a core on this hardware, and the owner reported the window
    ///   as laggy when every page's own motion drove it at the input rate.
    /// - [`FRAME_IDLE`] after [`FRAME_QUIET_TICKS`] unchanged probes: a still
    ///   page costs one capture every 120 ms, and the activity stamp is what
    ///   makes the next input publish promptly instead of waiting for it.
    ///
    /// Why polling and not `Page.startScreencast`: MEASURED on this engine, a
    /// screencast acknowledges its start and then pushes frames for some paints
    /// but not all - a key typed into a field published NO frame (the pane froze
    /// with one character visible while every keystroke reached the page), and a
    /// page animating at 25 fps was published at ~10 fps. A capture probe
    /// answers every change, which is what a browser pane has to do.
    ///
    /// The popup check rides this thread rather than the pane's clock: adopting
    /// one costs two CDP calls, and the frame clock runs on the UI thread.
    fn spawn_pump(&self) -> Option<JoinHandle<()>> {
        // The decode runs on its own thread, so the capture round trip that
        // follows it does not wait behind it. See `spawn_decoder`.
        let decoder = spawn_decoder(self.frames.clone())?;
        let stop = self.stop.clone();
        let streaming = self.streaming.clone();
        let client = self.client.clone();
        std::thread::Builder::new()
            .name(String::from("bsh-browser-pump"))
            .spawn(move || {
                let mut last: Option<String> = None;
                let mut seen_intent = client.intent();
                let mut seen_hover = client.hover();
                let mut quiet = FRAME_QUIET_TICKS;
                // Captures waiting for a reply, oldest first. More than one so
                // the engine always has the next probe queued while it works on
                // the current one - see `PIPELINE_DEPTH`.
                let mut inflight: std::collections::VecDeque<
                    std::sync::mpsc::Receiver<Result<Value, EngineError>>,
                > = std::collections::VecDeque::new();
                // When deliberate input last arrived, so the input cadence can
                // expire back to the motion one.
                let mut interacted = std::time::Instant::now() - FRAME_INPUT_WINDOW;
                // The frame count the last published change was measured from,
                // so a probe cycle can report what it cost.
                let mut cycle_started = std::time::Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    if !streaming.load(Ordering::Relaxed) {
                        // Nothing is watching: drop anything outstanding and
                        // stop spending captures on this pane.
                        inflight.clear();
                        std::thread::sleep(FRAME_IDLE);
                        continue;
                    }
                    // A click can open a window; the pane has one view, so the
                    // page it opened is shown HERE.
                    adopt_popup(&client);
                    // A keystroke, a press, a wheel, a resize or a navigation is
                    // deliberate input: probe at once and stay fast while the
                    // reader is working.
                    if client.intent() != seen_intent {
                        seen_intent = client.intent();
                        quiet = 0;
                        interacted = std::time::Instant::now();
                    }
                    // A hover is worth one probe - `:hover` must land - but it
                    // is not the reader working, so it does not buy the input
                    // cadence.
                    if client.hover() != seen_hover {
                        seen_hover = client.hover();
                        quiet = 0;
                    }
                    // Keep the pipe full. Topping up BEFORE the pause is the
                    // whole point: the engine works on the oldest capture while
                    // this thread waits out the cadence, so the pause and the
                    // engine-side queue wait overlap instead of queueing up.
                    while inflight.len() < PIPELINE_DEPTH {
                        match client.dispatch_async(
                            "Page.captureScreenshot",
                            json!({ "format": FRAME_FORMAT, "quality": FRAME_QUALITY }),
                            Some(client.session()),
                        ) {
                            Ok(reply) => inflight.push_back(reply),
                            // The socket is down; the retry is the next loop.
                            Err(_) => break,
                        }
                    }
                    // Wait out the cadence with the pipe already full. This is
                    // a real sleep, bounded by a wake on deliberate input, and
                    // it is what keeps a still page cheap and an input prompt.
                    let budget = probe_budget(quiet, interacted.elapsed());
                    let remaining = budget.saturating_sub(cycle_started.elapsed());
                    client.wait_intent(remaining);
                    cycle_started = std::time::Instant::now();
                    // Now drain the OLDEST reply. It has had the whole pause to
                    // be produced, so this normally returns immediately; the
                    // timeout is only a guard against a lost reply wedging the
                    // pump. A failed or unanswered probe keeps the previous
                    // frame on screen and the next probe retries.
                    let Some(reply) = inflight.pop_front() else {
                        continue;
                    };
                    if let Ok(Ok(answer)) = reply.recv_timeout(LOAD_TIMEOUT) {
                        if let Some(text) = answer.get("data").and_then(Value::as_str) {
                            if last.as_deref() == Some(text) {
                                quiet = quiet.saturating_add(1);
                            } else {
                                quiet = 0;
                                // Hand the text to the decoder and start the
                                // next capture immediately: the decode must not
                                // be paid in front of the round trip.
                                //
                                // `last` advances ONLY when the hand-off was
                                // accepted. A full slot means the decoder is
                                // behind and this frame is dropped in favour of
                                // the next one - so it must not be remembered as
                                // displayed, or the following probe would see
                                // identical text, count it as unchanged, and let
                                // the pane settle on an image it never received.
                                if decoder.try_send(text.to_string()).is_ok() {
                                    last = Some(text.to_string());
                                }
                            }
                        }
                    }
                }
                // Dropping the sender ends the decoder once it drains.
                drop(decoder);
            })
            .ok()
    }

    /// Run one `Runtime.evaluate` and return its value.
    pub(crate) fn evaluate(&self, expression: &str) -> Result<Value, EngineError> {
        let answer = self.client.call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )?;
        if let Some(details) = answer.get("exceptionDetails") {
            let text = details
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("the page script threw");
            return Err(EngineError::Rpc {
                method: String::from("Runtime.evaluate"),
                error: RpcError {
                    code: 0,
                    message: text.to_string(),
                    data: Some(wire::truncate(&details.to_string(), 300)),
                },
            });
        }
        Ok(answer
            .get("result")
            .and_then(|result| result.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(pump) = self.pump.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = pump.join();
        }
    }
}

/// How long the pump waits before its next probe.
///
/// Three classes, and each is a different budget against the same fixed cost
/// (MEASURED: one `Page.captureScreenshot` of the pane's real 960x1488 page
/// costs 40-135 ms and does not shrink with the format or the scale):
///
/// - [`FRAME_IDLE`] once [`FRAME_QUIET_TICKS`] probes in a row are unchanged.
/// - [`FRAME_FAST`] while the reader is working: a keystroke or a click is
///   visible before the hand moves on.
/// - [`FRAME_MOTION`] while only the page moves (a carousel, a video).
///
/// The page's own cadences never spend more than half the pump's time in a
/// capture: the pause is at least as long as the capture it follows.
/// MEASURED, a pane-sized capture can cost 120 ms, and a cadence under that
/// pins the browser process at its capture ceiling - which is what made the
/// window feel slow while a page animated under a moving pointer.
///
/// A pure function because the rule is the contract; the pump only measures.
fn frame_budget(quiet: u32, since_intent: Duration, capture: Duration) -> Duration {
    if quiet >= FRAME_QUIET_TICKS {
        FRAME_IDLE.max(capture)
    } else if since_intent <= FRAME_INPUT_WINDOW {
        FRAME_FAST
    } else {
        FRAME_MOTION.max(capture)
    }
}

/// The interval between two PROBES, given the same inputs as [`frame_budget`].
///
/// [`frame_budget`] answers "how long may one probe cycle take", which is the
/// right question when a cycle is one blocking capture. With captures
/// pipelined the pump issues the next probe before waiting for the oldest
/// reply, so what the pane receives is a PROBE RATE and the thing to bound is
/// the spacing between probes, not the duration of a cycle.
///
/// Because the engine already overlaps the captures, the spacing needs no
/// division by the depth: the measured rate IS the pipelined rate. Dividing
/// here as well would make the pump issue twice as often as the engine can
/// answer and grow the in-flight set without bound.
fn probe_budget(quiet: u32, since_intent: Duration) -> Duration {
    frame_budget(quiet, since_intent, Duration::ZERO)
}

/// Show a page the page itself opened in THIS pane.
///
/// MEASURED: a `target="_blank"` click - which is how most search results link -
/// leaves a second page target rendering in the browser's own invisible window
/// while the pane keeps the old document, so the click looks dead. The pane has
/// ONE view and one session (every key, click and capture goes through it), so
/// the extra target is closed and its URL navigates the attached one. Closing
/// first also releases that renderer, so repeated clicks cannot pile pages up.
///
/// The navigation is fired, not awaited: this runs on the frame pump, and
/// waiting for a load here would stall the frame clock for the navigation.
fn adopt_popup(client: &Client) {
    let Some((target, url)) = client.take_popup() else {
        return;
    };
    let _ = client.call_browser("Target.closeTarget", json!({ "targetId": target }));
    let _ = client.notify("Page.navigate", json!({ "url": url }));
}

/// Spawn the decode thread and return its handle.
///
/// The pump captures and this thread decodes, because the two costs are
/// different sizes and worth overlapping. MEASURED at the pane's real
/// 480x744@2x: a `Page.captureScreenshot` round trip costs ~50 ms and is
/// format- and size-independent (it is the compositor readback), while the
/// JPEG decode plus the RGBA-to-BGRA swap that follows it costs ~12 ms.
/// Paying the decode on the pump thread put it INSIDE the cycle and made the
/// pane's rate `capture + decode`; handing the bytes across instead makes it
/// `max(capture, decode)`, i.e. the ~50 ms the engine actually takes.
///
/// The channel is bounded to one and the pump uses `try_send`, so a decoder
/// that falls behind cannot build a backlog: the pump drops the frame it
/// could not hand over and keeps capturing, and the newest frame is the one
/// that reaches the pane. That is the same coalescing rule the frame slot
/// itself follows.
fn spawn_decoder(frames: Arc<FrameSlot>) -> Option<async_channel::Sender<String>> {
    let (sender, queue) = async_channel::bounded::<String>(1);
    std::thread::Builder::new()
        .name(String::from("bsh-browser-decode"))
        .spawn(move || {
            while let Ok(text) = queue.recv_blocking() {
                if let Some(bytes) = wire::decode_base64(&text) {
                    store_captured(&frames, &bytes);
                }
            }
        })
        .ok()
        .map(|_| sender)
}

/// Decode one captured frame into the frame slot, paying the channel swap here.
///
/// The one decode path for both the pump and a navigation's paint beat: GPUI's
/// `RenderImage` wants BGRA and a capture is RGBA. MEASURED: handing RGBA over
/// unchanged draws a blue heading orange.
///
/// Runs on the decoder thread, never on the pump or the UI thread.
fn store_captured(frames: &FrameSlot, bytes: &[u8]) {
    let Ok(image) = image::load_from_memory(bytes) else {
        return;
    };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    frames.store(Arc::new(bgra), width, height);
}

/// Capture one frame as the base64 text the engine sends.
///
/// The pump compares this TEXT with the previous probe before decoding
/// anything. A probe is one round trip, and most probes of a page that has
/// stopped moving return bytes identical to the last ones; decoding those
/// would spend a measured 0.72 ms and a 17 KB allocation to prove the page is
/// still. Comparing the text instead costs 0.0007 ms - a thousandth - and only
/// a capture that actually changed is handed to the decoder.
///
/// MEASURED: the round trip this makes is ~50 ms and does not depend on the
/// format, the scale or the region captured - a 240x372@2x capture and a
/// 960x1488@2x one differ only in the DECODE that follows. That is why
/// [`FRAME_FORMAT`] trades PNG for JPEG: it makes the decode half of the
/// pipeline cheaper without touching the half that cannot move.
///
/// Test-only: the pump issues its captures through
/// [`Client::dispatch_async`] so several can be in flight at once, which a
/// blocking helper cannot express. A proof or a saved capture still wants one
/// request and its bytes, and that is what this is for.
#[cfg(test)]
fn capture_b64(client: &Client, format: &str, quality: Option<u32>) -> Result<String, EngineError> {
    let mut params = json!({ "format": format });
    if let Some(quality) = quality {
        params["quality"] = json!(quality);
    }
    let answer = client.call("Page.captureScreenshot", params)?;
    answer
        .get("data")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| EngineError::Rpc {
            method: String::from("Page.captureScreenshot"),
            error: RpcError {
                code: 0,
                message: String::from("the capture carried no data"),
                data: None,
            },
        })
}

/// Capture one frame, decoded. The path a proof or a saved capture takes,
/// where the bytes are wanted whatever they cost - so it is test-only, because
/// the pump deliberately compares text and decodes only a changed capture, and
/// a capture taken on the navigation thread is MEASURED slower than letting
/// the pump's woken probe produce the frame.
#[cfg(test)]
fn capture(client: &Client, format: &str, quality: Option<u32>) -> Result<Vec<u8>, EngineError> {
    let text = capture_b64(client, format, quality)?;
    wire::decode_base64(&text).ok_or_else(|| EngineError::Rpc {
        method: String::from("Page.captureScreenshot"),
        error: RpcError {
            code: 0,
            message: String::from("the capture was not valid base64"),
            data: None,
        },
    })
}

/// The parameters of one pointer event.
///
/// `button` is the event's button name (`none` for a move); `buttons` is the
/// mask of the buttons still held, which is what a page reads to tell a hover
/// from a drag.
fn mouse_params(
    kind: &str,
    x: f64,
    y: f64,
    clicks: Option<u32>,
    button: &str,
    buttons: u32,
    modifiers: u32,
) -> Value {
    let mut params = json!({
        "type": kind,
        "x": x,
        "y": y,
        "button": button,
        "buttons": buttons,
        // The held chord, in CDP's own bits: a page reads `shiftKey` here,
        // which is what makes Shift+click extend a selection and a modified
        // click reach the page's own handler.
        "modifiers": i64::from(modifiers),
    });
    if let Some(count) = clicks {
        params["clickCount"] = json!(count);
    }
    params
}

/// The text a named key produces, when it produces one.
///
/// Only Enter here: printable characters are dispatched as a keydown plus a
/// char event (`type_key`), and composition text goes through
/// `Input.insertText`, so both already reach the page as text.
fn char_text(name: &str) -> Option<&'static str> {
    match name {
        "Enter" => Some("\r"),
        _ => None,
    }
}

/// A printable character's US-keyboard identity: the DOM `key`, `code` and
/// virtual-key number a browser puts on a keydown.
///
/// None means "not a US-keyboard key": composed text (CJK, emoji, an accented
/// character from a dead key) has no honest key identity here, so the caller
/// delivers it as composition text instead of inventing one.
fn printable_identity(text: &str) -> Option<(String, &'static str, i64)> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    if chars.next().is_some() || ch.is_control() {
        return None;
    }
    let key = String::from(ch);
    let (code, vk) = match ch {
        'a'..='z' => (letter_code(ch), i64::from(ch.to_ascii_uppercase() as u8)),
        'A'..='Z' => (letter_code(ch.to_ascii_lowercase()), i64::from(ch as u8)),
        '0'..='9' => (digit_code(ch), i64::from(ch as u8)),
        ' ' => ("Space", 32),
        '`' | '~' => ("Backquote", 192),
        '-' | '_' => ("Minus", 189),
        '=' | '+' => ("Equal", 187),
        '[' | '{' => ("BracketLeft", 219),
        ']' | '}' => ("BracketRight", 221),
        '\\' | '|' => ("Backslash", 220),
        ';' | ':' => ("Semicolon", 186),
        '\'' | '"' => ("Quote", 222),
        ',' | '<' => ("Comma", 188),
        '.' | '>' => ("Period", 190),
        '/' | '?' => ("Slash", 191),
        '!' => ("Digit1", 49),
        '@' => ("Digit2", 50),
        '#' => ("Digit3", 51),
        '$' => ("Digit4", 52),
        '%' => ("Digit5", 53),
        '^' => ("Digit6", 54),
        '&' => ("Digit7", 55),
        '*' => ("Digit8", 56),
        '(' => ("Digit9", 57),
        ')' => ("Digit0", 48),
        _ => return None,
    };
    Some((key, code, vk))
}

/// The `code` of a lowercase ASCII letter.
fn letter_code(lower: char) -> &'static str {
    match lower {
        'a' => "KeyA",
        'b' => "KeyB",
        'c' => "KeyC",
        'd' => "KeyD",
        'e' => "KeyE",
        'f' => "KeyF",
        'g' => "KeyG",
        'h' => "KeyH",
        'i' => "KeyI",
        'j' => "KeyJ",
        'k' => "KeyK",
        'l' => "KeyL",
        'm' => "KeyM",
        'n' => "KeyN",
        'o' => "KeyO",
        'p' => "KeyP",
        'q' => "KeyQ",
        'r' => "KeyR",
        's' => "KeyS",
        't' => "KeyT",
        'u' => "KeyU",
        'v' => "KeyV",
        'w' => "KeyW",
        'x' => "KeyX",
        'y' => "KeyY",
        _ => "KeyZ",
    }
}

/// The `code` of an ASCII digit.
fn digit_code(digit: char) -> &'static str {
    match digit {
        '0' => "Digit0",
        '1' => "Digit1",
        '2' => "Digit2",
        '3' => "Digit3",
        '4' => "Digit4",
        '5' => "Digit5",
        '6' => "Digit6",
        '7' => "Digit7",
        '8' => "Digit8",
        _ => "Digit9",
    }
}

/// Read (and let a page override) the text a copy would put on the clipboard.
///
/// The `copy` event is SYNTHETIC - a page sees `isTrusted: false`, because a
/// CDP client cannot mint a trusted clipboard event - but its
/// `clipboardData` object behaves like the real one, so a page that writes its
/// own text is honoured and one that only reads the selection gets the default.
const COPY_JS: &str = r#"
(function() {
  var target = document.activeElement || document.body;
  var captured = '';
  var data = {
    setData: function(kind, value) {
      if (kind === 'text/plain' || kind === 'Text') captured = String(value);
    },
    getData: function(kind) { return kind === 'text/plain' ? captured : ''; },
    types: ['text/plain']
  };
  var event;
  try { event = new ClipboardEvent('copy', { bubbles: true, cancelable: true }); }
  catch (error) { event = new Event('copy', { bubbles: true, cancelable: true }); }
  try { Object.defineProperty(event, 'clipboardData', { value: data }); } catch (error) {}
  target.dispatchEvent(event);
  if (captured) return captured;
  var field = document.activeElement;
  if (field && (field.tagName === 'INPUT' || field.tagName === 'TEXTAREA') &&
      typeof field.selectionStart === 'number' && typeof field.selectionEnd === 'number' &&
      field.selectionEnd > field.selectionStart) {
    return String(field.value == null ? '' : field.value)
      .slice(field.selectionStart, field.selectionEnd);
  }
  var selection = window.getSelection();
  return selection ? String(selection) : '';
})()
"#;

/// Cut: the copy route, then remove the selection when the page accepted the
/// event. A page that prevents default owns the removal itself.
const CUT_JS: &str = r#"
(function() {
  var target = document.activeElement || document.body;
  var captured = '';
  var data = {
    setData: function(kind, value) {
      if (kind === 'text/plain' || kind === 'Text') captured = String(value);
    },
    getData: function(kind) { return kind === 'text/plain' ? captured : ''; },
    types: ['text/plain']
  };
  var event;
  try { event = new ClipboardEvent('cut', { bubbles: true, cancelable: true }); }
  catch (error) { event = new Event('cut', { bubbles: true, cancelable: true }); }
  try { Object.defineProperty(event, 'clipboardData', { value: data }); } catch (error) {}
  var accepted = target.dispatchEvent(event);
  var text = captured;
  var field = document.activeElement;
  var isField = field && (field.tagName === 'INPUT' || field.tagName === 'TEXTAREA') &&
    typeof field.selectionStart === 'number' && typeof field.selectionEnd === 'number';
  if (!text) {
    if (isField && field.selectionEnd > field.selectionStart) {
      text = String(field.value == null ? '' : field.value)
        .slice(field.selectionStart, field.selectionEnd);
    } else {
      var selection = window.getSelection();
      text = selection ? String(selection) : '';
    }
  }
  if (accepted) {
    if (isField && field.selectionEnd > field.selectionStart) {
      var value = String(field.value == null ? '' : field.value);
      var start = field.selectionStart;
      var end = field.selectionEnd;
      field.value = value.slice(0, start) + value.slice(end);
      field.setSelectionRange(start, start);
      var input;
      try { input = new InputEvent('input', { bubbles: true, inputType: 'deleteByCut' }); }
      catch (error) { input = new Event('input', { bubbles: true }); }
      field.dispatchEvent(input);
    } else {
      var chosen = window.getSelection();
      if (chosen && chosen.rangeCount > 0) chosen.getRangeAt(0).deleteContents();
    }
  }
  return text;
})()
"#;

/// Paste: hand the page a paste event carrying the text, and report whether the
/// page accepted it. The caller inserts the text only when accepted.
const PASTE_JS: &str = r#"
(function(text) {
  var target = document.activeElement || document.body;
  var data = {
    getData: function(kind) {
      return kind === 'text/plain' || kind === 'Text' ? text : '';
    },
    types: ['text/plain']
  };
  var event;
  try { event = new ClipboardEvent('paste', { bubbles: true, cancelable: true }); }
  catch (error) { event = new Event('paste', { bubbles: true, cancelable: true }); }
  try { Object.defineProperty(event, 'clipboardData', { value: data }); } catch (error) {}
  return target.dispatchEvent(event);
})(%TEXT%)
"#;

/// Select the focused field's whole value, or the whole document.
const SELECT_ALL_JS: &str = r#"
(function() {
  var field = document.activeElement;
  if (field && (field.tagName === 'INPUT' || field.tagName === 'TEXTAREA') &&
      typeof field.setSelectionRange === 'function') {
    field.setSelectionRange(0, String(field.value == null ? '' : field.value).length);
    return 'field';
  }
  var selection = window.getSelection();
  if (!selection) return 'none';
  var range = document.createRange();
  range.selectNodeContents(document.body);
  selection.removeAllRanges();
  selection.addRange(range);
  return 'document';
})()
"#;

/// The JS that decides what an element is. Shared by both scripts so a click
/// and a snapshot cannot disagree about a ref's kind.
const KIND_JS: &str = r#"
  const name = el.tagName.toLowerCase();
  let kind = 'button';
  if (name === 'a' || name === 'area') kind = 'link';
  else if (name === 'button') kind = (el.type || 'submit') === 'submit' ? 'submit' : 'button';
  else if (name === 'select') kind = 'select';
  else if (name === 'textarea') kind = 'textarea';
  else if (name === 'input') kind = (el.type === 'submit' || el.type === 'button') ? 'submit' : 'input';
  const label = ((el.innerText || '').trim().replace(/\s+/g, ' ')) ||
    el.getAttribute('aria-label') || el.getAttribute('placeholder') ||
    el.getAttribute('name') || el.getAttribute('title') || el.type || name || kind;
"#;

/// The snapshot expression: read the live DOM and stamp refs.
fn snapshot_script(start: u64) -> String {
    format!(
        r#"(() => {{
  const START = {start};
  const SELECTOR = 'a[href],area[href],button,input:not([type="hidden"]),textarea,select,[role="button"],[role="link"],[contenteditable="true"],[contenteditable=""]';
  let next = START;
  const elements = [];
  for (const el of Array.from(document.querySelectorAll(SELECTOR))) {{
    const style = window.getComputedStyle(el);
    // MEASURED: Chromium's inline layout reports a ZERO rect for an <a>, so
    // size cannot be the visibility test or every link would disappear from
    // the model. Only a declared hidden style is.
    if (style.display === 'none' || style.visibility === 'hidden') continue;
    let mark = el.getAttribute('data-bsh-ref');
    let id = mark ? parseInt(mark.slice(1), 10) : NaN;
    if (!Number.isFinite(id) || id < START) {{ id = next++; el.setAttribute('data-bsh-ref', 'e' + id); }}
    {KIND_JS}
    const item = {{ ref: 'e' + id, kind: kind, label: label }};
    if (kind === 'link') item.href = el.href || null;
    if (kind === 'input' || kind === 'textarea' || kind === 'select') {{
      item.value = typeof el.value === 'string' ? el.value : '';
      item.type = el.type || 'text';
    }}
    if (kind === 'select') item.options = Array.from(el.options || []).map(o => (o.textContent || '').trim()).filter(Boolean);
    elements.push(item);
  }}
  let focused = null;
  if (document.activeElement && document.activeElement.getAttribute) {{
    focused = document.activeElement.getAttribute('data-bsh-ref');
  }}
  return JSON.stringify({{
    url: location.href,
    title: document.title || '',
    text: document.body ? document.body.innerText : '',
    elements: elements,
    focused: focused,
    maxId: next - 1
  }});
}})()"#
    )
}

/// The expression that finds one ref's centre, or says why it cannot.
///
/// MEASURED: an inline `<a>` can have a zero `getBoundingClientRect()` box,
/// so using the element box alone would fall back to
/// `element.click()` for every link. The hit point therefore falls back to the
/// union of the element's own client rects and a Range over its CONTENTS, which
/// is where inline text actually lays out when the element box does not. A real
/// point is used whenever one exists; `zero` now means "no box anywhere", not
/// "this engine does not give links a box".
fn point_script(reference: &str) -> String {
    let reference = serde_json::to_string(reference).unwrap_or_else(|_| String::from("\"\""));
    format!(
        r#"(() => {{
  const el = document.querySelector('[data-bsh-ref=' + {reference} + ']');
  if (!el) return {{ error: 'not-on-page' }};
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  {KIND_JS}
  const usable = (r) => !!r && r.width > 0 && r.height > 0;
  let rect = el.getBoundingClientRect();
  let box = usable(rect);
  if (!box) {{
    // Inline targets: the element has no box, but its glyphs may. getClientRects
    // first (cheap), then a Range over the contents, then the union of every
    // usable rect. The union's centre is a point a pointer event can land on.
    let rects = Array.from(el.getClientRects()).filter(usable);
    if (!rects.length && document.createRange) {{
      try {{
        const range = document.createRange();
        range.selectNodeContents(el);
        rects = Array.from(range.getClientRects()).filter(usable);
      }} catch (_) {{}}
    }}
    if (rects.length) {{
      let left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
      for (const r of rects) {{
        left = Math.min(left, r.left); top = Math.min(top, r.top);
        right = Math.max(right, r.right); bottom = Math.max(bottom, r.bottom);
      }}
      if (right > left && bottom > top) {{
        rect = {{ x: left, y: top, width: right - left, height: bottom - top }};
        box = true;
      }}
    }}
  }}
  return {{
    x: rect.x + rect.width / 2,
    y: rect.y + rect.height / 2,
    zero: !box,
    kind: kind,
    label: label
  }};
}})()"#
    )
}

/// The expression that prepares a field to receive text.
fn type_script(reference: &str, text: &str) -> String {
    let reference = serde_json::to_string(reference).unwrap_or_else(|_| String::from("\"\""));
    let text = serde_json::to_string(text).unwrap_or_else(|_| String::from("\"\""));
    format!(
        r#"(() => {{
  const el = document.querySelector('[data-bsh-ref=' + {reference} + ']');
  if (!el) return {{ error: 'not-on-page', kind: 'input' }};
  const name = el.tagName.toLowerCase();
  if (name === 'select') {{
    const wanted = {text}.trim().toLowerCase();
    const options = Array.from(el.options || []);
    const exact = options.find(o => (o.textContent || '').trim().toLowerCase() === wanted);
    const prefix = options.find(o => (o.textContent || '').trim().toLowerCase().startsWith(wanted));
    const chosen = exact || prefix;
    if (!chosen) return {{ error: 'not-an-option', kind: 'select', offered: options.map(o => (o.textContent || '').trim()).filter(Boolean).slice(0, 8) }};
    el.value = chosen.value;
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
    return {{ kind: 'select', selected: true, value: (chosen.textContent || '').trim(), secret: false }};
  }}
  el.focus();
  if (typeof el.select === 'function') el.select();
  return {{ kind: name === 'textarea' ? 'textarea' : 'input', selected: false, secret: el.type === 'password' }};
}})()"#
    )
}

/// The expression that reads a field's value back.
fn read_value_script(reference: &str) -> String {
    let reference = serde_json::to_string(reference).unwrap_or_else(|_| String::from("\"\""));
    format!(
        r#"(() => {{
  const el = document.querySelector('[data-bsh-ref=' + {reference} + ']');
  return {{ value: el && typeof el.value === 'string' ? el.value : '' }};
}})()"#
    )
}

/// The expression that names the page's focused element.
fn focus_script(_backwards: bool) -> String {
    r#"(() => {
  let el = document.activeElement;
  if (!el || el === document.body || !el.getAttribute || !el.getAttribute('data-bsh-ref')) {
    const marked = Array.from(document.querySelectorAll('[data-bsh-ref]'));
    if (!marked.length) return { ref: null, label: '' };
    el = marked[0];
    el.focus();
  }
  const label = ((el.innerText || '').trim().replace(/\s+/g, ' ')) ||
    el.getAttribute('aria-label') || el.getAttribute('placeholder') ||
    el.getAttribute('name') || el.getAttribute('title') || el.tagName.toLowerCase();
  return { ref: el.getAttribute('data-bsh-ref'), label: label };
})()"#
        .to_string()
}

/// Turn the snapshot JSON into a `Page`.
fn page_from_json(value: &Value) -> Page {
    let mut elements = Vec::new();
    for item in value
        .get("elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let reference = item.get("ref").and_then(Value::as_str).unwrap_or("e0");
        let id = reference
            .strip_prefix('e')
            .and_then(|digits| digits.parse::<u64>().ok())
            .unwrap_or(0);
        let kind = match item.get("kind").and_then(Value::as_str) {
            Some("link") => "link",
            Some("button") => "button",
            Some("submit") => "submit",
            Some("textarea") => "textarea",
            Some("select") => "select",
            _ => "input",
        };
        elements.push(ElementData {
            id,
            kind,
            label: item
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            href: item.get("href").and_then(Value::as_str).map(String::from),
            value: item.get("value").and_then(Value::as_str).map(String::from),
            input_type: item.get("type").and_then(Value::as_str).map(String::from),
            options: item
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Page {
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        text: value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        elements,
        focused: value
            .get("focused")
            .and_then(Value::as_str)
            .and_then(|reference| reference.strip_prefix('e'))
            .and_then(|digits| digits.parse::<u64>().ok()),
        max_id: value.get("maxId").and_then(Value::as_u64).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_hand_off_must_not_be_remembered_as_displayed() {
        // The decoder channel is bounded to one, so a pump that outruns the
        // decoder drops a frame. The rule that matters: `last` (what the pane
        // is believed to be showing) may only advance when the hand-off was
        // ACCEPTED. If a dropped frame advanced it, the next probe would see
        // identical text, count it as unchanged, and the pane would settle on
        // an image it never received.
        let (sender, queue) = async_channel::bounded::<String>(1);
        assert!(sender.try_send(String::from("frame-1")).is_ok());
        assert!(
            sender.try_send(String::from("frame-2")).is_err(),
            "a full slot must refuse: that refusal is what stops `last` advancing"
        );
        // Draining one frees exactly one slot, which is the coalescing the
        // pump relies on to catch up with a moving page.
        assert_eq!(queue.recv_blocking().as_deref(), Ok("frame-1"));
        assert!(sender.try_send(String::from("frame-3")).is_ok());
    }

    #[test]
    fn the_pipeline_depth_is_small_and_probed_between_not_around() {
        // Depth 1 is the old serial behaviour and pays the engine's full
        // per-capture queue wait. More than two buys little (MEASURED: 2 ->
        // ~25 ms per capture, 4 -> ~17 ms, 8 -> ~10 ms) and costs latency,
        // because every extra capture in flight is another frame of delay
        // between a change and the pane showing it.
        const {
            assert!(
                PIPELINE_DEPTH >= 2,
                "a serial pump pays the full queue wait"
            );
            assert!(
                PIPELINE_DEPTH <= 4,
                "a deeper pipe is latency the reader feels for little gain"
            );
        }
        // The probe interval must not also be divided by the depth: the engine
        // already overlaps the captures, so dividing here would have the pump
        // issue faster than the engine answers and grow the in-flight set.
        assert_eq!(
            probe_budget(0, Duration::from_millis(10)),
            FRAME_FAST,
            "the probe interval is the frame budget, not the budget over depth"
        );
        assert_eq!(probe_budget(FRAME_QUIET_TICKS, Duration::ZERO), FRAME_IDLE);
    }

    #[test]
    fn the_pump_probes_losselessly_enough_but_decodes_in_jpeg() {
        // The format is a performance decision, not a cosmetic one, so it is
        // pinned: JPEG's decode is measured at roughly half PNG's at the
        // pane's size, and the frame is only ever compared as text before
        // being shown, so nothing depends on the encoding being lossless.
        assert_eq!(FRAME_FORMAT, "jpeg");
        assert!((1..=100).contains(&FRAME_QUALITY));
        // PNG is the one alternative that would still decode, and it is slower.
        assert_ne!(FRAME_FORMAT, "png");
    }

    #[test]
    fn the_frame_budget_follows_input_then_motion_then_stillness() {
        let cheap = Duration::from_millis(1);
        // A still page: the idle probe.
        assert_eq!(
            frame_budget(FRAME_QUIET_TICKS, Duration::ZERO, cheap),
            FRAME_IDLE
        );
        // The page is changing right after input: the input cadence.
        assert_eq!(
            frame_budget(0, Duration::from_millis(10), cheap),
            FRAME_FAST
        );
        // The page is changing on its own: the motion cadence, half the cost.
        assert_eq!(
            frame_budget(0, FRAME_INPUT_WINDOW + Duration::from_millis(1), cheap),
            FRAME_MOTION
        );
        // Stillness wins over a recent intent: nothing changed, so probe slowly.
        assert_eq!(
            frame_budget(FRAME_QUIET_TICKS, Duration::from_millis(10), cheap),
            FRAME_IDLE
        );
        assert!(FRAME_FAST < FRAME_MOTION && FRAME_MOTION < FRAME_IDLE);
        // A slow capture floors the page's own cadences: never more than half
        // the pump's time in a capture.
        let slow = Duration::from_millis(300);
        assert_eq!(frame_budget(FRAME_QUIET_TICKS, Duration::ZERO, slow), slow);
        assert_eq!(
            frame_budget(0, FRAME_INPUT_WINDOW + Duration::from_millis(1), slow),
            slow
        );
        // Input is the reader waiting: it keeps its cadence even on a slow host.
        assert_eq!(frame_budget(0, Duration::from_millis(10), slow), FRAME_FAST);
    }

    #[test]
    fn the_snapshot_script_stamps_a_monotonic_floor() {
        let script = snapshot_script(41);
        assert!(script.contains("const START = 41;"), "{script}");
        assert!(script.contains("data-bsh-ref"), "{script}");
        assert!(script.contains("id < START"), "{script}");
    }

    #[test]
    fn a_ref_is_embedded_as_a_string_literal_not_raw() {
        let script = point_script("e3'\"");
        assert!(
            !script.contains("e3'\"'"),
            "the ref must be escaped: {script}"
        );
    }

    #[test]
    fn mouse_move_carries_the_held_button_for_drag_selection() {
        let drag = mouse_params("mouseMoved", 14.0, 22.0, None, "none", 1, 0);
        let hover = mouse_params("mouseMoved", 14.0, 22.0, None, "none", 0, 0);
        assert_eq!(drag["button"], "none");
        assert_eq!(drag["buttons"], 1);
        assert_eq!(hover["buttons"], 0);
    }

    #[test]
    fn a_right_press_names_the_right_button_and_its_mask() {
        let press = mouse_params("mousePressed", 10.0, 20.0, Some(1), "right", 2, 0);
        assert_eq!(press["button"], "right");
        assert_eq!(press["buttons"], 2);
        // A release reports no held buttons, so the page sees the press end.
        let release = mouse_params("mouseReleased", 10.0, 20.0, Some(1), "right", 0, 0);
        assert_eq!(release["button"], "right");
        assert_eq!(release["buttons"], 0);
    }

    #[test]
    fn the_mouse_params_carry_the_held_chord() {
        // A page reads `shiftKey` from this field, which is what makes
        // Shift+click extend a selection instead of replacing it.
        let shifted = mouse_params("mousePressed", 1.0, 2.0, Some(1), "left", 1, SHIFT);
        assert_eq!(shifted["modifiers"], i64::from(SHIFT));
        let plain = mouse_params("mousePressed", 1.0, 2.0, Some(1), "left", 1, 0);
        assert_eq!(plain["modifiers"], 0);
    }

    #[test]
    fn the_three_buttons_have_distinct_names_and_masks() {
        assert_eq!(PointerButton::Left.name(), "left");
        assert_eq!(PointerButton::Left.mask(), 1);
        assert_eq!(PointerButton::Right.name(), "right");
        assert_eq!(PointerButton::Right.mask(), 2);
        assert_eq!(PointerButton::Middle.name(), "middle");
        assert_eq!(PointerButton::Middle.mask(), 4);
    }

    #[test]
    fn a_snapshot_payload_becomes_the_models_page() {
        let value = json!({
            "url": "http://127.0.0.1/",
            "title": "Probe",
            "text": "hello",
            "focused": "e2",
            "maxId": 2,
            "elements": [
                { "ref": "e1", "kind": "link", "label": "a link", "href": "http://x/y" },
                { "ref": "e2", "kind": "input", "label": "type here", "value": "", "type": "text" }
            ]
        });
        let page = page_from_json(&value);
        assert_eq!(page.title, "Probe");
        assert_eq!(page.elements.len(), 2);
        assert_eq!(page.elements[0].kind, "link");
        assert_eq!(page.elements[0].id, 1);
        assert_eq!(page.elements[1].input_type.as_deref(), Some("text"));
        assert_eq!(page.focused, Some(2));
        assert_eq!(page.max_id, 2);
    }
}
