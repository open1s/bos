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
/// The frame budget while the page is changing: about thirty frames a second,
/// and a BUDGET rather than a sleep.
///
/// MEASURED on this engine: a PNG capture of a STILL page costs ~8 ms and of a
/// CHANGING one ~19 ms (min 6, max 161), and a still page's captures are
/// byte-identical, which is what makes the comparison worth doing. The decode
/// of the frame is the other half of a cycle and is the slow part in a debug
/// build, so the pump sleeps only what is LEFT of this budget after the work
/// it just did: a fixed sleep on top would make the cadence the work plus the
/// sleep, and a page would be displayed slower than the machine can decode it.
const FRAME_FAST: Duration = Duration::from_millis(33);
/// Time to wait for the first pushed frame before falling back to capture RPCs.
const FRAME_START_TIMEOUT: Duration = Duration::from_millis(500);
/// Bounded wait for a compositor frame, also limiting how long the pump takes
/// to notice that the preview pane was hidden.
const FRAME_IDLE: Duration = Duration::from_millis(120);
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
        self.mouse(false, "mouseMoved", x, y, None, 0)?;
        self.mouse(false, "mousePressed", x, y, Some(1), 1)?;
        self.mouse(false, "mouseReleased", x, y, Some(1), 0)?;
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
        let (name, code, vk) = match key {
            "tab" => ("Tab", "Tab", 9),
            "shift+tab" => ("Tab", "Tab", 9),
            "arrowdown" => ("ArrowDown", "ArrowDown", 40),
            "arrowup" => ("ArrowUp", "ArrowUp", 38),
            "arrowleft" => ("ArrowLeft", "ArrowLeft", 37),
            "arrowright" => ("ArrowRight", "ArrowRight", 39),
            "escape" => ("Escape", "Escape", 27),
            "enter" => ("Enter", "Enter", 13),
            "backspace" => ("Backspace", "Backspace", 8),
            "delete" => ("Delete", "Delete", 46),
            "home" => ("Home", "Home", 36),
            "end" => ("End", "End", 35),
            "pageup" => ("PageUp", "PageUp", 33),
            "pagedown" => ("PageDown", "PageDown", 34),
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
        let modifiers = i64::from(key == "shift+tab") * 8;
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
        let focused = self.evaluate(&focus_script(key == "shift+tab" || key == "arrowup"))?;
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

    /// Stream compositor frames as they are painted, rather than polling
    /// `Page.captureScreenshot` after each page update.
    #[cfg(test)]
    pub(crate) fn start_screencast(&self) -> Result<(), EngineError> {
        self.client.start_screencast()
    }

    /// Stop compositor frame delivery.
    #[cfg(test)]
    pub(crate) fn stop_screencast(&self) -> Result<(), EngineError> {
        self.client.stop_screencast()
    }

    /// Wait for the latest compositor frame, coalescing intermediate frames.
    #[cfg(test)]
    pub(crate) fn wait_screencast_frame(&self, timeout: Duration) -> Option<String> {
        self.client.wait_screencast_frame(timeout)
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
    fn mouse(
        &self,
        fire: bool,
        kind: &str,
        x: f64,
        y: f64,
        clicks: Option<u32>,
        buttons: u32,
    ) -> Result<(), EngineError> {
        self.send(
            fire,
            "Input.dispatchMouseEvent",
            mouse_params(kind, x, y, clicks, buttons),
        )
    }

    /// A press at a point: move there, then press and HOLD.
    ///
    /// The release is a separate call. Sending press+release on the down event
    /// is what a click needs, but it makes every drag a burst of clicks and
    /// leaves no way to select text, so the pane forwards the three events it
    /// actually receives.
    pub(crate) fn pointer_down(
        &self,
        x: f32,
        y: f32,
        clicks: u32,
        fire: bool,
    ) -> Result<(), EngineError> {
        let (x, y) = (f64::from(x), f64::from(y));
        self.mouse(fire, "mouseMoved", x, y, None, 0)?;
        self.mouse(fire, "mousePressed", x, y, Some(clicks.max(1)), 1)
    }

    /// A pointer move: hover when no button is held, a drag while one is.
    ///
    /// Fire-and-forget: a move has no answer worth waiting for, and blocking
    /// the UI thread on every ~16 ms of hover would be the input path's whole
    /// cost. Queue order still preserves move-then-press.
    pub(crate) fn pointer_move(&self, x: f32, y: f32, pressed: bool) -> Result<(), EngineError> {
        self.client.notify(
            "Input.dispatchMouseEvent",
            mouse_params(
                "mouseMoved",
                f64::from(x),
                f64::from(y),
                None,
                u32::from(pressed),
            ),
        )
    }

    /// Keep a text drag selected on engines whose mouse pipeline reports the
    /// drag but does not update the page's Selection object itself.
    pub(crate) fn select_text_between(
        &self,
        start: (f32, f32),
        end: (f32, f32),
        fire: bool,
    ) -> Result<(), EngineError> {
        self.send(
            fire,
            "Runtime.evaluate",
            json!({
                "expression": text_selection_script(start, end),
                "returnByValue": true,
            }),
        )
    }

    /// The release that completes a press.
    ///
    /// `clicks` is the same count the press carried, so the page's `click` and
    /// `dblclick` events see one number. With a count of two or more the pane
    /// also selects the word (or, at three, the line) under the pointer: the
    /// engine fires the click events but implements none of the browser's
    /// DEFAULT selection, which is measured, not assumed.
    pub(crate) fn pointer_up(
        &self,
        x: f32,
        y: f32,
        clicks: u32,
        fire: bool,
    ) -> Result<(), EngineError> {
        let (px, py) = (f64::from(x), f64::from(y));
        self.mouse(fire, "mouseReleased", px, py, Some(clicks.max(1)), 0)?;
        if clicks >= 2 {
            // The selection is made by the page, through a script whose value
            // the pane never reads: firing it keeps a double click off the
            // UI thread's wait.
            self.send(
                fire,
                "Runtime.evaluate",
                json!({ "expression": select_at_script(px, py, clicks), "returnByValue": true, "awaitPromise": true }),
            )?;
        }
        Ok(())
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
    pub(crate) fn scroll(
        &self,
        x: f32,
        y: f32,
        delta_y: f32,
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
                "deltaX": 0.0,
                "deltaY": f64::from(delta_y),
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

    /// Spawn the compositor capture thread. Chromium pushes frames as it paints,
    /// avoiding the screenshot RPC round trip on every frame. Screenshot polling
    /// remains a fallback for Chromium builds that cannot start a screencast.
    fn spawn_pump(&self) -> Option<JoinHandle<()>> {
        let stop = self.stop.clone();
        let streaming = self.streaming.clone();
        let frames = self.frames.clone();
        let client = self.client.clone();
        std::thread::Builder::new()
            .name(String::from("bsh-browser-pump"))
            .spawn(move || {
                let mut last: Option<String> = None;
                let mut active = false;
                let mut screencast = false;
                let mut fallback = false;
                let mut started_at = None;
                while !stop.load(Ordering::Relaxed) {
                    let wants_frames = streaming.load(Ordering::Relaxed);
                    if wants_frames != active {
                        active = wants_frames;
                        if active {
                            screencast = client.start_screencast().is_ok();
                            fallback = !screencast;
                            started_at = Some(std::time::Instant::now());
                            last = None;
                        } else if screencast {
                            let _ = client.stop_screencast();
                            screencast = false;
                        }
                    }
                    if !active {
                        std::thread::sleep(FRAME_IDLE);
                        continue;
                    }
                    if screencast {
                        if let Some(text) = client.wait_screencast_frame(FRAME_IDLE) {
                            started_at = None;
                            if last.as_ref() != Some(&text) {
                                if let Some(bytes) = wire::decode_base64(&text) {
                                    store_captured(&frames, &bytes);
                                }
                                last = Some(text);
                            }
                        } else if started_at.is_some_and(|at| at.elapsed() >= FRAME_START_TIMEOUT) {
                            let _ = client.stop_screencast();
                            screencast = false;
                            fallback = true;
                        }
                    }
                    if fallback {
                        if let Ok(text) = capture_b64(&client, "png", None) {
                            if last.as_ref() != Some(&text) {
                                if let Some(bytes) = wire::decode_base64(&text) {
                                    store_captured(&frames, &bytes);
                                }
                                last = Some(text);
                            }
                        }
                        std::thread::sleep(FRAME_FAST);
                    }
                }
                if screencast {
                    let _ = client.stop_screencast();
                }
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

/// Decode a captured PNG into the frame slot, paying the channel swap here.
///
/// The one decode path for both the pump and a navigation's paint beat: GPUI's
/// `RenderImage` wants BGRA and a capture is RGBA. MEASURED: handing RGBA over
/// unchanged draws a blue heading orange.
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
/// a capture that actually changed is decoded.
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
fn mouse_params(kind: &str, x: f64, y: f64, clicks: Option<u32>, buttons: u32) -> Value {
    let mut params = json!({
        "type": kind,
        "x": x,
        "y": y,
        "button": if kind == "mouseMoved" { "none" } else { "left" },
        "buttons": buttons,
    });
    if let Some(count) = clicks {
        params["clickCount"] = json!(count);
    }
    params
}

/// Set the page selection from the text carets under a drag's endpoints.
///
/// Caret APIs use Chromium's rendered text geometry to provide a consistent
/// selection and copy path across native pointer and captured-frame input.
fn text_selection_script(start: (f32, f32), end: (f32, f32)) -> String {
    const SCRIPT: &str = r#"(function(ax, ay, bx, by) {
  function caret(x, y) {
    if (typeof document.caretPositionFromPoint === 'function') {
      var position = document.caretPositionFromPoint(x, y);
      return position && [position.offsetNode, position.offset];
    }
    if (typeof document.caretRangeFromPoint === 'function') {
      var range = document.caretRangeFromPoint(x, y);
      return range && [range.startContainer, range.startOffset];
    }
    return null;
  }
  function editable(node) {
    var element = node && (node.nodeType === 1 ? node : node.parentElement);
    while (element) {
      var tag = element.tagName;
      if (element.isContentEditable || tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') {
        return true;
      }
      element = element.parentElement;
    }
    return false;
  }
  var start = caret(ax, ay);
  var end = caret(bx, by);
  if (!start || !end || start[0].nodeType !== 3 || end[0].nodeType !== 3 ||
      editable(start[0]) || editable(end[0])) return false;
  var selection = document.getSelection();
  if (!selection || typeof selection.setBaseAndExtent !== 'function') return false;
  selection.setBaseAndExtent(start[0], start[1], end[0], end[1]);
  return String(selection);
})(%AX%, %AY%, %BX%, %BY%)"#;
    SCRIPT
        .replace("%AX%", &start.0.to_string())
        .replace("%AY%", &start.1.to_string())
        .replace("%BX%", &end.0.to_string())
        .replace("%BY%", &end.1.to_string())
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

/// The double-click word selection, or the triple-click line/block selection,
/// applied at a page point.
///
/// MEASURED: `mousePressed` with `clickCount: 2` fires
/// `dblclick` (`event.detail` 2) but the DOM's own selection stays empty, and
/// a drag never selects text either. That is the engine's gap, so the pane
/// supplies the DEFAULT selection here - using the engine's own hit testing, a
/// field's own `selectionStart` (which a press does set) or
/// `caretRangeFromPoint` for document text. A page's own handlers still run;
/// this only fills in the missing default.
const SELECT_AT_JS: &str = r#"
(function(x, y, clicks) {
  function wordChar(ch) {
    if (!ch) return false;
    var code = ch.charCodeAt(0);
    return (code >= 48 && code <= 57) || (code >= 65 && code <= 90) ||
      (code >= 97 && code <= 122) || ch === '_' || code > 127;
  }
  var element = document.elementFromPoint ? document.elementFromPoint(x, y) : null;
  var field = element && element.closest ? element.closest('input, textarea') : null;
  if (field && typeof field.setSelectionRange === 'function') {
    var value = String(field.value == null ? '' : field.value);
    var start = field.selectionStart;
    // MEASURED: a click focuses an input/textarea but the CDP input path leaves
    // selectionStart null, so the click's caret is not observable and there is
    // no offset to expand. Guessing (the end of the value) would select the
    // wrong word, so this returns none and the doc states the residual.
    if (typeof start !== 'number') return 'none';
    if (clicks >= 3) {
      var lineStart = 0;
      var lineEnd = value.length;
      if (field.tagName !== 'INPUT') {
        lineStart = value.lastIndexOf(String.fromCharCode(10), Math.max(0, start - 1)) + 1;
        var next = value.indexOf(String.fromCharCode(10), start);
        lineEnd = next < 0 ? value.length : next;
      }
      field.setSelectionRange(lineStart, lineEnd);
      return 'field-line';
    }
    var from = start;
    var to = start;
    while (from > 0 && wordChar(value[from - 1])) from--;
    while (to < value.length && wordChar(value[to])) to++;
    if (from === to) return 'none';
    field.setSelectionRange(from, to);
    return 'field-word';
  }
  if (!document.caretRangeFromPoint) return 'none';
  var range = document.caretRangeFromPoint(x, y);
  if (!range) return 'none';
  if (clicks >= 3) {
    var node = range.startContainer;
    var block = node && node.nodeType === 3 ? node.parentNode : node;
    if (!block || !block.nodeType) return 'none';
    var whole = document.createRange();
    whole.selectNodeContents(block);
    var selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(whole);
    return 'block';
  }
  var textNode = range.startContainer;
  if (!textNode || textNode.nodeType !== 3) return 'none';
  var data = String(textNode.data);
  var at = range.startOffset;
  var wordFrom = at;
  var wordTo = at;
  while (wordFrom > 0 && wordChar(data[wordFrom - 1])) wordFrom--;
  while (wordTo < data.length && wordChar(data[wordTo])) wordTo++;
  if (wordFrom === wordTo) return 'none';
  var word = document.createRange();
  word.setStart(textNode, wordFrom);
  word.setEnd(textNode, wordTo);
  var target = window.getSelection();
  target.removeAllRanges();
  target.addRange(word);
  return 'word';
})(%X%, %Y%, %CLICKS%)
"#;

/// The expression that selects at a point, with the point filled in.
fn select_at_script(x: f64, y: f64, clicks: u32) -> String {
    SELECT_AT_JS
        .replace("%X%", &x.to_string())
        .replace("%Y%", &y.to_string())
        .replace("%CLICKS%", &clicks.to_string())
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
        let drag = mouse_params("mouseMoved", 14.0, 22.0, None, 1);
        let hover = mouse_params("mouseMoved", 14.0, 22.0, None, 0);
        assert_eq!(drag["button"], "none");
        assert_eq!(drag["buttons"], 1);
        assert_eq!(hover["buttons"], 0);
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
