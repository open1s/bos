//! The built-in browser: an honest text view of a page, and the API a model
//! drives it with.
//!
//! # What this is
//!
//! A page is fetched through the workspace policy, then reduced to the four
//! facts a reader or a model can act on: the title, the final URL, the readable
//! text, and the interactive elements. Every interactive element carries a
//! short ref (e1, e2, ...) that the pane draws and the tools accept, so a model
//! never invents a CSS selector and never has to guess which control it saw.
//!
//! This is NOT a rendered page and says so everywhere. No layout engine, no
//! CSS, no script: a page whose content is built by JavaScript shows its
//! no-script text, buttons that only act through a handler refuse with a
//! sentence saying so, and a form can be submitted only as a GET. Those are
//! properties of the view, not bugs to be discovered later.
//!
//! # One source of truth
//!
//! The browser's whole state - document, history, focus, failure - lives in one
//! Browser inside one BrowserHandle. The pane renders it and the tools mutate
//! it, so the model and the reader are looking at the same document at all
//! times; there is no second copy to drift.
//!
//! # Refs
//!
//! Element refs are MONOTONIC, not per page: the first element of the first page
//! is e1 and the next page's elements start where the previous page's stopped.
//! That is what makes staleness detectable without a token the model has to echo
//! back. A ref that is not on the current document is either an old page's ref
//! or a mistyped one, and both get the same actionable answer: the page changed,
//! take a fresh snapshot. A per-page numbering would collide - e3 would mean one
//! element on this page and another on the next, which is exactly the silent
//! wrong action a model must never get.
//!
//! # Policy
//!
//! Every navigation asks the SAME EgressGate the shell's shell tool and MCP
//! transport ask; there is no second allowlist, no bypass and no cached
//! decision that outlives the policy that produced it. The decision is made
//! before a request exists, so a refused URL opens no socket and the pane shows
//! the rule that refused it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent::policy::{EgressGate, FetchError, FetchLimits, FetchedPage, PolicySet, SystemResolver};
use agent::tools::AsyncFunctionTool;
use agent::{Agent, ToolError};

/// The rendered engine: an external Obscura process spoken to over CDP.
///
/// The model below is the ONE surface; this module only changes who produces a
/// document. A build with no binary installed falls back to the text view and
/// the pane says why.
pub(crate) mod engine;

/// What the rendered browser may put on the wire, as a reader's choice.
///
/// The engine is a child process behind a loopback proxy. That proxy can see
/// the absolute request URI of a PLAIN HTTP request, so it enforces the policy
/// on the full host AND path before connecting. An https page is a CONNECT
/// tunnel: TLS is not terminated here (that would need a MITM CA the reader has
/// to trust), so the proxy sees only host:port and a path rule inside the page
/// cannot be decided at the socket.
///
/// That is a limit of the transport, not a gap a control can hide, so it is the
/// reader's explicit choice:
///
/// - [`NetworkMode::Proxied`] (the default) allows https and gates it at
///   host:port - today's behaviour;
/// - [`NetworkMode::Strict`] refuses the CONNECT tunnel outright, so nothing
///   traverses an un-terminated TLS session and every request that does happen
///   was authorized on its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum NetworkMode {
    /// https is allowed and authorized on host:port, like before.
    #[default]
    Proxied,
    /// https is refused, so no un-terminated tunnel carries a request.
    Strict,
}

impl NetworkMode {
    /// Read a stored value; anything but `strict` is the safe default.
    pub(crate) fn from_setting(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("strict") {
            NetworkMode::Strict
        } else {
            NetworkMode::Proxied
        }
    }

    /// The spelling the settings file and a payload carry.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NetworkMode::Proxied => "proxied",
            NetworkMode::Strict => "strict",
        }
    }

    /// The locale key that names the mode to a reader.
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            NetworkMode::Proxied => "right.browser.networkProxied",
            NetworkMode::Strict => "right.browser.networkStrict",
        }
    }
}

/// Characters of page text a snapshot returns.
pub(crate) const TEXT_CAP: usize = 4_000;
/// Elements a snapshot returns.
pub(crate) const ELEMENT_CAP: usize = 60;
/// Characters of raw HTML the ask-for-it path returns.
pub(crate) const RAW_CAP: usize = 6_000;
/// Text the document keeps at all, so a huge page cannot become a huge frame.
pub(crate) const STORE_TEXT_CAP: usize = 40_000;
/// Documents the back/forward history keeps.
pub(crate) const HISTORY_CAP: usize = 20;

/// Why a page could not be shown, in the terms a reader can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    /// A stable id for the cause: scheme, policy, dns, tls, connect, redirect
    /// or body. The pane picks its label from this, so the sentence and the
    /// cause cannot drift.
    pub(crate) kind: &'static str,
    /// The policy's own refusal prose, when the policy produced one.
    pub(crate) message: String,
    /// The rule that refused, when the policy named one.
    pub(crate) rule: Option<String>,
    /// The URL that was attempted.
    pub(crate) url: String,
}

impl Failure {
    /// The pane's locale key for this cause.
    pub(crate) fn key(&self) -> &'static str {
        match self.kind {
            "policy" => "right.browser.reasonPolicy",
            "dns" => "right.browser.reasonDns",
            "tls" => "right.browser.reasonTls",
            "connect" => "right.browser.reasonConnect",
            "scheme" => "right.browser.reasonScheme",
            "redirect" => "right.browser.reasonRedirect",
            "engine" => "right.browser.reasonEngine",
            _ => "right.browser.reasonBody",
        }
    }

    /// What to do next, appended to every tool error.
    ///
    /// A policy refusal gets no advice about widening the policy: the tool
    /// speaks to a model, and "edit the allowlist" is instruction for the
    /// reader, not a next step for the caller.
    pub(crate) fn next_step(&self) -> &'static str {
        match self.kind {
            "policy" => {
                "No request was made. Open a URL this workspace allows, or call browser_snapshot to work with the page already loaded."
            }
            "dns" => {
                "The name did not resolve, so no request was made. Check the host in the URL, or open a different page."
            }
            "tls" => {
                "The TLS handshake failed, so no page was read. The host may not serve HTTPS; try its http URL if it has one."
            }
            "connect" => {
                "The connection failed, so no page was read. The host may be down or the port closed; try again, or open a different page."
            }
            "scheme" => "Only http and https URLs can be opened. Call browser_open with an http(s) URL.",
            "redirect" => "The redirect chain did not settle. Call browser_open with the final URL directly.",
            "engine" => "The rendered view could not read this page. The text view is the fallback: call browser_snapshot to read it, or install Obscura as docs/bsh-browser-substrate.md describes.",
            _ => "The response body could not be read. Call browser_open again.",
        }
    }

    /// The one-line error a tool returns: what happened, then what to do.
    pub(crate) fn tool_message(&self) -> String {
        format!("{} {}", self.message, self.next_step())
    }

    /// The failure for a transport error, in the policy's own words.
    pub(crate) fn from_fetch(error: FetchError, gate: &EgressGate, url: &str) -> Self {
        let (kind, message, rule) = match &error {
            FetchError::Scheme(_) => ("scheme", error.to_string(), None),
            FetchError::Refused(denial) => (
                "policy",
                gate.policy().refusal(&denial.decision),
                denial.decision.rule.clone(),
            ),
            FetchError::Unresolved(_) => ("dns", error.to_string(), None),
            FetchError::Tls(_) => ("tls", error.to_string(), None),
            FetchError::Connect(_) => ("connect", error.to_string(), None),
            FetchError::Redirect(_) => ("redirect", error.to_string(), None),
            FetchError::Body(_) => ("body", error.to_string(), None),
        };
        Self {
            kind,
            message,
            rule,
            url: url.to_string(),
        }
    }

    /// The failure for an engine that could not do what was asked.
    ///
    /// The engine's own sentence travels unchanged, with its install hint, so
    /// the pane and the tool error say the same thing.
    pub(crate) fn from_engine(error: &engine::EngineError, url: &str) -> Self {
        Self {
            kind: "engine",
            message: format!("{} {}", error.message(), error.hint()),
            rule: None,
            url: url.to_string(),
        }
    }
}

/// What an element is, for the kind of interaction a ref accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// An anchor with an href.
    Link,
    /// A button, which can only submit a form here.
    Button,
    /// A submit control.
    Submit,
    /// A single-line field.
    Input,
    /// A multi-line field.
    Textarea,
    /// A drop-down list.
    Select,
}

impl Kind {
    /// The wire name a tool payload carries.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Link => "link",
            Kind::Button => "button",
            Kind::Submit => "submit",
            Kind::Input => "input",
            Kind::Textarea => "textarea",
            Kind::Select => "select",
        }
    }

    /// Whether the element holds a value the model may set.
    fn takes_text(self) -> bool {
        matches!(self, Kind::Input | Kind::Textarea | Kind::Select)
    }
}

/// One interactive element, with the ref that addresses it.
#[derive(Debug, Clone)]
pub(crate) struct Element {
    /// Monotonic id; the wire ref is "e" followed by this.
    pub(crate) id: u64,
    /// What it is.
    pub(crate) kind: Kind,
    /// Its human label: the anchor's text, a button's text, or a field's
    /// placeholder, name or type - whichever the page actually provided.
    pub(crate) label: String,
    /// The absolute target of a link.
    pub(crate) href: Option<String>,
    /// The current value of a field, as the pane shows it.
    pub(crate) value: Option<String>,
    /// The options of a select.
    pub(crate) options: Vec<String>,
    /// The form field name, used when a GET form is submitted.
    pub(crate) name: Option<String>,
    /// The enclosing form, as an index into the document's forms.
    pub(crate) form: Option<usize>,
    /// The input's own type (text, search, password, submit, ...).
    pub(crate) input_type: Option<String>,
}

impl Element {
    /// The ref string the model and the pane use.
    pub(crate) fn reference(&self) -> String {
        format!("e{}", self.id)
    }

    /// Whether a value must not be echoed back.
    fn secret(&self) -> bool {
        self.input_type.as_deref() == Some("password")
    }

    /// The value the wire payload carries: a password field never returns its
    /// content, so a secret typed by the model cannot be read back out of it.
    fn wire_value(&self) -> Option<String> {
        if self.secret() && self.value.is_some() {
            Some(String::from("(hidden)"))
        } else {
            self.value.clone()
        }
    }
}

/// A form a GET submission can be built from.
#[derive(Debug, Clone)]
pub(crate) struct Form {
    /// The resolved action URL.
    pub(crate) action: String,
    /// The method, lower-cased. Only GET is buildable here.
    pub(crate) method: String,
}

/// One fetched page, extracted.
#[derive(Debug, Clone)]
pub(crate) struct Document {
    /// The URL that answered.
    pub(crate) url: String,
    /// The document title, empty when the page has none.
    pub(crate) title: String,
    /// The readable text, bounded to STORE_TEXT_CAP.
    pub(crate) text: String,
    /// Characters extracted before that bound was applied.
    pub(crate) text_total_chars: usize,
    /// The interactive elements, in document order.
    pub(crate) elements: Vec<Element>,
    /// The forms the elements belong to.
    pub(crate) forms: Vec<Form>,
    /// The raw body, for the ask-for-it path only.
    pub(crate) raw: String,
    /// The HTTP status.
    pub(crate) status: u16,
    /// Whether the body itself was truncated by the fetch limit.
    pub(crate) body_truncated: bool,
    /// The highest element id this document minted.
    pub(crate) max_id: u64,
}

impl Document {
    /// Re-mint every ref against next_id.
    ///
    /// A restored history entry goes through this, so a ref belongs to the
    /// document on screen and to nothing else: going back to a page you have
    /// seen does not resurrect the refs you held before.
    fn remint(&mut self, next_id: &mut u64) {
        for element in &mut self.elements {
            element.id = *next_id;
            *next_id += 1;
        }
        self.max_id = next_id.saturating_sub(1);
    }

    /// The element a ref names, when this document holds it.
    fn element(&self, id: u64) -> Option<&Element> {
        self.elements.iter().find(|element| element.id == id)
    }

    /// The same, mutably.
    fn element_mut(&mut self, id: u64) -> Option<&mut Element> {
        self.elements.iter_mut().find(|element| element.id == id)
    }

    /// The bounded view of this document.
    fn snapshot(&self, focused: Option<u64>, can_back: bool, can_forward: bool) -> Snapshot {
        let text: String = self.text.chars().take(TEXT_CAP).collect();
        let text_chars = text.chars().count();
        let truncated = self.text_total_chars > text_chars;
        let elements_total = self.elements.len();
        let elements: Vec<ElementView> = self
            .elements
            .iter()
            .take(ELEMENT_CAP)
            .map(|element| ElementView {
                reference: element.reference(),
                kind: element.kind.as_str(),
                label: element.label.clone(),
                href: element.href.clone(),
                value: element.wire_value(),
                options: element.options.clone(),
                input_type: element.input_type.clone(),
            })
            .collect();
        Snapshot {
            url: self.url.clone(),
            title: if self.title.is_empty() {
                String::from("(untitled)")
            } else {
                self.title.clone()
            },
            text,
            text_chars,
            text_cap: TEXT_CAP,
            text_truncated: truncated,
            text_total_chars: self.text_total_chars,
            elements,
            elements_total,
            elements_cap: ELEMENT_CAP,
            focused: focused.map(|id| format!("e{id}")),
            can_back,
            can_forward,
        }
    }
}

