//! The native view's script channel.
//!
//! The pane renders a `gpui_webview::WebView`, which is `!Send` and lives on
//! the UI thread. The agent tools run on the turn's runtime, so every
//! interaction with the view is queued here and drained by the pane's own
//! clock on the UI thread. A tool posts a command and blocks on its reply,
//! which keeps the tools' synchronous shape without moving the view.

use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

/// How long a tool waits for the view to answer one command.
///
/// The pane drains the queue on its own clock, so the wait covers at most one
/// tick plus the script itself; a view that has gone away must not hang the
/// tool, and this bound is what turns that into an error.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// What the tools need from a page runner: one command, one answer.
///
/// The pane's native view and the test harness both implement this. That is
/// the seam that makes the tool path testable without a native window: the
/// headless suite runs the SAME tool code against Chromium over CDP, so the
/// web route is covered by a permanent test rather than a probe.
pub(crate) trait ScriptRunner: Send + Sync {
    /// Run one command and return its answer, JSON-encoded.
    fn run(&self, command: ScriptCommand) -> Result<String, String>;
}

impl ScriptRunner for ScriptBridge {
    fn run(&self, command: ScriptCommand) -> Result<String, String> {
        self.post(command)
    }
}

/// A runner that answers through Chromium over CDP.
///
/// Test-only, and the reason the tool path has coverage at all: a native view
/// cannot be created in the headless harness, but the commands the tools send
/// are ordinary JavaScript, and the engine can run them.
#[cfg(test)]
pub(crate) struct EngineRunner(pub(crate) std::sync::Arc<crate::browser::engine::Engine>);

#[cfg(test)]
impl ScriptRunner for EngineRunner {
    fn run(&self, command: ScriptCommand) -> Result<String, String> {
        match command {
            ScriptCommand::Eval(js) => {
                let value = self.0.evaluate(&js).map_err(|error| error.message())?;
                serde_json::to_string(&value)
                    .map_err(|error| format!("the answer did not serialize: {error}"))
            }
            ScriptCommand::Load(url) => self
                .0
                .navigate_settling(&url, crate::browser::engine::Settle::Idle)
                .map(|_| String::new())
                .map_err(|error| error.message()),
        }
    }
}

/// One command the UI thread runs against the native view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScriptCommand {
    /// Evaluate JavaScript and answer with its result, as the view returns it.
    Eval(String),
    /// Load a URL in the view.
    Load(String),
}

/// One queued command, with the channel its answer goes back on.
pub(crate) struct ScriptJob {
    /// What to run.
    pub(crate) command: ScriptCommand,
    /// Where the answer goes. Dropped unanswered if the view goes away.
    pub(crate) reply: Sender<Result<String, String>>,
}

/// The bridge between the tools and the native view.
#[derive(Default)]
pub(crate) struct ScriptBridge {
    jobs: Mutex<Vec<ScriptJob>>,
    /// The last URL the view navigated to, as its navigation handler reported.
    ///
    /// The pane reads this to move the address bar; a tool reads it to know a
    /// click navigated somewhere.
    last_url: Mutex<Option<String>>,
    /// A link that asked for a NEW window (`target="_blank"`, `window.open`).
    ///
    /// The pane has one page, so the request is DENIED and the URL is loaded
    /// here instead. Without this a search result - Baidu's are all
    /// `target="_blank"` - looked dead: the click did nothing at all.
    new_window: Mutex<Option<String>>,
}

impl ScriptBridge {
    /// Queue a command and wait for the view's answer.
    pub(crate) fn post(&self, command: ScriptCommand) -> Result<String, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let Ok(mut jobs) = self.jobs.lock() else {
                return Err(String::from("the browser view is gone"));
            };
            jobs.push(ScriptJob { command, reply: tx });
        }
        match rx.recv_timeout(ANSWER_TIMEOUT) {
            Ok(answer) => answer,
            Err(_) => Err(format!(
                "the browser view did not answer within {} s",
                ANSWER_TIMEOUT.as_secs()
            )),
        }
    }

    /// Take everything queued. Called on the UI thread only.
    pub(crate) fn drain(&self) -> Vec<ScriptJob> {
        self.jobs
            .lock()
            .map(|mut jobs| std::mem::take(&mut *jobs))
            .unwrap_or_default()
    }

    /// Record a navigation the view reported.
    pub(crate) fn note_navigation(&self, url: String) {
        if let Ok(mut slot) = self.last_url.lock() {
            *slot = Some(url);
        }
    }

    /// Take the last reported navigation, if one is waiting to be read.
    pub(crate) fn take_navigation(&self) -> Option<String> {
        self.last_url.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Record a link that asked for a new window.
    pub(crate) fn note_new_window(&self, url: String) {
        if let Ok(mut slot) = self.new_window.lock() {
            *slot = Some(url);
        }
    }

    /// Take a pending new-window URL, if a link asked for one.
    pub(crate) fn take_new_window(&self) -> Option<String> {
        self.new_window.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// Read a view answer as JSON.
///
/// Wry answers with the JavaScript value encoded as JSON, and a script that
/// returns `JSON.stringify(...)` therefore arrives as a JSON STRING holding
/// JSON: decoding twice is what turns that back into the object the caller
/// meant. An object answer is used as it is.
pub(crate) fn decode_answer(out: &str) -> Result<serde_json::Value, String> {
    let outer: serde_json::Value = serde_json::from_str(out).unwrap_or(serde_json::Value::Null);
    match outer {
        serde_json::Value::String(text) => serde_json::from_str(&text)
            .map_err(|error| format!("the page answered with text that is not JSON: {error}")),
        other => Ok(other),
    }
}

/// A queue entry that was never run, answered with the reason.
pub(crate) fn reject(job: ScriptJob, reason: &str) {
    let _ = job.reply.send(Err(String::from(reason)));
}