/// One element as the snapshot payload and the pane see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElementView {
    /// The ref the model passes back.
    pub(crate) reference: String,
    /// link, button, submit, input, textarea or select.
    pub(crate) kind: &'static str,
    /// The visible label.
    pub(crate) label: String,
    /// Where a link goes.
    pub(crate) href: Option<String>,
    /// A field's current value. A password's value is hidden.
    pub(crate) value: Option<String>,
    /// A select's options.
    pub(crate) options: Vec<String>,
    /// A field's own type.
    pub(crate) input_type: Option<String>,
}

/// The bounded, structured view of the page: what both the pane and every tool
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// The final URL.
    pub(crate) url: String,
    /// The document title, or (untitled).
    pub(crate) title: String,
    /// Readable text, at most text_cap characters.
    pub(crate) text: String,
    /// Characters actually returned.
    pub(crate) text_chars: usize,
    /// The cap the text obeyed.
    pub(crate) text_cap: usize,
    /// Whether the page had more text than the cap.
    pub(crate) text_truncated: bool,
    /// Characters the page held in total.
    pub(crate) text_total_chars: usize,
    /// The elements, at most elements_cap of them.
    pub(crate) elements: Vec<ElementView>,
    /// Elements the page held in total.
    pub(crate) elements_total: usize,
    /// The cap the list obeyed.
    pub(crate) elements_cap: usize,
    /// The focused ref, when one is.
    pub(crate) focused: Option<String>,
    /// Whether going back is possible.
    pub(crate) can_back: bool,
    /// Whether going forward is possible.
    pub(crate) can_forward: bool,
}
impl Snapshot {
    /// The wire form. Every cap travels with the value it bounds, so a reader
    /// of the payload can never mistake a truncation for a short page.
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let elements: Vec<serde_json::Value> = self
            .elements
            .iter()
            .map(|element| {
                let mut object = serde_json::json!({
                    "ref": element.reference,
                    "kind": element.kind,
                    "label": element.label,
                });
                if let Some(href) = &element.href {
                    object["href"] = serde_json::json!(href);
                }
                if let Some(value) = &element.value {
                    object["value"] = serde_json::json!(value);
                }
                if let Some(input_type) = &element.input_type {
                    object["type"] = serde_json::json!(input_type);
                }
                if !element.options.is_empty() {
                    object["options"] = serde_json::json!(element.options);
                }
                object
            })
            .collect();
        let truncated = self.elements_total > self.elements.len();
        serde_json::json!({
            "url": self.url,
            "title": self.title,
            "text": self.text,
            "text_chars": self.text_chars,
            "text_cap": self.text_cap,
            "text_truncated": self.text_truncated,
            "text_total_chars": self.text_total_chars,
            "elements": elements,
            "elements_total": self.elements_total,
            "elements_cap": self.elements_cap,
            "elements_truncated": truncated,
            "focused": self.focused,
            "can_back": self.can_back,
            "can_forward": self.can_forward,
        })
    }
}

/// What the pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Status {
    /// Nothing loaded: the empty state offers the address bar.
    #[default]
    Empty,
    /// A navigation is in flight.
    Loading,
    /// A document is on screen.
    Ready,
    /// The last navigation failed; Browser::failure says why.
    Failed,
}

impl Status {
    /// The stable id the pane's debug selector and tests use.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Status::Empty => "empty",
            Status::Loading => "loading",
            Status::Ready => "ready",
            Status::Failed => "failed",
        }
    }
}

/// A navigation that has been started but not settled.
#[derive(Debug, Clone)]
struct PendingLoad {
    seq: u64,
    url: String,
}

/// The browser's whole state.
#[derive(Debug)]
pub(crate) struct Browser {
    /// The next element id. Refs start at e1, so a first page reads e1, e2.
    next_id: u64,
    doc: Option<Document>,
    history: Vec<Document>,
    at: usize,
    focused: Option<u64>,
    status: Status,
    failure: Option<Failure>,
    pending: Option<PendingLoad>,
    address: String,
    open: bool,
    seq: u64,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            next_id: 1,
            doc: None,
            history: Vec::new(),
            at: 0,
            focused: None,
            status: Status::Empty,
            failure: None,
            pending: None,
            address: String::new(),
            open: false,
            seq: 0,
        }
    }
}

impl Browser {
    /// Begin a navigation. Returns the sequence number that settles it.
    pub(crate) fn begin(&mut self, url: &str) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        self.pending = Some(PendingLoad {
            seq,
            url: url.to_string(),
        });
        self.status = Status::Loading;
        self.failure = None;
        self.address = url.to_string();
        self.open = true;
        seq
    }

    /// Settle a navigation that produced a page.
    ///
    /// Returns false when a newer navigation replaced this one while it was in
    /// flight; the caller then reports the replacement rather than pretending
    /// its own page is on screen.
    pub(crate) fn land(&mut self, seq: u64, page: FetchedPage) -> bool {
        if self.pending.as_ref().map(|pending| pending.seq) != Some(seq) {
            return false;
        }
        self.pending = None;
        let mut doc = parse_document(&page.url, &page.body, &mut self.next_id);
        doc.status = page.status;
        doc.body_truncated = page.body_truncated;
        doc.raw = page.body;
        self.address = doc.url.clone();
        self.focused = None;
        self.failure = None;
        self.status = Status::Ready;
        self.open = true;
        // A fresh navigation truncates the forward stack, exactly as a browser
        // does: the future that was discarded is not the future this page is in.
        let keep = if self.history.is_empty() {
            0
        } else {
            self.at + 1
        };
        self.history.truncate(keep);
        self.history.push(doc);
        if self.history.len() > HISTORY_CAP {
            self.history.remove(0);
        }
        self.at = self.history.len().saturating_sub(1);
        self.doc = self.history.get(self.at).cloned();
        true
    }

    /// Settle a page read from the live engine.
    ///
    /// Same contract as `land`: a navigation that a newer one replaced settles
    /// nothing, and the caller reports the replacement rather than a page the
    /// reader did not ask for.
    pub(crate) fn land_page(&mut self, seq: u64, page: engine::Page) -> bool {
        if self.pending.as_ref().map(|pending| pending.seq) != Some(seq) {
            return false;
        }
        self.pending = None;
        let focused = page.focused;
        let mut next = self.next_id;
        let doc = document_from_page(page, &mut next);
        self.next_id = next;
        self.adopt(doc, focused);
        true
    }

    /// Put a live page on screen.
    ///
    /// A page whose URL moved is a NEW history entry, exactly as a navigation
    /// is; a page re-read at the same URL (a reload, a snapshot, a click that
    /// only ran script) replaces the entry under the cursor.
    pub(crate) fn adopt(&mut self, doc: Document, focused: Option<u64>) {
        let moved = self
            .doc
            .as_ref()
            .map(|current| current.url != doc.url)
            .unwrap_or(true);
        self.address = doc.url.clone();
        self.focused = focused;
        self.failure = None;
        self.status = Status::Ready;
        self.open = true;
        if moved {
            let keep = if self.history.is_empty() {
                0
            } else {
                self.at + 1
            };
            self.history.truncate(keep);
            self.history.push(doc);
            if self.history.len() > HISTORY_CAP {
                self.history.remove(0);
            }
            self.at = self.history.len().saturating_sub(1);
        } else {
            self.replace_current(doc, focused);
            return;
        }
        self.doc = self.history.get(self.at).cloned();
    }

    /// Replace the entry under the cursor, without moving history.
    pub(crate) fn replace_current(&mut self, doc: Document, focused: Option<u64>) {
        self.address = doc.url.clone();
        self.focused = focused;
        self.failure = None;
        self.status = Status::Ready;
        self.open = true;
        if self.history.is_empty() {
            self.history.push(doc);
            self.at = 0;
        } else {
            if self.at >= self.history.len() {
                self.at = self.history.len() - 1;
            }
            self.history[self.at] = doc;
        }
        self.doc = self.history.get(self.at).cloned();
    }

    /// Move the history cursor one entry, for a page the ENGINE moved to.
    ///
    /// The document on screen is re-read from the live page immediately after,
    /// so this only decides which slot on the pane's own stack the live page
    /// belongs to; it never restores stale markup.
    pub(crate) fn move_history(&mut self, step: isize) -> Result<(), String> {
        if step < 0 {
            if !self.can_back() {
                return Err(String::from(
                    "There is no earlier page in this pane's history. Call browser_snapshot to read the current page.",
                ));
            }
            self.at -= 1;
        } else {
            if !self.can_forward() {
                return Err(String::from(
                    "There is no later page in this pane's history. Call browser_snapshot to read the current page.",
                ));
            }
            self.at += 1;
        }
        self.focused = None;
        self.failure = None;
        Ok(())
    }

    /// Settle a navigation that failed.
    ///
    /// A failed navigation does NOT clear the document already on screen: a
    /// reader who mistypes an address keeps the page they were reading, and the
    /// pane shows the failure over it. The failure is what the address bar's
    /// next attempt replaces.
    pub(crate) fn fail(&mut self, seq: u64, failure: Failure) -> bool {
        if self.pending.as_ref().map(|pending| pending.seq) != Some(seq) {
            return false;
        }
        self.pending = None;
        self.address = failure.url.clone();
        self.status = if self.doc.is_some() {
            Status::Ready
        } else {
            Status::Failed
        };
        self.failure = Some(failure);
        true
    }

    /// The document on screen.
    pub(crate) fn document(&self) -> Option<&Document> {
        self.doc.as_ref()
    }

    /// The current status.
    pub(crate) fn status(&self) -> Status {
        self.status
    }

    /// The last failure, when one is worth showing.
    pub(crate) fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// The address the bar shows.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// Whether going back is possible.
    pub(crate) fn can_back(&self) -> bool {
        self.at > 0
    }

    /// Whether going forward is possible.
    pub(crate) fn can_forward(&self) -> bool {
        !self.history.is_empty() && self.at + 1 < self.history.len()
    }

    /// The bounded view of the current page.
    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        self.doc
            .as_ref()
            .map(|doc| doc.snapshot(self.focused, self.can_back(), self.can_forward()))
    }

    /// Resolve a ref against THIS document, or say why it cannot be.
    fn resolve(&self, reference: &str) -> Result<u64, String> {
        let Some(doc) = self.doc.as_ref() else {
            return Err(String::from(
                "No page is open, so this ref names nothing. Call browser_open with a URL first.",
            ));
        };
        let id = parse_ref(reference).ok_or_else(|| {
            format!(
                "{reference} is not an element ref. Refs look like e3 and come from browser_snapshot; call it to get this page's refs."
            )
        })?;
        if doc.element(id).is_none() {
            let range = match (doc.elements.first(), doc.elements.last()) {
                (Some(first), Some(last)) => format!(
                    "this page's refs run from {} to {}",
                    first.reference(),
                    last.reference()
                ),
                _ => String::from("this page has no interactive elements"),
            };
            return Err(format!(
                "{reference} is not on the page shown now ({range}). The page changed after that ref was taken; call browser_snapshot for fresh refs."
            ));
        }
        Ok(id)
    }

    /// The focused element, when the document still holds it.
    fn focused_element(&self) -> Option<&Element> {
        let id = self.focused?;
        self.doc.as_ref()?.element(id)
    }

    /// Move focus by step elements, wrapping at the ends.
    fn shift_focus(&mut self, step: isize) -> Result<String, String> {
        let Some(doc) = self.doc.as_ref() else {
            return Err(String::from(
                "No page is open, so there is nothing to focus. Call browser_open first.",
            ));
        };
        if doc.elements.is_empty() {
            return Err(String::from(
                "This page has no interactive elements to focus. Call browser_snapshot to read its text instead.",
            ));
        }
        let count = doc.elements.len() as isize;
        let current = doc
            .elements
            .iter()
            .position(|element| Some(element.id) == self.focused)
            .map(|index| index as isize);
        let next = match (current, step) {
            (Some(index), _) => (index + step).rem_euclid(count),
            // No focus yet: the first Tab lands on the first element, the first
            // Shift+Tab on the last.
            (None, _) if step > 0 => 0,
            (None, _) => count - 1,
        };
        let id = doc.elements[next as usize].id;
        let label = doc.elements[next as usize].label.clone();
        self.focused = Some(id);
        Ok(format!("e{id} {label}"))
    }

    /// Back one document, from this pane's own cache.
    pub(crate) fn back(&mut self) -> Result<(), String> {
        if !self.can_back() {
            return Err(String::from(
                "There is no earlier page in this pane's history. Call browser_snapshot to read the current page.",
            ));
        }
        self.at -= 1;
        self.restore_at()
    }

    /// Forward one document.
    pub(crate) fn forward(&mut self) -> Result<(), String> {
        if !self.can_forward() {
            return Err(String::from(
                "There is no later page in this pane's history. Call browser_snapshot to read the current page.",
            ));
        }
        self.at += 1;
        self.restore_at()
    }

    /// Put the history entry at the cursor on screen with fresh refs.
    fn restore_at(&mut self) -> Result<(), String> {
        let mut doc = self.history.get(self.at).cloned().ok_or_else(|| {
            String::from("That history entry is gone. Call browser_open to load a page.")
        })?;
        doc.remint(&mut self.next_id);
        self.address = doc.url.clone();
        self.doc = Some(doc);
        self.focused = None;
        self.failure = None;
        self.status = Status::Ready;
        self.open = true;
        Ok(())
    }

    /// Close the browser and forget the document and its history.
    fn close(&mut self) {
        self.doc = None;
        self.history.clear();
        self.at = 0;
        self.focused = None;
        self.failure = None;
        self.pending = None;
        self.status = Status::Empty;
        self.address.clear();
        self.open = false;
    }
}

/// Parse a ref string back into its element id.
fn parse_ref(reference: &str) -> Option<u64> {
    reference.trim().strip_prefix('e')?.parse().ok()
}

/// Where the rendered engine stands.
enum EngineState {
    /// Nothing has been tried yet.
    Idle,
    /// A live process and its page.
    Ready(Arc<engine::Engine>),
    /// A named failure, kept so a missing binary costs one lookup and not one
    /// per navigation. Closing the browser returns the slot to `Idle`.
    Unavailable(engine::EngineError),
}

/// The shared browser: one state, one revision counter, and the engine that
/// produces its documents.
///
/// The revision is what lets the pane repaint when a TOOL changed the page -
/// the tool runs on the turn's runtime and cannot notify a window, so the shell
/// compares this counter on its own clock instead of holding the lock every
/// frame. Frames are separate from the revision: a capture moves eight times a
/// second and does not change the document, so it has its own sequence.
pub(crate) struct BrowserHandle {
    state: Mutex<Browser>,
    revision: AtomicU64,
    engine: Mutex<EngineState>,
    frames: Arc<engine::FrameSlot>,
    /// Whether this handle may start an engine at all.
    ///
    /// A TEST build defaults this off, so the unit tests of the text view keep
    /// pinning the text producer even on a machine with Obscura installed; the
    /// engine's own tests and the shipped binary turn it on. A test that wants
    /// the engine uses `BrowserHandle::live`.
    engine_disabled: bool,
    /// Where an engine started by this handle keeps its pidfile.
    ///
    /// None is production (the harness data directory). The engine's own tests
    /// point it at a scratch path so crash bookkeeping never touches `~/.bos`.
    engine_state_dir: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for BrowserHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserHandle")
            .field("revision", &self.revision())
            .field("engine", &self.engine_ready())
            .finish_non_exhaustive()
    }
}

impl Default for BrowserHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserHandle {
    /// A browser with nothing loaded.
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(Browser::default()),
            revision: AtomicU64::new(1),
            engine: Mutex::new(EngineState::Idle),
            frames: Arc::new(engine::FrameSlot::default()),
            engine_disabled: cfg!(test),
            engine_state_dir: None,
        }
    }

    /// A handle that starts the rendered engine, for the engine's own tests.
    #[cfg(test)]
    pub(crate) fn live() -> Self {
        let mut handle = Self::new();
        handle.engine_disabled = false;
        handle.engine_state_dir = Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(format!("bsh-browser-state-{}", std::process::id())),
        );
        handle
    }

    /// Read the state. The lock is held only for the duration of f.
    pub(crate) fn read<R>(&self, f: impl FnOnce(&Browser) -> R) -> R {
        let guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&guard)
    }

    /// Change the state, then bump the revision so a pane repaints.
    pub(crate) fn mutate<R>(&self, f: impl FnOnce(&mut Browser) -> R) -> R {
        let result = {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            f(&mut guard)
        };
        self.revision.fetch_add(1, Ordering::Relaxed);
        result
    }

    /// The current revision.
    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// The live engine, when one is running.
    pub(crate) fn engine(&self) -> Option<Arc<engine::Engine>> {
        let slot = self.engine.lock().ok()?;
        match &*slot {
            EngineState::Ready(engine) => Some(engine.clone()),
            _ => None,
        }
    }

    /// Whether a rendered engine is running.
    pub(crate) fn engine_ready(&self) -> bool {
        self.engine().is_some()
    }

    /// The named failure that kept the pane on the text view, if any.
    pub(crate) fn engine_error(&self) -> Option<engine::EngineError> {
        let slot = self.engine.lock().ok()?;
        match &*slot {
            EngineState::Unavailable(error) => Some(error.clone()),
            _ => None,
        }
    }

    /// Where the engine's frames arrive.
    pub(crate) fn frames(&self) -> &Arc<engine::FrameSlot> {
        &self.frames
    }

    /// The live engine, starting one on first use.
    ///
    /// `BSH_BROWSER_ENGINE=off` keeps the process from being started at all,
    /// which is what a machine without the binary wants and what a test that
    /// must not spawn anything sets.
    pub(crate) fn ensure_engine(
        &self,
        gate: &Arc<EgressGate>,
        mode: NetworkMode,
    ) -> Result<Arc<engine::Engine>, engine::EngineError> {
        let mut slot = self
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let EngineState::Ready(engine) = &*slot {
            return Ok(engine.clone());
        }
        if let EngineState::Unavailable(error) = &*slot {
            return Err(error.clone());
        }
        if self.engine_disabled {
            // A test of the text view: no process, no notice, no launch attempt.
            return Err(engine::EngineError::MissingBinary {
                searched: String::from("a text-only browser handle"),
                hint: String::new(),
            });
        }
        if std::env::var("BSH_BROWSER_ENGINE")
            .map(|value| value.eq_ignore_ascii_case("off"))
            .unwrap_or(false)
        {
            let error = engine::EngineError::MissingBinary {
                searched: String::from("BSH_BROWSER_ENGINE=off"),
                hint: String::from(
                    "The rendered view is switched off in this environment; the text view is what the pane shows.",
                ),
            };
            *slot = EngineState::Unavailable(error.clone());
            return Err(error);
        }
        let started =
            engine::Gate::start(gate.policy().clone(), gate.session(), gate.preset(), mode)
                .map_err(engine::EngineError::Spawn)
                .and_then(|gate| match &self.engine_state_dir {
                    Some(state_dir) => {
                        engine::Engine::launch_in(gate, self.frames.clone(), (1000, 700), state_dir)
                    }
                    None => engine::Engine::launch(gate, self.frames.clone(), (1000, 700)),
                });
        match started {
            Ok(engine) => {
                let engine = Arc::new(engine);
                *slot = EngineState::Ready(engine.clone());
                Ok(engine)
            }
            Err(error) => {
                *slot = EngineState::Unavailable(error.clone());
                Err(error)
            }
        }
    }

    /// Stop the engine, if one is running, and allow a later open to retry.
    pub(crate) fn stop_engine(&self) {
        let previous = self
            .engine
            .lock()
            .ok()
            .map(|mut slot| std::mem::replace(&mut *slot, EngineState::Idle));
        // Dropped outside the lock: the engine's own Drop joins its pump and
        // reaps the child, and that must not happen while a reader waits.
        drop(previous);
    }
}

/// Fetch a URL through the policy and settle the result into the browser.
///
/// One place performs a navigation, so the pane's own Go control and every tool
/// make the same decision in the same order: ask the gate, then fetch, then show.
#[cfg(test)]
pub(crate) fn navigate(
    handle: &BrowserHandle,
    gate: &Arc<EgressGate>,
    url: &str,
) -> Result<(), Failure> {
    navigate_in(handle, gate, url, NetworkMode::Proxied)
}

/// Navigate under a chosen [`NetworkMode`].
pub(crate) fn navigate_in(
    handle: &BrowserHandle,
    gate: &Arc<EgressGate>,
    url: &str,
    mode: NetworkMode,
) -> Result<(), Failure> {
    let seq = handle.mutate(|browser| browser.begin(url));
    load(handle, gate, url, seq, mode)
}

/// Fetch and settle a navigation that has ALREADY begun.
///
/// The pane begins the load on the frame that accepted the click, so the
/// loading state paints before the request exists; the fetch then settles the
/// same sequence number. A tool that navigates does both in one call.
pub(crate) fn load(
    handle: &BrowserHandle,
    gate: &Arc<EgressGate>,
    url: &str,
    seq: u64,
    mode: NetworkMode,
) -> Result<(), Failure> {
    // The scheme rule is the model's, not the engine's: a file: or javascript:
    // URL is refused with the text view's own "scheme" failure before a
    // producer is chosen, so the two paths cannot disagree about it.
    if let Err(FetchError::Scheme(attempted)) = agent::policy::http_host(url) {
        let failure = Failure {
            kind: "scheme",
            message: format!("Only http and https URLs can be opened, and {attempted} is neither."),
            rule: None,
            url: url.to_string(),
        };
        let applied = handle.mutate(|browser| browser.fail(seq, failure.clone()));
        return if applied {
            Err(failure)
        } else {
            Err(Failure {
                kind: "body",
                message: String::from("A newer navigation replaced this one before it finished."),
                rule: None,
                url: String::new(),
            })
        };
    }
    match handle.ensure_engine(gate, mode) {
        Ok(engine) => load_live(handle, &engine, gate, url, seq, mode),
        Err(_error) => {
            // The text view is the fallback, and it goes through the SAME gate:
            // the policy decision is identical, only the producer differs.
            let limits = FetchLimits::default();
            let result = agent::policy::fetch(gate, url, &limits);
            settle(handle, gate, seq, result)
        }
    }
}

/// A navigation produced by the live engine.
fn load_live(
    handle: &BrowserHandle,
    engine: &engine::Engine,
    gate: &Arc<EgressGate>,
    url: &str,
    seq: u64,
    mode: NetworkMode,
) -> Result<(), Failure> {
    let replaced = || Failure {
        kind: "engine",
        message: String::from("A newer navigation replaced this one before it finished."),
        rule: None,
        url: String::new(),
    };
    let failure = match engine.navigate(gate, url, mode) {
        Ok(engine::Navigation::Loaded) => {
            let start = handle.read(|browser| browser.next_id);
            match engine.snapshot(start) {
                Ok(page) => {
                    let applied = handle.mutate(|browser| browser.land_page(seq, page));
                    return if applied { Ok(()) } else { Err(replaced()) };
                }
                Err(error) => Failure::from_engine(&error, url),
            }
        }
        Ok(engine::Navigation::Refused { rule, message }) => Failure {
            kind: "policy",
            message,
            rule,
            url: url.to_string(),
        },
        Err(error) => Failure::from_engine(&error, url),
    };
    let applied = handle.mutate(|browser| browser.fail(seq, failure.clone()));
    if applied {
        Err(failure)
    } else {
        Err(replaced())
    }
}

/// Turn a live page into the model's document.
fn document_from_page(page: engine::Page, next_id: &mut u64) -> Document {
    let total = page.text.chars().count();
    let text: String = page.text.chars().take(STORE_TEXT_CAP).collect();
    let elements = page
        .elements
        .into_iter()
        .map(|item| Element {
            id: item.id,
            kind: match item.kind {
                "link" => Kind::Link,
                "submit" => Kind::Submit,
                "textarea" => Kind::Textarea,
                "select" => Kind::Select,
                "button" => Kind::Button,
                _ => Kind::Input,
            },
            label: item.label,
            href: item.href,
            value: item.value,
            options: item.options,
            name: None,
            form: None,
            input_type: item.input_type,
        })
        .collect();
    // Never go backwards: a document with no stamped elements reports
    // max_id = start - 1, and a shrunk range must not hand a live ref to a
    // later page.
    *next_id = (*next_id).max(page.max_id.saturating_add(1));
    Document {
        url: page.url,
        title: page.title,
        text,
        text_total_chars: total,
        elements,
        forms: Vec::new(),
        raw: String::new(),
        status: 200,
        body_truncated: false,
        max_id: page.max_id,
    }
}

/// Settle a fetch into the browser, reporting a superseded load as its own
/// failure rather than showing a page the caller did not ask for.
fn settle(
    handle: &BrowserHandle,
    gate: &EgressGate,
    seq: u64,
    result: Result<FetchedPage, FetchError>,
) -> Result<(), Failure> {
    let replaced = |url: String| Failure {
        kind: "body",
        message: String::from("A newer navigation replaced this one before it finished."),
        rule: None,
        url,
    };
    match result {
        Ok(page) => {
            let applied = handle.mutate(|browser| browser.land(seq, page));
            if applied {
                Ok(())
            } else {
                Err(replaced(String::new()))
            }
        }
        Err(error) => {
            let failure = Failure::from_fetch(error, gate, &requested_url(handle, seq));
            let applied = handle.mutate(|browser| browser.fail(seq, failure.clone()));
            if applied {
                Err(failure)
            } else {
                Err(replaced(failure.url))
            }
        }
    }
}

/// The URL a still-pending sequence number was started for.
fn requested_url(handle: &BrowserHandle, seq: u64) -> String {
    handle.read(|browser| match &browser.pending {
        Some(pending) if pending.seq == seq => pending.url.clone(),
        _ => browser.address.clone(),
    })
}
/// Everything the browser's tools need to decide and act.
#[derive(Clone)]
pub(crate) struct Tools {
    /// The one shared browser.
    pub(crate) browser: Arc<BrowserHandle>,
    /// The turn's compiled policy: the same set the shell's other tools ask.
    pub(crate) policy: Arc<PolicySet>,
    /// The session a decision is recorded under.
    pub(crate) session: String,
    /// The preset a decision is recorded under.
    pub(crate) preset: String,
    /// How the rendered browser treats the network: the reader's setting.
    pub(crate) network_mode: NetworkMode,
}

impl Tools {
    /// The gate one browser tool asks, named for the audit row it produces.
    fn gate(&self, tool: &'static str) -> Arc<EgressGate> {
        Arc::new(EgressGate::new(
            self.policy.clone(),
            Arc::new(SystemResolver),
            self.session.clone(),
            self.preset.clone(),
            tool,
        ))
    }

    /// The honest confinement facts every result carries.
    ///
    /// The audit reads these from the RESULT, so a browser call states its own
    /// reach instead of inheriting the shell tool's. There is no process
    /// sandbox behind this pane - the request is the app's own client and the
    /// policy is the gate - and the network fact is the policy's own data.
    fn facts(&self) -> serde_json::Value {
        let data = self.policy.data();
        let any_rule = !(data.egress_allowlist.is_empty()
            && data.egress_hosts.is_empty()
            && data.egress_ports.is_empty())
            || data.preset_egress_restricted;
        let network = if data.egress_unrestricted && !any_rule {
            "unrestricted"
        } else if !any_rule {
            "deny-all"
        } else {
            "allowlist"
        };
        serde_json::json!({
            "sandbox": "none",
            "network": network,
            // The reader's choice, so a caller can tell what the transport
            // actually enforced: https is gated at host:port under Proxied and
            // refused outright under Strict.
            "network_mode": self.network_mode.as_str(),
        })
    }

    /// Merge the confinement facts into a result object.
    fn stamp(&self, value: &mut serde_json::Value) {
        let facts = self.facts();
        if let (Some(object), serde_json::Value::Object(facts)) = (value.as_object_mut(), facts) {
            for (key, fact) in facts {
                object.insert(key, fact);
            }
        }
    }

    /// The snapshot payload every navigation tool answers with.
    fn snapshot_result(&self, action: &'static str) -> Result<serde_json::Value, ToolError> {
        let mut object = snapshot_payload(&self.browser, action)?;
        self.stamp(&mut object);
        Ok(object)
    }

    /// The raw-HTML payload, which is the one path that returns markup.
    fn html_result(&self) -> Result<serde_json::Value, ToolError> {
        let (snapshot, html) = self.browser.read(|browser| {
            let html = browser
                .document()
                .map(|doc| doc.raw.chars().take(RAW_CAP).collect::<String>())
                .unwrap_or_default();
            (browser.snapshot(), html)
        });
        let Some(snapshot) = snapshot else {
            return Err(ToolError::Failed(String::from(
                "No page is open. Call browser_open with a URL first.",
            )));
        };
        let mut object = serde_json::json!({
            "action": "snapshot_html",
            "title": snapshot.title,
            "url": snapshot.url,
            "html": html,
            "html_cap": RAW_CAP,
            "snapshot": snapshot.to_json(),
        });
        self.stamp(&mut object);
        Ok(object)
    }
}

/// The snapshot payload, without the confinement facts.
///
/// Free so the live-engine helpers below can answer with the same object the
/// text path does: the payload shape is the surface, and it does not depend on
/// which producer filled it.
fn snapshot_payload(
    handle: &BrowserHandle,
    action: &'static str,
) -> Result<serde_json::Value, ToolError> {
    let snapshot = handle.read(|browser| browser.snapshot());
    let Some(snapshot) = snapshot else {
        return Err(ToolError::Failed(String::from(
            "No page is open. Call browser_open with a URL first.",
        )));
    };
    Ok(serde_json::json!({
        "action": action,
        "title": snapshot.title,
        "url": snapshot.url,
        "snapshot": snapshot.to_json(),
    }))
}

/// What the request seam refused, in the policy's own words.
///
/// The document pre-flight cannot see a subresource: an image, a script or a
/// stylesheet the policy refused is reported here, with its rule, so a page
/// that loaded with a piece missing says which piece and why.
pub(crate) fn engine_refusal(handle: &BrowserHandle) -> Option<String> {
    let engine = handle.engine()?;
    let describe = |rule: &Option<String>, message: &str| match rule {
        Some(rule) => format!("{message} (rule {rule})"),
        None => message.to_string(),
    };
    if let Some(denial) = engine.gate().last() {
        return Some(describe(&denial.rule, &denial.message));
    }
    let refusals = engine.client().shared().refusals();
    refusals
        .last()
        .map(|refusal| describe(&refusal.rule, &refusal.message))
}

/// The tool error an engine failure becomes: what happened, then the hint.
fn engine_error(error: &engine::EngineError) -> ToolError {
    ToolError::Failed(format!("{} {}", error.message(), error.hint()))
}

/// Re-read the live page into the entry under the cursor.
fn refresh_live(handle: &BrowserHandle) -> Result<(), ToolError> {
    let Some(engine) = handle.engine() else {
        return Ok(());
    };
    let start = handle.read(|browser| browser.next_id);
    let page = engine
        .snapshot(start)
        .map_err(|error| engine_error(&error))?;
    let focused = page.focused;
    handle.mutate(|browser| {
        let mut next = browser.next_id;
        let doc = document_from_page(page, &mut next);
        browser.next_id = next;
        browser.replace_current(doc, focused);
    });
    Ok(())
}

/// Re-read the live page, letting a changed URL open a history entry.
fn settle_live(handle: &BrowserHandle) -> Result<(), ToolError> {
    let Some(engine) = handle.engine() else {
        return Ok(());
    };
    let start = handle.read(|browser| browser.next_id);
    let page = engine
        .snapshot(start)
        .map_err(|error| engine_error(&error))?;
    let focused = page.focused;
    handle.mutate(|browser| {
        let mut next = browser.next_id;
        let doc = document_from_page(page, &mut next);
        browser.next_id = next;
        browser.adopt(doc, focused);
    });
    Ok(())
}

/// `browser_click` against the live page.
pub(crate) fn click_live(
    handle: &BrowserHandle,
    reference: &str,
) -> Result<serde_json::Value, ToolError> {
    let engine = handle
        .engine()
        .ok_or_else(|| ToolError::Failed(String::from("No rendered engine is running.")))?;
    // The ref is resolved against the model FIRST, so a stale one gets the
    // same sentence the text view gives rather than the engine's own wording.
    handle
        .read(|browser| browser.resolve(reference))
        .map_err(ToolError::Failed)?;
    match engine
        .click(reference)
        .map_err(|error| engine_error(&error))?
    {
        engine::cdp::Activation::Focused {
            reference,
            kind,
            label,
            via,
        } => {
            if let Some(id) = parse_ref(&reference) {
                handle.mutate(|browser| browser.focused = Some(id));
            }
            Ok(serde_json::json!({
                "focused": reference,
                "kind": kind,
                "label": label,
                // "pointer" is a synthesized mouse event at the element's real
                // box; "script" is the DOM's own click(), used only when the
                // element genuinely has no box to hit.
                "via": via,
                "next": "Call browser_type with this ref and the text to enter, then browser_press Enter if it is inside a form."
            }))
        }
        engine::cdp::Activation::Clicked { via } => {
            settle_live(handle)?;
            let mut payload = snapshot_payload(handle, "navigated")?;
            payload["via"] = serde_json::json!(via);
            Ok(payload)
        }
    }
}

/// `browser_type` against the live page.
pub(crate) fn type_live(
    handle: &BrowserHandle,
    reference: &str,
    text: &str,
) -> Result<serde_json::Value, ToolError> {
    let engine = handle
        .engine()
        .ok_or_else(|| ToolError::Failed(String::from("No rendered engine is running.")))?;
    handle
        .read(|browser| browser.resolve(reference))
        .map_err(ToolError::Failed)?;
    let typed = engine
        .type_into(reference, text)
        .map_err(|error| engine_error(&error))?;
    if let Some(error) = &typed.error {
        if error == "not-an-option" {
            let shown: Vec<String> = typed.offered.iter().take(8).cloned().collect();
            return Err(ToolError::Failed(format!(
                "'{}' is not an option of {}. Its options are {:?}; call browser_type with one of them.",
                text, typed.reference, shown
            )));
        }
        return Err(ToolError::Failed(format!(
            "{} is not on this page. Call browser_snapshot.",
            typed.reference
        )));
    }
    refresh_live(handle)?;
    let value = if typed.secret {
        String::from("(hidden)")
    } else {
        typed.value.clone()
    };
    let mut payload = serde_json::json!({
        "ref": typed.reference,
        "kind": typed.kind,
        "value": value,
    });
    if typed.secret {
        payload["hidden"] = serde_json::json!(true);
        payload["note"] = serde_json::json!(
            "The field is a password: its value is stored but never returned, so you cannot read a secret back out of the page."
        );
    }
    Ok(payload)
}

/// `browser_press` against the live page.
pub(crate) fn press_live(
    handle: &BrowserHandle,
    key: &str,
) -> Result<serde_json::Value, ToolError> {
    let engine = handle
        .engine()
        .ok_or_else(|| ToolError::Failed(String::from("No rendered engine is running.")))?;
    if key == "enter" && handle.read(|browser| browser.focused).is_none() {
        return Err(ToolError::Failed(String::from(
            "No element is focused, so Enter has nothing to activate. Call browser_click on an element (or browser_press Tab to focus the first one), then press Enter.",
        )));
    }
    let focused = engine.press(key).map_err(|error| engine_error(&error))?;
    match focused {
        Some((reference, label)) => {
            if let Some(id) = parse_ref(&reference) {
                handle.mutate(|browser| browser.focused = Some(id));
            }
            Ok(focused_payload(&format!("{reference} {label}")))
        }
        None if key == "escape" => {
            handle.mutate(|browser| browser.focused = None);
            Ok(serde_json::json!({
                "focused": serde_json::Value::Null,
                "next": "Nothing is focused. Call browser_click on a field to focus it."
            }))
        }
        None => {
            let before = handle.read(|browser| browser.address.clone());
            settle_live(handle)?;
            let after = handle.read(|browser| browser.address.clone());
            snapshot_payload(
                handle,
                if before == after {
                    "pressed"
                } else {
                    "navigated"
                },
            )
        }
    }
}

/// `browser_back`/`browser_forward` against the live page.
pub(crate) fn history_live(
    handle: &BrowserHandle,
    step: isize,
) -> Result<serde_json::Value, ToolError> {
    let engine = handle
        .engine()
        .ok_or_else(|| ToolError::Failed(String::from("No rendered engine is running.")))?;
    let moved = if step < 0 {
        engine.back()
    } else {
        engine.forward()
    }
    .map_err(|error| engine_error(&error))?;
    if !moved {
        let sentence = if step < 0 {
            "There is no earlier page in this pane's history. Call browser_snapshot to read the current page."
        } else {
            "There is no later page in this pane's history. Call browser_snapshot to read the current page."
        };
        return Err(ToolError::Failed(String::from(sentence)));
    }
    handle
        .mutate(|browser| browser.move_history(step))
        .map_err(ToolError::Failed)?;
    refresh_live(handle)?;
    snapshot_payload(handle, "restored")
}

/// `browser_reload` against the live page.
pub(crate) fn reload_live(
    handle: &BrowserHandle,
    gate: &Arc<EgressGate>,
    mode: NetworkMode,
) -> Result<serde_json::Value, ToolError> {
    let engine = handle
        .engine()
        .ok_or_else(|| ToolError::Failed(String::from("No rendered engine is running.")))?;
    engine
        .reload(gate, mode)
        .map_err(|error| engine_error(&error))?;
    refresh_live(handle)?;
    snapshot_payload(handle, "reloaded")
}

/// `browser_snapshot` against the live page: the text a script wrote is text.
pub(crate) fn snapshot_live(handle: &BrowserHandle) -> Result<serde_json::Value, ToolError> {
    refresh_live(handle)?;
    snapshot_payload(handle, "snapshot")
}

/// `browser_close`: stop the page target and the process behind it.
pub(crate) fn close_live(handle: &BrowserHandle) {
    handle.stop_engine();
}

/// Run a blocking fetch on a runtime thread that may block.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ToolError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ToolError::Failed(format!("the browser task did not finish: {error}")))
}

/// The ref argument every ref-taking tool requires.
fn arg_ref(args: &serde_json::Value) -> Result<String, ToolError> {
    let reference = args
        .get("ref")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if reference.is_empty() {
        return Err(ToolError::InvalidInput(String::from(
            "ref is required. Call browser_snapshot and pass one of its refs, such as e3.",
        )));
    }
    Ok(reference)
}

/// The payload a focus move returns.
fn focused_payload(focused: &str) -> serde_json::Value {
    let reference = focused.split_whitespace().next().unwrap_or(focused);
    let label = focused.strip_prefix(reference).unwrap_or("").trim();
    serde_json::json!({
        "focused": reference,
        "label": label,
        "next": "Call browser_type to fill it, browser_click to activate it, or browser_press Enter."
    })
}
/// Register the browser's tools on an agent.
///
/// Every navigation they perform goes through navigate, which asks the turn's
/// own policy first, so a model can no more reach a host the workspace refuses
/// than the shell tool can.
pub(crate) fn attach(agent: &mut Agent, tools: Tools) {
    let mut add = |tool: AsyncFunctionTool| {
        if let Err(error) = agent.try_add_async_tool(Arc::new(tool)) {
            log::warn!("browser tool was not registered: {error}");
        }
    };

    add(AsyncFunctionTool::from_fn(
        "browser_open",
        "Open an http(s) URL in the built-in Browser pane and return its title, final URL and a structured snapshot of the readable text plus every interactive element. The navigation waits for the page to go network-idle (500 ms with at most two requests in flight, capped by the timeout), so content a late subresource writes is usually already in the returned snapshot. The workspace policy decides reachability BEFORE any request, so a refused URL names the rule that refused it and makes no network request. Only http and https are accepted. The returned snapshot is the same payload browser_snapshot returns, so one call is usually enough.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute http or https URL to open, for example https://example.com/docs."
                }
            },
            "required": ["url"]
        }),
        {
            let tools = tools.clone();
            move |args| {
                let tools = tools.clone();
                async move {
                    let url = args
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if url.is_empty() {
                        return Err(ToolError::InvalidInput(String::from(
                            "url is required. Call browser_open with an absolute http(s) URL.",
                        )));
                    }
                    let gate = tools.gate("browser_open");
                    let handle = tools.browser.clone();
                    let target = url.clone();
                    let mode = tools.network_mode;
                    let loaded =
                        blocking(move || navigate_in(&handle, &gate, &target, mode)).await?;
                    loaded.map_err(|failure| ToolError::Failed(failure.tool_message()))?;
                    tools.snapshot_result("open")
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_snapshot",
        "Return the page on screen as a bounded structured view: title, final URL, readable text capped at 4000 characters (the cap and the total are stated in the payload) and every interactive element with a short stable ref such as e3. Refs stay valid while this document is on screen; any navigation mints new ones, and an old ref then returns an error telling you to snapshot again. When the rendered engine is available this reads the LIVE DOM, so text a page script wrote is included; without it the fetched text is used. Pass html=true only when the markup itself matters.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "html": {
                    "type": "boolean",
                    "description": "Also return the raw HTML, capped at 6000 characters. Off by default: the text and element view is what you act on."
                }
            }
        }),
        {
            let tools = tools.clone();
            move |args| {
                let tools = tools.clone();
                async move {
                    let html = args
                        .get("html")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    if html {
                        tools.html_result()
                    } else if tools.browser.engine_ready() {
                        // The rendered engine re-reads the LIVE DOM here, so a
                        // string a page script wrote is text the model can see.
                        let handle = tools.browser.clone();
                        let mut value = blocking(move || snapshot_live(&handle)).await??;
                        tools.stamp(&mut value);
                        Ok(value)
                    } else {
                        tools.snapshot_result("snapshot")
                    }
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_click",
        "Activate an element by the ref its snapshot gave you. A link navigates and the response carries the new snapshot, so no follow-up call is needed. An input, textarea or select takes focus for browser_type. A submit control submits its form with GET. A ref that is no longer on screen returns an error naming the page change and asking for a fresh snapshot - it never silently succeeds. With the rendered engine available the click is a real input event, so a control that acts through page JavaScript also works; the text view refuses one and says so. The result's via field says which activation ran: 'pointer' is a synthesized mouse event at a real box, and 'script' is the DOM's own click(), used only when the target genuinely has no box to hit.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "ref": {
                    "type": "string",
                    "description": "Element ref from the current snapshot, for example e12."
                }
            },
            "required": ["ref"]
        }),
        {
            let tools = tools.clone();
            move |args| {
                let tools = tools.clone();
                async move {
                    let reference = arg_ref(&args)?;
                    if tools.browser.engine_ready() {
                        let handle = tools.browser.clone();
                        let target = reference.clone();
                        let mut value = blocking(move || click_live(&handle, &target)).await??;
                        tools.stamp(&mut value);
                        return Ok(value);
                    }
                    let act = tools
                        .browser
                        .mutate(|browser| browser.activate(&reference))
                        .map_err(ToolError::Failed)?;
                    match act {
                        Action::Report(mut value) => {
                            tools.stamp(&mut value);
                            Ok(value)
                        }
                        Action::Navigate(url) => {
                            let gate = tools.gate("browser_click");
                            let handle = tools.browser.clone();
                            let target = url.clone();
                            let mode = tools.network_mode;
                            let loaded =
                                blocking(move || navigate_in(&handle, &gate, &target, mode)).await?;
                            loaded.map_err(|failure| ToolError::Failed(failure.tool_message()))?;
                            tools.snapshot_result("navigated")
                        }
                    }
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_type",
        "Replace the value of an input, textarea or select named by a ref. For a select, the text picks the option whose label matches exactly or by prefix, case-insensitively. Typing does not submit: call browser_press with Enter when the field is inside a form, or browser_click the form's submit control. The response echoes what was stored. A password field accepts text but never returns it.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "ref": {
                    "type": "string",
                    "description": "Element ref of a field from the current snapshot."
                },
                "text": {
                    "type": "string",
                    "description": "The complete value to store, not an increment."
                }
            },
            "required": ["ref", "text"]
        }),
        {
            let tools = tools.clone();
            move |args| {
                let tools = tools.clone();
                async move {
                    let reference = arg_ref(&args)?;
                    let text = args
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if tools.browser.engine_ready() {
                        let handle = tools.browser.clone();
                        let target = reference.clone();
                        let mut value =
                            blocking(move || type_live(&handle, &target, &text)).await??;
                        tools.stamp(&mut value);
                        return Ok(value);
                    }
                    let mut value = tools
                        .browser
                        .mutate(|browser| browser.type_into(&reference, &text))
                        .map_err(ToolError::Failed)?;
                    tools.stamp(&mut value);
                    Ok(value)
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_press",
        "Press a key on the focused element. Enter activates it: a link navigates, a submit control submits its form, and an input or textarea inside a GET form submits that form with its current values. Tab and ArrowDown focus the next element; Shift+Tab and ArrowUp focus the previous one; Escape clears the focus. With the rendered engine available the page itself receives the key, and Backspace, Delete, Home, End, PageUp, PageDown and the left/right arrows edit and move within a field too. Any other key is refused with the list this view supports.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "One of Enter, Tab, Shift+Tab, ArrowDown, ArrowUp, Escape."
                }
            },
            "required": ["key"]
        }),
        {
            let tools = tools.clone();
            move |args| {
                let tools = tools.clone();
                async move {
                    let key = args
                        .get("key")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    if tools.browser.engine_ready() {
                        let handle = tools.browser.clone();
                        let target = key.clone();
                        let mut value = blocking(move || press_live(&handle, &target)).await??;
                        tools.stamp(&mut value);
                        return Ok(value);
                    }
                    let act = tools
                        .browser
                        .mutate(|browser| browser.press_key(&key))
                        .map_err(ToolError::Failed)?;
                    match act {
                        Action::Report(mut value) => {
                            tools.stamp(&mut value);
                            Ok(value)
                        }
                        Action::Navigate(url) => {
                            let gate = tools.gate("browser_press");
                            let handle = tools.browser.clone();
                            let target = url.clone();
                            let mode = tools.network_mode;
                            let loaded =
                                blocking(move || navigate_in(&handle, &gate, &target, mode)).await?;
                            loaded.map_err(|failure| ToolError::Failed(failure.tool_message()))?;
                            tools.snapshot_result("navigated")
                        }
                    }
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_back",
        "Go back one document in the Browser pane's history. With the rendered engine available it moves through the page's OWN history; the text view restores its cached copy. Refs are re-minted when a new document appears: take a fresh snapshot before acting. Returns the restored snapshot.",
        serde_json::json!({ "type": "object", "properties": {} }),
        {
            let tools = tools.clone();
            move |_args| {
                let tools = tools.clone();
                async move {
                    if tools.browser.engine_ready() {
                        let handle = tools.browser.clone();
                        let mut value = blocking(move || history_live(&handle, -1)).await??;
                        tools.stamp(&mut value);
                        return Ok(value);
                    }
                    tools
                        .browser
                        .mutate(|browser| browser.back())
                        .map_err(ToolError::Failed)?;
                    tools.snapshot_result("restored")
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_forward",
        "Go forward one document in the Browser pane's history, undoing a browser_back. With the rendered engine available it moves through the page's OWN history; the text view restores its cached copy. Refs are re-minted when a new document appears: take a fresh snapshot before acting.",
        serde_json::json!({ "type": "object", "properties": {} }),
        {
            let tools = tools.clone();
            move |_args| {
                let tools = tools.clone();
                async move {
                    if tools.browser.engine_ready() {
                        let handle = tools.browser.clone();
                        let mut value = blocking(move || history_live(&handle, 1)).await??;
                        tools.stamp(&mut value);
                        return Ok(value);
                    }
                    tools
                        .browser
                        .mutate(|browser| browser.forward())
                        .map_err(ToolError::Failed)?;
                    tools.snapshot_result("restored")
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_reload",
        "Fetch the page on screen again, as a NEW policy decision and a new request. Use it when the page is expected to have changed. Refs are re-minted.",
        serde_json::json!({ "type": "object", "properties": {} }),
        {
            let tools = tools.clone();
            move |_args| {
                let tools = tools.clone();
                async move {
                    if tools.browser.engine_ready() {
                        let handle = tools.browser.clone();
                        let gate = tools.gate("browser_reload");
                        let mode = tools.network_mode;
                        let mut value =
                            blocking(move || reload_live(&handle, &gate, mode)).await??;
                        tools.stamp(&mut value);
                        return Ok(value);
                    }
                    let url = tools
                        .browser
                        .read(|browser| browser.document().map(|doc| doc.url.clone()))
                        .ok_or_else(|| {
                            ToolError::Failed(String::from(
                                "There is no page to reload. Call browser_open with a URL.",
                            ))
                        })?;
                    let gate = tools.gate("browser_reload");
                    let handle = tools.browser.clone();
                    let target = url.clone();
                    let mode = tools.network_mode;
                    let loaded =
                        blocking(move || navigate_in(&handle, &gate, &target, mode)).await?;
                    loaded.map_err(|failure| ToolError::Failed(failure.tool_message()))?;
                    tools.snapshot_result("reloaded")
                }
            }
        },
    ));

    add(AsyncFunctionTool::from_fn(
        "browser_close",
        "Close the browser: drop the document, its history and the pane's page, returning to the empty state. A later browser_open starts a fresh history. Use it when a browsing task is finished, so the next task cannot mistake an old page for the current one.",
        serde_json::json!({ "type": "object", "properties": {} }),
        {
            let tools = tools.clone();
            move |_args| {
                let tools = tools.clone();
                async move {
                    // The page target and the process go with the document: an
                    // engine nobody can see is a process the reader never asked
                    // for.
                    close_live(&tools.browser);
                    tools.browser.mutate(|browser| browser.close());
                    let mut value = serde_json::json!({
                        "closed": true,
                        "next": "The pane is empty. Call browser_open with a URL to start again."
                    });
                    tools.stamp(&mut value);
                    Ok(value)
                }
            }
        },
    ));
}

/// What activating or pressing on a ref decided to do.
///
/// The decision and the request are separate because the fetch is blocking: the
/// browser is locked only long enough to decide, then the caller performs the
/// navigation outside the lock.
#[derive(Debug)]
pub(crate) enum Action {
    /// The activation opens this URL; the caller fetches it.
    Navigate(String),
    /// The activation changed the document in place.
    Report(serde_json::Value),
}

impl Browser {
    /// Activate the element a ref names.
    ///
    /// A link or a form control answers with the URL to fetch; a field takes
    /// focus and answers with what to do next. Anything the view cannot do -
    /// a link with no target, a button outside a form, a script-only control -
    /// answers with a sentence naming the reason and the next step.
    pub(crate) fn activate(&mut self, reference: &str) -> Result<Action, String> {
        let id = self.resolve(reference)?;
        let element = self
            .doc
            .as_ref()
            .and_then(|doc| doc.element(id))
            .cloned()
            .ok_or_else(|| format!("{reference} is not on this page. Call browser_snapshot."))?;
        match element.kind {
            Kind::Link => {
                let href = element.href.clone().ok_or_else(|| {
                    format!(
                        "{} is a link with no target this view can open. Call browser_snapshot.",
                        element.reference()
                    )
                })?;
                Ok(Action::Navigate(href))
            }
            Kind::Button | Kind::Submit => Ok(Action::Navigate(submit_url(self, &element)?)),
            Kind::Input | Kind::Textarea | Kind::Select => {
                self.focused = Some(id);
                Ok(Action::Report(serde_json::json!({
                    "focused": element.reference(),
                    "kind": element.kind.as_str(),
                    "label": element.label,
                    "next": "Call browser_type with this ref and the text to enter, then browser_press Enter if it is inside a form."
                })))
            }
        }
    }
}

impl Browser {
    /// Replace the value of the field a ref names.
    ///
    /// A select takes the option whose label matches exactly or by prefix; the
    /// failure lists the options so the caller does not have to guess. A
    /// password field stores the text and never returns it.
    pub(crate) fn type_into(
        &mut self,
        reference: &str,
        text: &str,
    ) -> Result<serde_json::Value, String> {
        let id = self.resolve(reference)?;
        let doc = self
            .doc
            .as_mut()
            .ok_or_else(|| String::from("No page is open. Call browser_open first."))?;
        let element = doc
            .element_mut(id)
            .ok_or_else(|| format!("{reference} is not on this page. Call browser_snapshot."))?;
        if !element.kind.takes_text() {
            return Err(format!(
                "{} is a {}, which holds no value. Call browser_click to activate it, or browser_snapshot to pick a field.",
                element.reference(),
                element.kind.as_str()
            ));
        }
        if element.kind == Kind::Select {
            let wanted = text.trim().to_ascii_lowercase();
            let chosen = element
                .options
                .iter()
                .find(|option| option.trim().eq_ignore_ascii_case(text.trim()))
                .or_else(|| {
                    element
                        .options
                        .iter()
                        .find(|option| option.trim().to_ascii_lowercase().starts_with(&wanted))
                })
                .cloned();
            let Some(chosen) = chosen else {
                let shown: Vec<String> = element.options.iter().take(8).cloned().collect();
                return Err(format!(
                    "'{}' is not an option of {}. Its options are {:?}; call browser_type with one of them.",
                    text,
                    element.reference(),
                    shown
                ));
            };
            element.value = Some(chosen.clone());
            return Ok(serde_json::json!({
                "ref": element.reference(),
                "kind": "select",
                "value": chosen,
            }));
        }
        element.value = Some(text.to_string());
        let mut payload = serde_json::json!({
            "ref": element.reference(),
            "kind": element.kind.as_str(),
            "value": element.wire_value(),
        });
        if element.secret() {
            payload["hidden"] = serde_json::json!(true);
            payload["note"] = serde_json::json!(
                "The field is a password: its value is stored but never returned, so you cannot read a secret back out of the page."
            );
        }
        Ok(payload)
    }
}

impl Browser {
    /// Press one key against the focused element.
    pub(crate) fn press_key(&mut self, key: &str) -> Result<Action, String> {
        match key {
            "tab" | "arrowdown" => {
                let focused = self.shift_focus(1)?;
                Ok(Action::Report(focused_payload(&focused)))
            }
            "shift+tab" | "arrowup" => {
                let focused = self.shift_focus(-1)?;
                Ok(Action::Report(focused_payload(&focused)))
            }
            "escape" => {
                self.focused = None;
                Ok(Action::Report(serde_json::json!({
                    "focused": serde_json::Value::Null,
                    "next": "Nothing is focused. Call browser_click on a field to focus it."
                })))
            }
            "enter" => {
                let Some(element) = self.focused_element().cloned() else {
                    return Err(String::from(
                        "No element is focused, so Enter has nothing to activate. Call browser_click on an element (or browser_press Tab to focus the first one), then press Enter.",
                    ));
                };
                match element.kind {
                    Kind::Link => {
                        let href = element.href.clone().ok_or_else(|| {
                            format!(
                                "{} is a link with no target this view can open. Call browser_snapshot.",
                                element.reference()
                            )
                        })?;
                        Ok(Action::Navigate(href))
                    }
                    Kind::Button | Kind::Submit => {
                        Ok(Action::Navigate(submit_url(self, &element)?))
                    }
                    Kind::Input | Kind::Textarea => {
                        let url = submit_url(self, &element).map_err(|_| {
                            format!(
                                "{} is not inside a form, so Enter has nowhere to go: this view does not run page scripts. Call browser_snapshot and follow a link instead.",
                                element.reference()
                            )
                        })?;
                        Ok(Action::Navigate(url))
                    }
                    Kind::Select => Err(format!(
                        "{} is a select, which Enter cannot activate here. Call browser_type with the option you want.",
                        element.reference()
                    )),
                }
            }
            other => Err(format!(
                "{other} is not a key this view supports. Use Enter (activate the focused element), Tab or ArrowDown (next element), Shift+Tab or ArrowUp (previous element) or Escape (clear focus)."
            )),
        }
    }
}

/// Build the URL a GET submission of the element's form would request.
///
/// Only GET is buildable in this view: a POST body would be a request this
/// client has no page-level permission to invent, so a POST form refuses rather
/// than silently becoming a GET.
fn submit_url(browser: &Browser, element: &Element) -> Result<String, String> {
    let doc = browser
        .doc
        .as_ref()
        .ok_or_else(|| String::from("No page is open. Call browser_open first."))?;
    let Some(index) = element.form else {
        return Err(format!(
            "{} is not inside a form, so it has no target to submit. Call browser_snapshot and follow a link instead.",
            element.reference()
        ));
    };
    let Some(form) = doc.forms.get(index) else {
        return Err(format!(
            "{} names a form this view did not keep. Call browser_snapshot and try again.",
            element.reference()
        ));
    };
    if !form.method.is_empty() && form.method != "get" {
        return Err(format!(
            "That form submits with {}, which this view cannot do: it sends a body, and the browser here is a GET view. Call browser_snapshot to read the page instead.",
            form.method.to_uppercase()
        ));
    }
    let base = if form.action.is_empty() {
        doc.url.clone()
    } else {
        form.action.clone()
    };
    let mut pairs: Vec<String> = Vec::new();
    for field in &doc.elements {
        if field.form != Some(index) {
            continue;
        }
        if matches!(field.kind, Kind::Input | Kind::Textarea | Kind::Select) {
            let Some(name) = &field.name else { continue };
            let value = field.value.clone().unwrap_or_default();
            pairs.push(format!("{}={}", encode_query(name), encode_query(&value)));
        }
    }
    if pairs.is_empty() {
        return Ok(base);
    }
    let separator = if base.contains('?') { '&' } else { '?' };
    Ok(format!("{base}{separator}{}", pairs.join("&")))
}

/// Percent-encode one query component, keeping the characters a query allows.
fn encode_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Resolve an href against the page it came from.
fn resolve_href(base: &str, href: &str) -> String {
    let href = href.trim();
    if let Some(fragment) = href.strip_prefix('#') {
        let without = base.split('#').next().unwrap_or(base);
        return format!("{without}#{fragment}");
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return href.to_string();
    }
    // Any other scheme is kept verbatim: opening it refuses with the scheme
    // rule, which is a better answer than a URL this module fabricated.
    if let Some(colon) = href.find(':') {
        let scheme = &href[..colon];
        if !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return href.to_string();
        }
    }
    agent::policy::join(base, href)
}

/// Everything the pane draws, read once per change.
///
/// The pane holds this instead of the lock: a document's text is up to 4000
/// characters, and cloning it per frame would put an allocation on the render
/// path for a page that did not change. It is rebuilt only when the browser's
/// revision moves.
#[derive(Debug, Clone, Default)]
pub(crate) struct PaneView {
    /// Empty, loading, ready or failed.
    pub(crate) status: Status,
    /// The address bar's value.
    pub(crate) url: String,
    /// The last failure, when the pane owes the reader an explanation.
    pub(crate) failure: Option<Failure>,
    /// The page, when one is on screen.
    pub(crate) snapshot: Option<Snapshot>,
    /// Whether back is possible.
    pub(crate) can_back: bool,
    /// Whether forward is possible.
    pub(crate) can_forward: bool,
}

impl Browser {
    /// The pane's view of the browser.
    pub(crate) fn pane_view(&self) -> PaneView {
        PaneView {
            status: self.status(),
            url: self.address().to_string(),
            failure: self.failure().cloned(),
            snapshot: self.snapshot(),
            can_back: self.can_back(),
            can_forward: self.can_forward(),
        }
    }
}

/// Parse a fetched body into a document.
///
/// This is a bounded, forgiving HTML reader: it walks tags, keeps the readable
/// text and records the elements a reader can act on. It is not a DOM and does
/// not pretend to be - an unclosed tag costs a paragraph break, never a page.
pub(crate) fn parse_document(url: &str, html: &str, next_id: &mut u64) -> Document {
    let max_id = next_id.saturating_sub(1);
    let mut parser = Parser {
        html,
        pos: 0,
        url,
        next_id,
        doc: Document {
            url: url.to_string(),
            title: String::new(),
            text: String::new(),
            text_total_chars: 0,
            elements: Vec::new(),
            forms: Vec::new(),
            raw: String::new(),
            status: 200,
            body_truncated: false,
            max_id,
        },
        main: String::new(),
        main_chars: 0,
        in_title: false,
        title: String::new(),
        skip_depth: 0,
        main_depth: 0,
        main_seen: false,
        capture: None,
        form_stack: Vec::new(),
        in_option: false,
        option_text: String::new(),
        option_value: None,
        total_chars: 0,
        dropped_chars: 0,
    };
    parser.run();
    parser.finish()
}

/// One open element whose text is still being read.
struct Capture {
    index: usize,
    tag: String,
}

/// One parsed tag.
struct Tag {
    name: String,
    closing: bool,
    self_closing: bool,
    attrs: Vec<(String, String)>,
    end: usize,
}

impl Tag {
    /// One attribute, by its lower-cased name.
    fn attr(&self, key: &str) -> Option<String> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }
}

/// The extraction state.
struct Parser<'a> {
    html: &'a str,
    pos: usize,
    url: &'a str,
    next_id: &'a mut u64,
    doc: Document,
    main: String,
    main_chars: usize,
    in_title: bool,
    title: String,
    skip_depth: usize,
    main_depth: usize,
    main_seen: bool,
    capture: Option<Capture>,
    form_stack: Vec<usize>,
    in_option: bool,
    option_text: String,
    option_value: Option<String>,
    total_chars: usize,
    /// Characters the store bound could not keep.
    dropped_chars: usize,
}

/// Tags whose content is not readable text.
const SKIP: [&str; 6] = ["script", "style", "noscript", "template", "svg", "iframe"];
/// Tags that end a line of readable text.
const BLOCK: [&str; 22] = [
    "p", "div", "br", "li", "ul", "ol", "tr", "td", "th", "table", "section", "article", "header",
    "footer", "nav", "main", "h1", "h2", "h3", "h4", "h5", "h6",
];

impl<'a> Parser<'a> {
    /// Walk the whole body.
    fn run(&mut self) {
        let html = self.html;
        while self.pos < html.len() {
            let Some(offset) = html[self.pos..].find('<') else {
                let rest = &html[self.pos..];
                self.text(rest);
                self.pos = html.len();
                break;
            };
            let start = self.pos + offset;
            if start > self.pos {
                let chunk = &html[self.pos..start];
                self.text(chunk);
            }
            self.pos = start;
            if html[self.pos..].starts_with("<!--") {
                match html[self.pos + 4..].find("-->") {
                    Some(end) => self.pos += 4 + end + 3,
                    None => self.pos = html.len(),
                }
                continue;
            }
            if html[self.pos..].starts_with("<!") || html[self.pos..].starts_with("<?") {
                match html[self.pos..].find('>') {
                    Some(end) => self.pos += end + 1,
                    None => self.pos = html.len(),
                }
                continue;
            }
            let Some(tag) = self.tag() else {
                // A bare '<' that begins no tag is text.
                self.text("<");
                self.pos += 1;
                continue;
            };
            self.handle(tag);
        }
    }

    /// Read the tag at the cursor.
    fn tag(&mut self) -> Option<Tag> {
        let html = self.html;
        let mut cursor = self.pos + 1;
        let closing = html[cursor..].starts_with('/');
        if closing {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < html.len() {
            let byte = html.as_bytes()[cursor];
            if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b':' {
                cursor += 1;
            } else {
                break;
            }
        }
        if cursor == name_start {
            return None;
        }
        let name = html[name_start..cursor].to_ascii_lowercase();
        let mut tag = Tag {
            name,
            closing,
            self_closing: false,
            attrs: Vec::new(),
            end: html.len(),
        };
        // Attributes, with quoted values that may contain '>'.
        loop {
            while cursor < html.len() && html.as_bytes()[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor >= html.len() {
                tag.end = html.len();
                break;
            }
            match html.as_bytes()[cursor] {
                b'>' => {
                    tag.end = cursor + 1;
                    break;
                }
                b'/' => {
                    tag.self_closing = true;
                    cursor += 1;
                }
                _ => {
                    let attr_start = cursor;
                    while cursor < html.len() {
                        let byte = html.as_bytes()[cursor];
                        if byte.is_ascii_whitespace()
                            || byte == b'='
                            || byte == b'>'
                            || byte == b'/'
                        {
                            break;
                        }
                        cursor += 1;
                    }
                    let key = html[attr_start..cursor].to_ascii_lowercase();
                    while cursor < html.len() && html.as_bytes()[cursor].is_ascii_whitespace() {
                        cursor += 1;
                    }
                    let mut value = String::new();
                    if cursor < html.len() && html.as_bytes()[cursor] == b'=' {
                        cursor += 1;
                        while cursor < html.len() && html.as_bytes()[cursor].is_ascii_whitespace() {
                            cursor += 1;
                        }
                        if cursor < html.len() {
                            let quote = html.as_bytes()[cursor];
                            if quote == b'"' || quote == b'\'' {
                                cursor += 1;
                                let value_start = cursor;
                                while cursor < html.len() && html.as_bytes()[cursor] != quote {
                                    cursor += 1;
                                }
                                value = html[value_start..cursor].to_string();
                                if cursor < html.len() {
                                    cursor += 1;
                                }
                            } else {
                                let value_start = cursor;
                                while cursor < html.len() {
                                    let byte = html.as_bytes()[cursor];
                                    if byte.is_ascii_whitespace() || byte == b'>' {
                                        break;
                                    }
                                    cursor += 1;
                                }
                                value = html[value_start..cursor].to_string();
                            }
                        }
                    }
                    if !key.is_empty() {
                        tag.attrs.push((key, decode_entities(&value)));
                    }
                }
            }
        }
        self.pos = tag.end.min(html.len());
        Some(tag)
    }

    /// Route one tag.
    fn handle(&mut self, tag: Tag) {
        if tag.closing {
            let name = tag.name.clone();
            self.close_tag(&name);
            return;
        }
        self.open_tag(&tag);
    }

    /// A closing tag.
    fn close_tag(&mut self, name: &str) {
        if let Some(capture) = &self.capture {
            if capture.tag == name {
                self.capture = None;
            }
        }
        match name {
            "title" => self.in_title = false,
            "option" => self.finish_option(),
            "form" => {
                self.form_stack.pop();
            }
            _ => {}
        }
        if SKIP.contains(&name) {
            self.skip_depth = self.skip_depth.saturating_sub(1);
        }
        if name == "main" || name == "article" {
            self.main_depth = self.main_depth.saturating_sub(1);
        }
        if BLOCK.contains(&name) {
            self.push_newline();
        }
    }

    /// An opening tag.
    fn open_tag(&mut self, tag: &Tag) {
        let name = tag.name.as_str();
        if BLOCK.contains(&name) {
            self.push_newline();
        }
        if name == "title" {
            self.in_title = !tag.self_closing;
            return;
        }
        if SKIP.contains(&name) {
            if !tag.self_closing {
                self.skip_depth += 1;
            }
            return;
        }
        if self.skip_depth > 0 {
            return;
        }
        if name == "main" || name == "article" {
            if self.main_depth == 0 {
                self.main_seen = true;
            }
            self.main_depth += 1;
        }
        if name == "form" {
            let action = tag
                .attr("action")
                .map(|action| resolve_href(self.url, &action))
                .unwrap_or_default();
            let method = tag
                .attr("method")
                .map(|method| method.trim().to_ascii_lowercase())
                .unwrap_or_else(|| String::from("get"));
            self.doc.forms.push(Form { action, method });
            self.form_stack.push(self.doc.forms.len() - 1);
        }
        match name {
            "a" => {
                if let Some(href) = tag.attr("href") {
                    let href = href.trim().to_string();
                    let scripted = href.to_ascii_lowercase().starts_with("javascript:");
                    if !href.is_empty() && !scripted {
                        let resolved = resolve_href(self.url, &href);
                        let fallback = tag
                            .attr("aria-label")
                            .or_else(|| tag.attr("title"))
                            .unwrap_or_default();
                        let index = self.push_element(Kind::Link, fallback);
                        self.doc.elements[index].href = Some(resolved);
                        if !tag.self_closing {
                            self.capture = Some(Capture {
                                index,
                                tag: String::from(name),
                            });
                        }
                    }
                }
            }
            "button" => {
                let submit = tag.attr("type").is_none_or(|kind| {
                    let kind = kind.trim().to_ascii_lowercase();
                    kind.is_empty() || kind == "submit"
                });
                let kind = if submit { Kind::Submit } else { Kind::Button };
                let fallback = tag.attr("aria-label").unwrap_or_default();
                let index = self.push_element(kind, fallback);
                self.doc.elements[index].form = self.form_stack.last().copied();
                if !tag.self_closing {
                    self.capture = Some(Capture {
                        index,
                        tag: String::from(name),
                    });
                }
            }

            "input" => {
                let input_type = tag
                    .attr("type")
                    .map(|kind| kind.trim().to_ascii_lowercase())
                    .unwrap_or_else(|| String::from("text"));
                if input_type == "hidden" {
                    return;
                }
                let kind = if input_type == "submit" || input_type == "button" {
                    Kind::Submit
                } else {
                    Kind::Input
                };
                let label = tag
                    .attr("aria-label")
                    .or_else(|| tag.attr("placeholder"))
                    .or_else(|| tag.attr("name"))
                    .or_else(|| tag.attr("title"))
                    .unwrap_or_else(|| input_type.clone());
                let index = self.push_element(kind, label);
                let form = self.form_stack.last().copied();
                let element = &mut self.doc.elements[index];
                element.input_type = Some(input_type);
                element.name = tag.attr("name");
                element.form = form;
                element.value = tag.attr("value");
            }
            "textarea" => {
                let label = tag
                    .attr("aria-label")
                    .or_else(|| tag.attr("placeholder"))
                    .or_else(|| tag.attr("name"))
                    .unwrap_or_else(|| String::from("textarea"));
                let index = self.push_element(Kind::Textarea, label);
                let form = self.form_stack.last().copied();
                let element = &mut self.doc.elements[index];
                element.name = tag.attr("name");
                element.form = form;
                element.value = Some(String::new());
                if !tag.self_closing {
                    self.capture = Some(Capture {
                        index,
                        tag: String::from(name),
                    });
                }
            }

            "select" => {
                let label = tag
                    .attr("aria-label")
                    .or_else(|| tag.attr("name"))
                    .or_else(|| tag.attr("title"))
                    .unwrap_or_else(|| String::from("select"));
                let index = self.push_element(Kind::Select, label);
                let form = self.form_stack.last().copied();
                let element = &mut self.doc.elements[index];
                element.name = tag.attr("name");
                element.form = form;
                element.value = Some(String::new());
                if !tag.self_closing {
                    self.capture = Some(Capture {
                        index,
                        tag: String::from(name),
                    });
                }
            }
            // An option's label belongs to its select, so it only matters
            // while one is open.
            "option" if self.enclosing_select().is_some() => {
                self.in_option = true;
                self.option_text.clear();
                self.option_value = tag.attr("value");
                if tag.self_closing {
                    self.finish_option();
                }
            }
            _ => {}
        }
    }

    /// The select an open option belongs to.
    fn enclosing_select(&self) -> Option<usize> {
        self.capture
            .as_ref()
            .filter(|capture| capture.tag == "select")
            .map(|capture| capture.index)
    }

    /// Close the open option into its select.
    fn finish_option(&mut self) {
        if !self.in_option {
            return;
        }
        self.in_option = false;
        let text = collapse(&self.option_text);
        let value = self
            .option_value
            .take()
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        let Some(index) = self.enclosing_select() else {
            return;
        };
        let element = &mut self.doc.elements[index];
        if !text.is_empty() {
            element.options.push(text.clone());
        }
        let chosen = if text.is_empty() { value } else { text };
        let blank = element
            .value
            .as_deref()
            .is_some_and(|current| current.is_empty());
        if blank && !chosen.is_empty() {
            element.value = Some(chosen);
        }
    }

    /// Start an element and return its index.
    fn push_element(&mut self, kind: Kind, label: String) -> usize {
        let id = *self.next_id;
        *self.next_id += 1;
        self.doc.elements.push(Element {
            id,
            kind,
            label: collapse(&decode_entities(&label)),
            href: None,
            value: None,
            options: Vec::new(),
            name: None,
            form: None,
            input_type: None,
        });
        self.doc.max_id = id;
        self.doc.elements.len() - 1
    }
    /// Append text to whichever buffers are open.
    fn text(&mut self, raw: &str) {
        if self.in_title {
            self.title.push_str(raw);
            return;
        }
        if self.skip_depth > 0 {
            return;
        }
        let decoded = decode_entities(raw);
        if self.in_option {
            self.option_text.push_str(&decoded);
        } else if let Some(capture) = &self.capture {
            let index = capture.index;
            let tag = capture.tag.clone();
            if tag != "select" {
                if let Some(element) = self.doc.elements.get_mut(index) {
                    if element.kind == Kind::Textarea {
                        if let Some(value) = element.value.as_mut() {
                            value.push_str(&decoded);
                        }
                    } else {
                        element.label.push_str(&decoded);
                    }
                }
            }
        }
        self.push_text(&decoded);
    }

    /// Append to the readable text buffers.
    fn push_text(&mut self, text: &str) {
        let chars = text.chars().count();
        self.total_chars = self.total_chars.saturating_add(chars);
        // A single chunk can be larger than the store bound (a page with no
        // tags in its body), so the stored copy takes the part that fits
        // instead of being dropped whole. What did not fit is counted, so the
        // snapshot can still say how much text the page held.
        let room = STORE_TEXT_CAP.saturating_sub(self.doc.text.chars().count());
        let kept = chars.min(room);
        if kept > 0 {
            self.doc.text.extend(text.chars().take(kept));
        }
        self.dropped_chars = self.dropped_chars.saturating_add(chars - kept);
        if self.main_depth > 0 {
            self.main_chars = self.main_chars.saturating_add(chars);
            let room = STORE_TEXT_CAP.saturating_sub(self.main.chars().count());
            let kept = chars.min(room);
            if kept > 0 {
                self.main.extend(text.chars().take(kept));
            }
        }
    }

    /// A paragraph break in the readable text.
    fn push_newline(&mut self) {
        self.push_text("\n");
    }

    /// Turn the parser state into the finished document.
    fn finish(mut self) -> Document {
        let (source, total) = if self.main_seen {
            (self.main.clone(), self.main_chars)
        } else {
            (self.doc.text.clone(), self.total_chars)
        };
        let text = normalize(&source);
        // The characters the page held: the normalized text plus whatever the
        // store bound had to drop. Exact for every page that fits the store.
        let held = text.chars().count().max(total.min(text.chars().count()));
        self.doc.text_total_chars = held.saturating_add(self.dropped_chars);
        self.doc.text = text.chars().take(STORE_TEXT_CAP).collect();
        self.doc.title = collapse(&decode_entities(&self.title));
        // A label that only collected whitespace falls back to something the
        // model can act on.
        for element in &mut self.doc.elements {
            if element.label.trim().is_empty() {
                element.label = match (&element.kind, &element.href) {
                    (Kind::Link, Some(href)) => href.clone(),
                    (kind, _) => kind.as_str().to_string(),
                };
            }
        }
        self.doc.raw = String::new();
        self.doc
    }
}

/// Collapse every whitespace run in one line to a single space.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Turn accumulated text into paragraphs of collapsed lines.
fn normalize(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            let line = collapse(&current);
            if !line.is_empty() {
                lines.push(line);
            }
            current.clear();
        } else {
            current.push(ch);
        }
    }
    let line = collapse(&current);
    if !line.is_empty() {
        lines.push(line);
    }
    lines.join("\n")
}

/// Decode the entities a page may put in text or in an attribute.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        let Some(end) = after.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('-'),
            "ndash" => Some('-'),
            "hellip" => Some('…'),
            _ => {
                let numeric = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| {
                        entity
                            .strip_prefix('#')
                            .and_then(|digits| digits.parse::<u32>().ok())
                    });
                numeric.and_then(char::from_u32)
            }
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    //! Probes for the browser, not for strings: every one of them fetches from a
    //! real HTTP listener on 127.0.0.1, so the page's title, text and elements
    //! are the ones a server actually sent.

    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    /// The network-mode vocabulary is exactly two values, and anything a future
    /// build might write reads as the SAFE default (Proxied, which still asks
    /// the policy at host:port) rather than as a silent widening.
    #[test]
    fn the_browser_network_mode_vocabulary_has_a_safe_default() {
        assert_eq!(
            NetworkMode::from_setting("strict"),
            NetworkMode::Strict,
            "the stored spelling must round-trip"
        );
        assert_eq!(
            NetworkMode::from_setting("  STRICT "),
            NetworkMode::Strict,
            "the value is compared case-insensitively and trimmed"
        );
        assert_eq!(NetworkMode::from_setting("proxied"), NetworkMode::Proxied);
        assert_eq!(
            NetworkMode::from_setting("something-new"),
            NetworkMode::Proxied,
            "an unknown value must not become Strict or anything else"
        );
        assert_eq!(NetworkMode::default(), NetworkMode::Proxied);
        assert_eq!(NetworkMode::Proxied.as_str(), "proxied");
        assert_eq!(NetworkMode::Strict.as_str(), "strict");
        assert_eq!(
            crate::settings::Settings::default()
                .global
                .general
                .browser_network_mode,
            "proxied",
            "a fresh install ships the Proxied default"
        );
        assert_eq!(
            NetworkMode::Proxied.label_key(),
            "right.browser.networkProxied"
        );
        assert_eq!(
            NetworkMode::Strict.label_key(),
            "right.browser.networkStrict"
        );
    }

    /// One local site, counting the requests that reached it.
    struct Site {
        addr: std::net::SocketAddr,
        hits: std::sync::Arc<AtomicUsize>,
        stop: std::sync::Arc<AtomicBool>,
    }

    impl Site {
        /// The URL of one path on this site.
        fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
        }

        /// How many requests arrived.
        fn hits(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for Site {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            // Wake the accept loop so its thread can end.
            let _ = std::net::TcpStream::connect(self.addr);
        }
    }

    /// The page each path serves.
    fn body_for(path: &str) -> String {
        match path {
            "/" => String::from(
                "<html><head><title>Test Page</title></head><body>\
                 <h1>Hello</h1><p>hello from the test server</p>\
                 <a href=\"/second\">Go to second</a>\
                 <form action=\"/search\" method=\"get\">\
                 <input type=\"search\" name=\"q\" placeholder=\"Search the site\">\
                 <button type=\"submit\">Find</button></form>\
                 </body></html>",
            ),
            "/second" => String::from(
                "<html><head><title>Second Page</title></head><body>second body</body></html>",
            ),
            "/search" => String::from(
                "<html><head><title>Search Results</title></head><body>results</body></html>",
            ),
            "/long" => {
                let word = "lorem ipsum dolor sit amet ";
                format!(
                    "<html><head><title>Long Page</title></head><body>{}</body></html>",
                    word.repeat(3_000)
                )
            }
            _ => String::from("<html><head><title>Not Found</title></head><body>no</body></html>"),
        }
    }

    /// Start a site on a loopback port.
    fn site() -> Site {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let addr = listener.local_addr().expect("the port");
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let counter = hits.clone();
        let flag = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .to_string();
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = body_for(&path);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Site { addr, hits, stop }
    }

    /// A gate over a policy that allows exactly one address.
    fn gate(allowed: &[&str], tool: &'static str) -> Arc<EgressGate> {
        let policy = Arc::new(PolicySet::compile_baseline(
            agent::policy::PolicyData::new("/tmp")
                .with_egress_allowlist(allowed.iter().map(|entry| entry.to_string())),
        ));
        Arc::new(EgressGate::new(
            policy,
            Arc::new(SystemResolver),
            "session-test",
            "preset-test",
            tool,
        ))
    }

    /// A browser with nothing loaded.
    fn browser() -> BrowserHandle {
        BrowserHandle::new()
    }

    /// The elements of one kind on the current page.
    fn refs(handle: &BrowserHandle, kind: &str) -> Vec<String> {
        handle.read(|browser| {
            browser
                .snapshot()
                .map(|snapshot| {
                    snapshot
                        .elements
                        .into_iter()
                        .filter(|element| element.kind == kind)
                        .map(|element| element.reference)
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// A ref taken from the snapshot clicks the element it named.
    #[test]
    fn a_ref_from_the_snapshot_clicks_the_right_element() {
        let site = site();
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_click");
        navigate(&handle, &gate, &site.url("/")).expect("the page loads");
        let link = refs(&handle, "link").remove(0);
        let act = handle
            .mutate(|browser| browser.activate(&link))
            .expect("the ref is on this page");
        let Action::Navigate(url) = act else {
            panic!("a link navigates");
        };
        assert!(url.ends_with("/second"), "the link's own target: {url}");
        navigate(&handle, &gate, &url).expect("the second page loads");
        let snapshot = handle.read(|browser| browser.snapshot()).expect("a page");
        assert_eq!(snapshot.title, "Second Page");
        assert_eq!(site.hits(), 2, "two requests");
    }

    /// Typing into a field is visible in the next snapshot.
    #[test]
    fn typing_into_a_field_shows_the_value_in_the_snapshot() {
        let site = site();
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_open");
        navigate(&handle, &gate, &site.url("/")).expect("the page loads");
        let field = refs(&handle, "input").remove(0);
        handle
            .mutate(|browser| browser.type_into(&field, "bos harness"))
            .expect("the field takes text");
        let snapshot = handle.read(|browser| browser.snapshot()).expect("a page");
        let element = snapshot
            .elements
            .iter()
            .find(|element| element.reference == field)
            .expect("the field is still there");
        assert_eq!(
            element.value.as_deref(),
            Some("bos harness"),
            "the typed value is in the snapshot"
        );
    }

    /// A stale ref says the page changed; it is never a silent no-op.
    #[test]
    fn a_stale_ref_says_the_page_changed() {
        let site = site();
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_click");
        navigate(&handle, &gate, &site.url("/")).expect("the page loads");
        let old = refs(&handle, "link").remove(0);
        navigate(&handle, &gate, &site.url("/second")).expect("the second page loads");
        // The second page has no elements at all, so the old ref cannot name
        // anything on it.
        let error = handle
            .mutate(|browser| browser.activate(&old))
            .expect_err("the old ref is stale");
        assert!(
            error.contains("page changed") && error.contains("browser_snapshot"),
            "the error names the change and the next step: {error}"
        );
        // And on a page that DOES have elements, the stale ref still names
        // nothing, because ids are monotonic across documents.
        navigate(&handle, &gate, &site.url("/")).expect("back to the first page");
        let fresh = refs(&handle, "link").remove(0);
        assert_ne!(fresh, old, "a reloaded page re-mints its refs");
        let error = handle
            .mutate(|browser| browser.activate(&old))
            .expect_err("the old ref is still stale");
        assert!(error.contains("page changed"), "{error}");
    }

    /// A URL outside the allowlist is refused with the rule named, and the
    /// listener sees nothing at all.
    #[test]
    fn a_refused_url_names_the_rule_and_sends_no_request() {
        let site = site();
        let handle = browser();
        // The allowlist names an address that is not this site's, so the policy
        // refuses 127.0.0.1 before a socket exists.
        let gate = gate(&["192.0.2.1"], "browser_open");
        let failure = navigate(&handle, &gate, &site.url("/")).expect_err("the policy refuses it");
        assert_eq!(failure.kind, "policy", "{failure:?}");
        assert_eq!(
            failure.rule.as_deref(),
            Some("egress-not-allowlisted"),
            "the refusal names the rule: {failure:?}"
        );
        assert!(
            failure.message.contains("egress-not-allowlisted"),
            "the prose names it too: {}",
            failure.message
        );
        assert!(
            failure.tool_message().contains("No request was made"),
            "{}",
            failure.tool_message()
        );
        assert_eq!(site.hits(), 0, "the listener saw no request");
        assert_eq!(
            handle.read(|browser| browser.status()),
            Status::Failed,
            "a refused load leaves no page"
        );
    }

    /// A snapshot's text is capped, and the payload says so.
    #[test]
    fn the_snapshot_text_is_capped_and_the_cap_is_visible() {
        let site = site();
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_open");
        navigate(&handle, &gate, &site.url("/long")).expect("the page loads");
        let snapshot = handle.read(|browser| browser.snapshot()).expect("a page");
        assert_eq!(snapshot.text_cap, TEXT_CAP);
        assert_eq!(
            snapshot.text.chars().count(),
            TEXT_CAP,
            "the text is exactly the cap"
        );
        assert!(snapshot.text_truncated, "the page had more text");
        assert!(
            snapshot.text_total_chars > TEXT_CAP,
            "the total exceeds the cap: {}",
            snapshot.text_total_chars
        );
        let json = snapshot.to_json();
        assert_eq!(json["text_truncated"], serde_json::json!(true));
        assert_eq!(json["text_cap"], serde_json::json!(TEXT_CAP));
        assert_eq!(json["text_chars"], serde_json::json!(TEXT_CAP));
    }

    /// Only http(s) reaches a policy question.
    #[test]
    fn a_non_http_scheme_is_refused_before_any_request() {
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_open");
        let failure = navigate(&handle, &gate, "file:///etc/passwd").expect_err("refused");
        assert_eq!(failure.kind, "scheme", "{failure:?}");
        assert!(
            failure.tool_message().contains("http(s)"),
            "{}",
            failure.tool_message()
        );
    }

    /// History restores a cached page and re-mints the refs.
    #[test]
    fn history_back_restores_the_page_with_new_refs() {
        let site = site();
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_open");
        navigate(&handle, &gate, &site.url("/")).expect("the page loads");
        let first = handle.read(|browser| browser.snapshot()).unwrap().title;
        let old_ref = refs(&handle, "link").remove(0);
        navigate(&handle, &gate, &site.url("/second")).expect("the second page loads");
        let hits = site.hits();
        handle
            .mutate(|browser| browser.back())
            .expect("history has an earlier page");
        let snapshot = handle.read(|browser| browser.snapshot()).expect("a page");
        assert_eq!(snapshot.title, first, "the first page is back");
        assert!(snapshot.can_forward, "and forward is available");
        assert_eq!(site.hits(), hits, "a cached restore makes no request");
        assert!(
            handle.mutate(|browser| browser.activate(&old_ref)).is_err(),
            "the restored page minted new refs"
        );
    }

    /// A GET form submits its fields, with the values the reader typed.
    #[test]
    fn a_get_form_submits_its_fields() {
        let site = site();
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_open");
        navigate(&handle, &gate, &site.url("/")).expect("the page loads");
        let field = refs(&handle, "input").remove(0);
        handle
            .mutate(|browser| browser.type_into(&field, "bos harness"))
            .expect("the field takes text");
        handle
            .mutate(|browser| browser.activate(&field))
            .expect("the field focuses");
        let act = handle
            .mutate(|browser| browser.press_key("enter"))
            .expect("enter submits the form");
        let Action::Navigate(url) = act else {
            panic!("Enter on a form field navigates: {act:?}");
        };
        assert_eq!(
            url,
            format!(
                "{}/search?q=bos+harness",
                site.url("").trim_end_matches('/')
            ),
            "the GET query carries the field"
        );
    }

    /// Opening a local page yields the server's own title, text and elements.
    #[test]
    fn a_local_page_arrives_with_its_title_text_and_elements() {
        let site = site();
        let handle = browser();
        let gate = gate(&["127.0.0.1"], "browser_open");
        navigate(&handle, &gate, &site.url("/")).expect("the page loads");
        let snapshot = handle.read(|browser| browser.snapshot()).expect("a page");
        assert_eq!(snapshot.title, "Test Page");
        assert!(
            snapshot.text.contains("hello from the test server"),
            "the page's real text: {:?}",
            snapshot.text
        );
        assert_eq!(refs(&handle, "link"), vec![String::from("e1")]);
        assert!(refs(&handle, "input").len() == 1, "one search field");
        assert_eq!(
            handle.read(|browser| browser.status()),
            Status::Ready,
            "a loaded page"
        );
        assert_eq!(site.hits(), 1, "one request");

        // The wire payload carries the same facts, with every cap beside its
        // value.
        let json = snapshot.to_json();
        assert_eq!(json["text_cap"], serde_json::json!(TEXT_CAP));
        assert_eq!(json["elements_cap"], serde_json::json!(ELEMENT_CAP));
        assert_eq!(json["elements_total"], serde_json::json!(3));
        assert_eq!(json["text_truncated"], serde_json::json!(false));
    }
    /// A real HTTPS page through the same gate. Network-dependent, so it is
    /// ignored by default; run it with --ignored when a browser "does not
    /// render" to separate the transport from the pane.
    #[test]
    #[ignore]
    fn probe_https_example_com() {
        let handle = browser();
        let gate = gate(&["0.0.0.0/0", "::/0"], "browser_open");
        let outcome = navigate(&handle, &gate, "https://example.com/");
        match outcome {
            Ok(()) => {
                let snapshot = handle.read(|browser| browser.snapshot()).expect("a page");
                let (raw_len, anchors, head) = handle.read(|browser| {
                    let doc = browser.document().expect("a document");
                    (
                        doc.raw.len(),
                        doc.raw.matches("<a ").count(),
                        doc.raw.chars().take(300).collect::<String>(),
                    )
                });
                println!(
                    "HTTPS OK title={:?} chars={} elements={} raw_len={} anchors={}",
                    snapshot.title, snapshot.text_chars, snapshot.elements_total, raw_len, anchors
                );
                println!("HEAD: {head}");
                assert!(snapshot.text_chars > 0, "the page had no text");
            }
            Err(failure) => panic!("HTTPS failed: {failure:?}"),
        }
    }

    /// The published shape of example.com: one heading, two paragraphs, ONE
    /// link. If the extractor drops it, the pane shows no rows and the model has
    /// no ref to act on.
    #[test]
    fn a_page_with_one_link_yields_one_link() {
        let html = "<!doctype html><html><head><title>Example Domain</title></head><body>\
                    <div><h1>Example Domain</h1>\
                    <p>This domain is for use in illustrative examples.</p>\
                    <p><a href=\"https://www.iana.org/domains/example\">More information...</a></p>\
                    </div></body></html>";
        let mut next = 1;
        let doc = parse_document("https://example.com/", html, &mut next);
        assert_eq!(doc.title, "Example Domain");
        assert_eq!(doc.elements.len(), 1, "one link: {:?}", doc.elements);
        assert_eq!(doc.elements[0].kind, Kind::Link);
        assert_eq!(
            doc.elements[0].href.as_deref(),
            Some("https://www.iana.org/domains/example")
        );
        assert!(
            doc.elements[0].label.contains("More information"),
            "the label: {:?}",
            doc.elements[0].label
        );
    }
}
