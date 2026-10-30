//! The terminal pane's model: the policy gate, its audit ring, and the line the
//! gate is shown.
//!
//! The pane's backend is libghostty (`gpui-libghostty`): Ghostty owns the PTY,
//! the parser, the scrollback and the drawing (see `ui/terminal_view.rs`), so
//! this module no longer keeps a screen. What stays is everything the pane
//! decides BEFORE bytes reach the shell: the workspace's Rego policy
//! (`.bsh/terminal-policy.rego`), the start gate, the Enter gate on the
//! mirrored line, and the bounded audit ring both answer through.
//!
//! # The gate
//!
//! One Rego module (package `terminal`) decides two points. Each evaluation
//! receives an `input` object and the gate reads `data.terminal.verdict`; the
//! shim builds that rule from the policy's own `decision` (a boolean or an
//! object) and falls back to its plain `allow` rule, so a policy may be either
//! shape:
//!
//! - **start**: `{action: "start", command, shell, cwd, deny}`, asked before
//!   any process exists. A refused start spawns nothing, and that stays the
//!   only door that guarantees no process runs.
//! - **input**: `{action: "input", command: <the submitted line, exactly>,
//!   cwd, deny}`, asked on Enter before the submitting carriage return is
//!   written. A refusal writes no line; the pane sends Ctrl-U instead so the
//!   shell's line editor erases the text it has already echoed, and the
//!   refusal is shown in the pane's status line and recorded in the audit ring.
//!
//! The output point is RETIRED, not forgotten. A native libghostty surface reads
//! the PTY itself and hands the bytes to Ghostty's renderer, so there is no
//! chunk bsh could show a policy before Ghostty draws it. A policy module may
//! still carry output rules; they are never consulted, and
//! docs/bsh-terminal-policy.md says so where a policy author reads the
//! contract. The start gate remains the hard boundary.
//!
//! Deliberately NOT gated: individual keystrokes (printable input, arrows,
//! control chords, backspace), cursor movement, resize, and scrollback
//! browsing. Those are line editing and window management, not commands, and a
//! live echo means the line exists in the shell's editor before Enter. Because
//! the line is a mirror of what this pane typed, a shell-side edit (history
//! recall, a mid-line cursor jump) can differ from it. The input object's `cwd`
//! is the directory the terminal was started in; bsh keeps no mirror of the
//! shell's live directory, so a `cd` the shell made is invisible to the policy.
//!
//! The policy file is the workspace's .bsh/terminal-policy.rego. A missing file is
//! the documented default (allow everything, the shipped policy module). A file
//! that exists but does not parse closes every gate, so an unusable rule can
//! never widen the gate it was meant to narrow. The file is re-read when its
//! stamp changes (checked from the terminal clock, not per frame); a broken edit
//! keeps the LAST GOOD policy, records the parse error in the audit ring and
//! shows it in the status line, so a reload never wedges and never silently
//! widens. Every decision is appended to a bounded audit ring and the pane
//! reports a compact count once something was refused. There is no settings
//! surface for policies: the file, its reload and the audit are the management
//! surface. The contract and worked examples are in
//! `docs/bsh-terminal-policy.md`.
//!
//! # The sandbox
//!
//! The pane still asks the turn's shared [agent::sandbox::SandboxSpec] what it
//! may do with the pane's terminal, and reports the answer. It can only REPORT
//! it now: Ghostty creates the PTY and starts the shell itself, so the sandbox
//! has no tty path to open by name and cannot join the shell. A spec that would
//! have confined the shell therefore reads "unsupported-pty", the shell starts
//! unconfined, and the status line names the boundary - the same sentence the
//! Windows backend already used, because it is the same limitation. The Rego
//! input gate still decides every submitted line, and the start gate still
//! decides whether a shell exists at all.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use regorus::{Engine as RegoEngine, Value as RegoValue};

use crate::i18n::Lang;

/// How much of a decision's subject the audit ring keeps, in characters.
const AUDIT_SUBJECT_CAP: usize = 160;
/// How much of a policy's own reason the audit ring keeps, in characters. The
/// ring holds a fixed number of rows, so an uncapped reason would make its
/// memory a function of the policy file rather than of the ring.
const AUDIT_REASON_CAP: usize = 512;
/// How many decisions the audit ring retains.
pub(crate) const AUDIT_LIMIT: usize = 256;

/// Where a gate's rules came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicySource {
    /// The shipped allow-all policy: no workspace policy file exists.
    Default,
    /// The workspace's .bsh/terminal-policy.rego file.
    File,
}

impl PolicySource {
    /// The label an audit row carries.
    pub(crate) fn label(self) -> &'static str {
        match self {
            PolicySource::Default => "default",
            PolicySource::File => "file",
        }
    }
}

/// One gate point's decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Decision {
    allow: bool,
    reason: Option<String>,
    redact: Option<String>,
}

impl Decision {
    /// Let the action through untouched.
    fn allow() -> Self {
        Self {
            allow: true,
            ..Self::default()
        }
    }

    /// Refuse the action, optionally with the policy's own reason.
    fn refuse(reason: Option<String>) -> Self {
        Self {
            allow: false,
            reason,
            redact: None,
        }
    }

    /// Whether the action may proceed.
    pub(crate) fn allowed(&self) -> bool {
        self.allow
    }

    /// The policy's reason for refusing.
    pub(crate) fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The text a decision asked to substitute.
    ///
    /// Only output decisions could carry one, and the output point is retired
    /// with the native backend; the field stays because a policy may still
    /// define it and the audit still records whether one was answered with.
    pub(crate) fn redaction(&self) -> Option<&str> {
        self.redact.as_deref()
    }
}

/// One decision, as the audit ring keeps it.
///
/// Serialisable: the ring the pane counts is also the record a deployment can
/// hand to serde, which is the audit's management face outside the window.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Audit {
    /// When the decision was made.
    pub(crate) at: SystemTime,
    /// The gate point: "start", "input" or "policy".
    pub(crate) action: &'static str,
    /// The command, or the policy path the loader refused.
    pub(crate) subject: String,
    /// Whether the action proceeded.
    pub(crate) allow: bool,
    /// Whether the action proceeded with substituted output. Always false for
    /// the two points that remain; kept so a stored ring stays readable.
    pub(crate) redacted: bool,
    /// The policy's own reason, when it gave one.
    pub(crate) reason: Option<String>,
    /// "file" or "default": which rules decided.
    pub(crate) source: &'static str,
}

/// The most recent refused input, for the pane's status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputRefusal {
    /// The exact line the gate refused.
    pub(crate) line: String,
    /// The policy's own reason, when it gave one.
    pub(crate) reason: Option<String>,
}

/// One editing keystroke, in the terms the input gate mirrors.
///
/// The mirror is deliberately coarse: it follows the line the reader is typing,
/// and nothing a shell-side edit does (history recall, a mid-line cursor jump)
/// can reach it. See the module docs for why that is the honest line available
/// with a live echo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    /// A printable text run: a typed character, a composed commit, or pasted
    /// text.
    Text(String),
    /// Backspace.
    Backspace,
    /// Ctrl-U / Ctrl-C: the shell's line editor kills the whole line.
    KillLine,
    /// Ctrl-W: the shell's line editor kills the trailing word.
    KillWord,
}

/// What the pane writes to the shell for one gated answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Emission {
    /// Submit the line: the carriage return Enter would have sent.
    Enter,
    /// Erase the echoed line: the shell's kill-whole-line (Ctrl-U).
    KillLine,
}

/// What one keystroke did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyOutcome {
    /// The keystroke reached the shell.
    Sent,
    /// The gate refused the submitted line; nothing was submitted.
    Refused,
    /// There is no running shell to write to.
    Closed,
}

/// The policy module the gate is born with.
///
/// It is the documented default (allow everything) and the only module this
/// crate owns; a workspace policy layers over it. It also builds the one rule
/// the gate reads, so a policy can answer with either shape: 'decision', a
/// boolean or an object, wins when it is defined, and the plain 'allow' rule is
/// the fallback every older policy is written against.
const DEFAULT_POLICY: &str = r#"
package terminal

# The shipped default: every gate point is open. input.deny is the shim's own
# escape hatch and is always false, so it never closes anything on its own.
default allow := true

allow := false if {
    input.deny
}

# The gate reads exactly one rule. A policy that wants to give a reason defines
# decision (a boolean or an object) instead of allow; both shapes are accepted
# and decision wins.
verdict := answer if {
    answer := data.terminal.decision
} else := allow
"#;

/// The regorus gate that decides whether the terminal may act.
///
/// The engine holds one Rego policy that receives each action as input and
/// returns a verdict. The default policy allows everything; a deployment
/// replaces it with its own rules. The gate is asked at two points - the
/// shell's start and each submitted input line - and a policy that fails to
/// load closes both.
pub(crate) struct Gate {
    engine: RegoEngine,
    source: PolicySource,
    /// The parse error that closed the gate, if it was closed.
    closed: Option<String>,
}

impl Gate {
    /// A gate whose policy allows every action.
    pub(crate) fn allow_all() -> Self {
        Self::new(PolicySource::Default)
    }

    fn new(source: PolicySource) -> Self {
        let mut engine = RegoEngine::new();
        engine
            .add_policy("terminal.rego".to_string(), DEFAULT_POLICY.to_string())
            .expect("the shipped policy is valid Rego");
        Self {
            engine,
            source,
            closed: None,
        }
    }

    /// A gate closed by the parse error that made its policy unusable.
    fn closed(source: PolicySource, error: String) -> Self {
        let mut gate = Self::new(source);
        gate.closed = Some(error);
        gate
    }

    /// A gate built from a workspace policy's source text.
    pub(crate) fn for_file(source: &str) -> anyhow::Result<Self> {
        let mut gate = Self::new(PolicySource::File);
        gate.load_policy("workspace.rego", source)?;
        Ok(gate)
    }

    /// Where this gate's rules came from.
    pub(crate) fn source(&self) -> PolicySource {
        self.source
    }

    /// Load an additional policy module.
    ///
    /// Deployments narrow the gate by adding their own modules: a later
    /// policy's decisions layer over the default's allow-all baseline. A module
    /// that cannot be parsed latches the gate shut, so an unusable rule can
    /// never widen the door it was meant to narrow.
    pub(crate) fn load_policy(&mut self, id: &str, source: &str) -> anyhow::Result<()> {
        if let Err(error) = self.engine.add_policy(id.to_string(), source.to_string()) {
            self.closed = Some(error.to_string());
            return Err(error);
        }
        Ok(())
    }

    /// Whether the gate permits this shell to start in this directory.
    pub(crate) fn start(&mut self, shell: &Path, cwd: &Path) -> Decision {
        let command = shell.to_string_lossy();
        let cwd = cwd.to_string_lossy();
        self.decide(
            "start",
            input_document([
                ("action", RegoValue::from("start")),
                ("command", RegoValue::from(command.as_ref())),
                ("shell", RegoValue::from(command.as_ref())),
                ("cwd", RegoValue::from(cwd.as_ref())),
                ("deny", RegoValue::from(false)),
            ]),
        )
    }

    /// Whether the gate permits this submitted line to reach the shell.
    pub(crate) fn input(&mut self, line: &str, cwd: &Path) -> Decision {
        let cwd = cwd.to_string_lossy();
        self.decide(
            "input",
            input_document([
                ("action", RegoValue::from("input")),
                ("command", RegoValue::from(line)),
                ("cwd", RegoValue::from(cwd.as_ref())),
                ("deny", RegoValue::from(false)),
            ]),
        )
    }

    /// Ask the policy one question and read its verdict.
    ///
    /// 'action' is the gate point this question belongs to. It is passed rather
    /// than read back out of the input document so a refusal can name the policy
    /// that said no without re-parsing what the gate just built.
    fn decide(&mut self, action: &str, input: RegoValue) -> Decision {
        if let Some(reason) = self.closed.as_ref() {
            return Decision::refuse(Some(reason.clone()));
        }
        self.engine.set_input(input);
        let Ok(results) = self
            .engine
            .eval_query("data.terminal.verdict".to_string(), false)
        else {
            return Decision::refuse(Some(String::from("the policy returned no verdict")));
        };
        let Some(expression) = results
            .result
            .first()
            .and_then(|row| row.expressions.first())
        else {
            return Decision::refuse(Some(String::from("the policy returned no verdict")));
        };
        match serde_json::to_value(&expression.value) {
            Ok(json) => {
                let decision = decision_from_json(json);
                // A plain 'false' carries no reason of its own, and a refusal
                // without a reason is a dead end for the reader. Name the policy
                // that said no, so the status line always answers "why".
                if action != "start" && !decision.allowed() && decision.reason().is_none() {
                    return Decision::refuse(Some(format!(
                        "refused by the {}",
                        self.source().label()
                    )));
                }
                decision
            }
            Err(_) => Decision::refuse(Some(String::from("the policy's verdict is not readable"))),
        }
    }
}

/// One gate question's input document, as a Regorus object.
///
/// The policy only ever reads the fields, so they are built directly instead of
/// serialising a 'serde_json::Value' and parsing it back for every question.
fn input_document<const N: usize>(fields: [(&'static str, RegoValue); N]) -> RegoValue {
    let map: BTreeMap<RegoValue, RegoValue> = fields
        .into_iter()
        .map(|(key, value)| (RegoValue::from(key), value))
        .collect();
    RegoValue::from(map)
}

/// Read one policy verdict: a boolean, or an object with an optional reason.
fn decision_from_json(json: serde_json::Value) -> Decision {
    match json {
        serde_json::Value::Bool(true) => Decision::allow(),
        serde_json::Value::Bool(false) => Decision::refuse(None),
        serde_json::Value::Object(fields) => Decision {
            allow: fields
                .get("allow")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            reason: fields
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            redact: fields
                .get("redact")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        },
        _ => Decision::refuse(Some(String::from(
            "the policy's verdict is neither a boolean nor an object",
        ))),
    }
}

/// What reading the workspace policy found.
enum PolicyRead {
    /// A policy file that parsed. Boxed because the engine is far larger than
    /// the other two answers, and this enum is built on every reload.
    Loaded(Box<Gate>),
    /// No policy file: the documented allow-all default.
    Missing,
    /// A policy file that exists and does not parse. The message names the file.
    Broken(String),
}

/// Append one keystroke decision to /tmp/bsh-term.log.
///
/// Temporary diagnostics: a live "Enter does nothing" report cannot be
/// reproduced in the harness, so the pane records what it actually saw. It is
/// opt-in with 'BSH_TERM_TRACE=1': the pane calls this on every keystroke, and
/// an unconditional open and append put a synchronous disk write on the input
/// path.
pub(crate) fn trace_key(key: &str, bytes: Option<&[u8]>) {
    use std::io::Write as _;
    if !trace_enabled() {
        return;
    }
    let line = match bytes {
        Some(bytes) => format!("key={key} bytes={bytes:?}\n"),
        None => format!("key={key} unmapped\n"),
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/bsh-term.log")
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Whether the opt-in keystroke trace is on, read once.
fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("BSH_TERM_TRACE").is_some())
}

/// The policy body written into a workspace the first time its terminal opens,
/// and the body of the '.bsh/terminal-policy.rego.tpl' authoring template.
///
/// One source of truth for both, so the file the pane drops and the file a
/// reader copies can never describe different rules. It must stay a VALID
/// module as written: a reader who copies it without editing a character must
/// get a policy that parses, allows, and changes nothing. Every worked example
/// is guarded by 'examples := false' AND reads a list that starts EMPTY, so
/// flipping the switch on its own changes nothing; a test flips it, injects a
/// scratch value and proves each refusal fires, so the examples cannot rot.
pub(crate) const WORKSPACE_POLICY_TEMPLATE: &str = r##"package terminal

# The terminal gate asks this policy at two points, each with its own input
# document. Every field is always present; 'deny' is bsh's own escape hatch and
# is always false, so a policy may ignore it.
#
#   start   {"action": "start",  "command": <shell path>, "shell": <shell path>,
#            "cwd": <workspace>, "deny": false}
#           Asked before any process exists. 'command' and 'shell' are the same
#           path. A refused start spawns NOTHING: it is the only door that
#           guarantees no process runs.
#
#   input   {"action": "input",  "command": <the submitted line, exactly>,
#            "cwd": <workspace>, "deny": false}
#           Asked on Enter, before the submitting carriage return is written.
#           'cwd' is the directory the terminal STARTED in, so a 'cd' the shell
#           made is invisible here.
#
# There used to be a third point, 'output', asked per chunk read from the PTY.
# The pane's terminal is now libghostty, which reads the PTY itself and draws
# the bytes directly, so no chunk passes through bsh and no output rule is ever
# consulted. Rules that mention action "output" are accepted and ignored.
#
# Answer with a BOOLEAN, or with an OBJECT to explain a refusal:
#
#   {"allow": true}
#   {"allow": false, "reason": "<shown in the pane's status line>"}
#
# An object that omits 'allow' reads as false. Telling the points apart is one
# test on the action: input.action == "start" / "input".
#
# The default is open. Narrow it by naming what to refuse below.
default allow := true

# -----------------------------------------------------------------------------
# The switch. Every worked example is guarded by it AND reads a list that starts
# EMPTY, so flipping it to true on its own changes NOTHING: name what to refuse
# in the list first, then flip. Flipping it back to false disables every example
# at once without deleting it.
# -----------------------------------------------------------------------------
examples := false

# -----------------------------------------------------------------------------
# 1. Refuse a destructive input line.
#
# A line containing one of these substrings is refused and never submitted: no
# carriage return reaches the shell, and Ctrl-U clears what it echoed.
# -----------------------------------------------------------------------------
denied_commands := [
    # "rm -rf",
    # "mkfs",
    # "dd of=/dev/",
]

denied_command(command) if {
    some needle in denied_commands
    contains(command, needle)
}

decision := {"allow": false, "reason": "this command is on the workspace's destructive list"} if {
    examples
    input.action == "input"
    denied_command(input.command)
}

# -----------------------------------------------------------------------------
# 2. Refuse to start one shell.
#
# The start gate is the hard boundary: a refusal leaves the pane with no process
# at all. 'shell' is the shell's path, and it is the same value as 'command'.
# -----------------------------------------------------------------------------
deny_shells := [
    # "/bin/bash",
]

denied_shell(shell) if {
    some refused in deny_shells
    shell == refused
}

decision := {"allow": false, "reason": "this shell is not used in this workspace"} if {
    examples
    input.action == "start"
    denied_shell(input.shell)
}
"##;

fn policy_path(cwd: &Path) -> PathBuf {
    cwd.join(".bsh").join("terminal-policy.rego")
}

/// Write the starter policy when the workspace has none, so a reader can edit
/// the file the gate actually reads instead of inventing it from the docs.
///
/// A write that fails is not an error the terminal should carry: the shipped
/// default already allows everything, so the pane opens exactly as it would
/// have with no file.
fn seed_policy(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // create_new is the existence check and the creation in one operation, so a
    // reader who writes their own policy at the same moment the pane first
    // opens it cannot have it truncated: either this creates the file, or their
    // edit wins.
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    else {
        return;
    };
    let _ = file.write_all(WORKSPACE_POLICY_TEMPLATE.as_bytes());
}

/// The file's stamp: its modification time and length, which is what a reload
/// compares so a quiet terminal pays one stat.
fn policy_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Read the workspace policy, keeping a parse failure separate from a missing
/// file: one closes every gate, the other is the documented default.
fn read_policy(path: &Path) -> PolicyRead {
    match std::fs::read_to_string(path) {
        Ok(text) => match Gate::for_file(&text) {
            Ok(gate) => PolicyRead::Loaded(Box::new(gate)),
            Err(error) => PolicyRead::Broken(format!("{}: {error}", path.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PolicyRead::Missing,
        Err(error) => PolicyRead::Broken(format!("{}: {error}", path.display())),
    }
}

/// The gate a policy path produces, and the parse error that closed it.
///
/// A path with no name has no workspace to confine, so it gets the shipped
/// allow-all gate; a file that does not parse closes the gate and names its
/// error, which is what the pane reports.
fn open_gate(path: &Path) -> (Gate, Option<String>) {
    if path.as_os_str().is_empty() {
        return (Gate::allow_all(), None);
    }
    match read_policy(path) {
        PolicyRead::Loaded(gate) => (*gate, None),
        PolicyRead::Missing => (Gate::allow_all(), None),
        PolicyRead::Broken(error) => (Gate::closed(PolicySource::File, error.clone()), Some(error)),
    }
}

/// The gate a workspace's terminal policy file produces, without a pane.
///
/// The authoring-template tests need exactly what the loader does - the exact
/// '.bsh/terminal-policy.rego' name, and the parse-or-fail-closed rule - without
/// opening a terminal. Used only by tests.
#[cfg(test)]
pub(crate) fn gate_for_workspace(root: &Path) -> Gate {
    open_gate(&policy_path(root)).0
}

/// Keep at most cap characters of a subject, on a character boundary.
fn truncate_chars(text: &str, cap: usize) -> &str {
    match text.char_indices().nth(cap) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// The bounded decision log, with the count the pane reports kept as it goes.
///
/// The count is maintained on every push and trim rather than by walking the
/// ring: the pane reads it from its render path, and the ring can hold
/// 'AUDIT_LIMIT' rows.
#[derive(Default)]
struct AuditRing {
    rows: VecDeque<Audit>,
    /// Rows that refused something. Maintained as rows go in and out, because
    /// the ring trims its front and a count cannot be recovered from a
    /// high-water mark.
    blocked: u64,
}

impl AuditRing {
    /// Append one row, trimming the oldest first so the ring stays bounded.
    fn push(&mut self, row: Audit) {
        while self.rows.len() >= AUDIT_LIMIT {
            if let Some(old) = self.rows.pop_front() {
                if !old.allow || old.redacted {
                    self.blocked = self.blocked.saturating_sub(1);
                }
            }
        }
        if !row.allow || row.redacted {
            self.blocked += 1;
        }
        self.rows.push_back(row);
    }
}

/// Append one decision to the audit ring.
fn record_decision(
    audit: &Mutex<AuditRing>,
    action: &'static str,
    subject: &str,
    decision: &Decision,
    source: PolicySource,
) {
    let redacted = decision.redaction().is_some();
    let row = Audit {
        at: SystemTime::now(),
        action,
        subject: truncate_chars(subject, AUDIT_SUBJECT_CAP).to_string(),
        allow: decision.allowed(),
        redacted,
        reason: decision
            .reason()
            .map(|reason| truncate_chars(reason, AUDIT_REASON_CAP).to_string()),
        source: source.label(),
    };
    if let Ok(mut ring) = audit.lock() {
        ring.push(row);
    }
}

/// The POLICY that left the pane's shell unconfined, when one did.
///
/// Two policies can release it and the shell reports the same "none" either
/// way, so the pane keeps WHICH one: the settings master switch (which wins over
/// every preset's arm) or the preset in force. They are reversed by different
/// controls, and a reader who is told only "not sandboxed" cannot tell which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SandboxOff {
    /// The sandbox master switch is off in the settings.
    Settings,
    /// The preset in force asked for an unconfined shell.
    Preset,
}

/// The terminal pane: what the policy decided, and the line Enter will submit.
pub(crate) struct Terminal {
    gate: Arc<Mutex<Gate>>,
    audit: Arc<Mutex<AuditRing>>,
    /// The shell's working directory, which is also the input gate's cwd.
    pub(crate) cwd: PathBuf,
    /// The value the shell sees as BSH_SANDBOX, when there is one to report.
    pub(crate) sandbox_env: Option<&'static str>,
    /// Why this shell is not inside the turn's sandbox, when it is not.
    pub(crate) sandbox_note: Option<String>,
    /// The POLICY that released this shell, when one did.
    pub(crate) sandbox_off: Option<SandboxOff>,
    /// Why the shell is not running, when it is not.
    pub(crate) failure: Option<String>,
    /// The gate refused the start, so no process was ever created.
    pub(crate) refused: bool,
    /// The workspace policy's path and stamp, so a reload costs one stat.
    policy_path: PathBuf,
    policy_stamp: Option<(SystemTime, u64)>,
    /// The last policy that failed to load or reload.
    policy_error: Option<String>,
    /// The line typed since the last submit: what the input gate is shown.
    typed_line: String,
    /// The shell this pane runs.
    ///
    /// Kept so a restart runs the SAME shell: a policy that refuses one shell
    /// must not be answered by silently choosing another.
    pub(crate) shell: PathBuf,
}

impl Terminal {
    /// A terminal in cwd, with no process of its own: the gate, the policy
    /// read, and the start decision.
    ///
    /// The workspace's .bsh/terminal-policy.rego is loaded when present and
    /// the gate is asked whether the shell may start. Nothing is spawned when
    /// it says no, and a policy that exists but cannot be parsed fails CLOSED:
    /// an unusable rule must never widen the gate it was meant to narrow.
    #[cfg(test)]
    pub(crate) fn new(cwd: PathBuf, lang: Lang) -> Self {
        Self::build(cwd, lang, None, None)
    }

    /// A terminal whose start was told what the turn may touch.
    ///
    /// The spec is the same value the shell tool runs under. Its answer for the
    /// pane's terminal is recorded and shown - Ghostty creates the PTY itself,
    /// so the sandbox cannot join it - and the Rego gate decides the shell and
    /// every submitted line.
    pub(crate) fn new_with_sandbox(
        cwd: PathBuf,
        lang: Lang,
        spec: Option<&agent::sandbox::SandboxSpec>,
    ) -> Self {
        Self::build(cwd, lang, spec, None)
    }

    /// The same start, with the attribution a policy release needs.
    ///
    /// 'off' is the POLICY that released the shell and the sentence naming it.
    /// It is passed BEFORE the spawn so the shell's own BSH_SANDBOX reports
    /// "none"; setting it afterwards would leave a running shell claiming a
    /// tier while the status line said the sandbox was off.
    pub(crate) fn new_with_sandbox_note(
        cwd: PathBuf,
        lang: Lang,
        spec: Option<&agent::sandbox::SandboxSpec>,
        off: Option<(SandboxOff, String)>,
    ) -> Self {
        Self::build(cwd, lang, spec, off)
    }

    /// Why this shell is not sandboxed, when it is not.
    pub(crate) fn sandbox_note(&self) -> Option<&str> {
        self.sandbox_note.as_deref()
    }

    /// What the shell sees as BSH_SANDBOX, when there is something to say.
    ///
    /// "none" is a policy that turned the sandbox off (the settings switch or a
    /// preset), "full-access" is a permission-released turn, a permission tier
    /// is a sandboxed shell, and "unsupported-pty" is a backend that could not
    /// host the pane's terminal.
    pub(crate) fn sandbox_label(&self) -> Option<&'static str> {
        self.sandbox_env
    }

    /// Build the model, record the sandbox answer, and ask the start gate.
    fn build(
        cwd: PathBuf,
        lang: Lang,
        spec: Option<&agent::sandbox::SandboxSpec>,
        off: Option<(SandboxOff, String)>,
    ) -> Self {
        let path = if cwd.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            policy_path(&cwd)
        };
        if !path.as_os_str().is_empty() {
            seed_policy(&path);
        }
        let (gate, policy_error) = open_gate(&path);
        let stamp = if path.as_os_str().is_empty() {
            None
        } else {
            policy_stamp(&path)
        };
        let (mut sandbox_env, mut sandbox_note) = sandbox_answer(lang, spec);
        let sandbox_off = off.as_ref().map(|(kind, _)| *kind);
        if let Some((_, note)) = off {
            // The shell reports what the TURN decided, not only the permission
            // tier: a policy release (the settings switch or a preset) is
            // "none", so the pane and the bash result agree. This must happen
            // BEFORE the spawn.
            sandbox_env = Some("none");
            sandbox_note = Some(note);
        }
        let mut terminal = Self {
            gate: Arc::new(Mutex::new(gate)),
            audit: Arc::new(Mutex::new(AuditRing::default())),
            cwd,
            sandbox_env,
            sandbox_note,
            sandbox_off,
            failure: policy_error.clone(),
            refused: policy_error.is_some(),
            policy_path: path,
            policy_stamp: stamp,
            policy_error,
            typed_line: String::new(),
            shell: shell_program(),
        };
        if let Some(error) = terminal.policy_error.clone() {
            let subject = terminal.policy_path.display().to_string();
            let decision = Decision::refuse(Some(error));
            record_decision(
                &terminal.audit,
                "policy",
                &subject,
                &decision,
                PolicySource::File,
            );
        }
        terminal.ask_start();
        terminal
    }

    /// Ask the start gate about the shell and remember the answer.
    fn ask_start(&mut self) -> bool {
        if self.refused {
            return false;
        }
        let shell = self.shell.clone();
        let cwd = self.cwd.clone();
        let decision = self.decide("start", &shell.display().to_string(), |gate| {
            gate.start(&shell, &cwd)
        });
        if !decision.allowed() {
            self.refused = true;
            self.failure = decision.reason().map(str::to_string);
        }
        decision.allowed()
    }

    /// Ask the start gate again for a fresh shell.
    ///
    /// Enter on a dead pane restarts: the same shell, under the same gate - a
    /// new shell has to pass the start rule again. A start refused because the
    /// policy FILE did not parse is cleared by the reload, not by Enter (see
    /// [Terminal::refresh_policy]); a policy that says no is simply refused
    /// again.
    pub(crate) fn restart(&mut self) -> bool {
        if self.refused && self.policy_error.is_none() {
            return false;
        }
        self.typed_line.clear();
        self.failure = None;
        self.refused = false;
        self.ask_start()
    }

    /// The command Ghostty runs: the shell, with the environment the pane
    /// promises it.
    ///
    /// Ghostty's command string is a shell command it hands to
    /// `login -flp <user> bash -c "exec -l <command>"` on macOS, so an `env`
    /// prefix is how a value reaches the shell without a wrapper process of our
    /// own. TERM and COLORTERM are set by Ghostty itself; the rest is what bsh
    /// has always promised the pane's shell.
    pub(crate) fn command(&self) -> String {
        let mut assignments: Vec<String> = Vec::new();
        if let Some(label) = self.sandbox_env {
            assignments.push(format!("BSH_SANDBOX={}", shell_word(label)));
        }
        #[cfg(unix)]
        {
            // A pane with no rc files would otherwise draw monochrome: BSD ls
            // reads these two, and git's colour.ui already defaults to auto.
            assignments.push("CLICOLOR=1".to_string());
            assignments.push(format!("LSCOLORS={}", shell_word("ExFxCxDxBxegedabagacad")));
        }
        let shell = shell_word(&self.shell.display().to_string());
        if assignments.is_empty() {
            shell
        } else {
            format!("/usr/bin/env {} {}", assignments.join(" "), shell)
        }
    }

    /// Mirror one editing keystroke into the line the input gate will see.
    ///
    /// Arrows, history recall and completion are deliberately not mirrored:
    /// they are not gated (see the module docs), and what they do to the line
    /// is the shell's business.
    pub(crate) fn track(&mut self, edit: Edit) {
        match edit {
            Edit::Text(text) => {
                if text.chars().all(|ch| !ch.is_control()) {
                    self.typed_line.push_str(&text);
                }
            }
            Edit::Backspace => {
                self.typed_line.pop();
            }
            Edit::KillLine => self.typed_line.clear(),
            Edit::KillWord => {
                let trimmed = self.typed_line.trim_end().len();
                self.typed_line.truncate(trimmed);
                match self.typed_line.rfind(' ') {
                    Some(index) => self.typed_line.truncate(index + 1),
                    None => self.typed_line.clear(),
                }
            }
        }
    }

    /// Submit the mirrored line through the input gate.
    ///
    /// An allowed line writes the carriage return Enter would have sent. A
    /// refusal writes no line: the pane sends Ctrl-U instead, so the shell's
    /// line editor erases what it has already echoed, and the refusal is shown
    /// where the reader typed it - the status line - and recorded in the audit.
    pub(crate) fn submit(&mut self, emit: &mut dyn FnMut(Emission) -> bool) -> KeyOutcome {
        let subject = std::mem::take(&mut self.typed_line);
        let cwd = self.cwd.clone();
        let decision = self.decide("input", &subject, |gate| gate.input(&subject, &cwd));
        if decision.allowed() {
            if emit(Emission::Enter) {
                KeyOutcome::Sent
            } else {
                KeyOutcome::Closed
            }
        } else {
            let _ = emit(Emission::KillLine);
            KeyOutcome::Refused
        }
    }

    /// Ask the gate one question, and record the answer in the audit ring.
    fn decide(
        &self,
        action: &'static str,
        subject: &str,
        evaluate: impl FnOnce(&mut Gate) -> Decision,
    ) -> Decision {
        let (decision, source) = match self.gate.lock() {
            Ok(mut gate) => {
                let decision = evaluate(&mut gate);
                let source = gate.source();
                (decision, source)
            }
            Err(_) => (
                Decision::refuse(Some(String::from("the terminal's policy lock is poisoned"))),
                PolicySource::Default,
            ),
        };
        record_decision(&self.audit, action, subject, &decision, source);
        decision
    }

    /// Re-read the workspace policy when its file changed.
    ///
    /// Called from the terminal clock - one stat per tick, never one per frame.
    /// A broken edit keeps the LAST GOOD policy and records the parse error;
    /// deleting the file restores the documented allow-all default. Either way
    /// the gate never wedges and never silently widens.
    pub(crate) fn refresh_policy(&mut self) -> bool {
        if self.policy_path.as_os_str().is_empty() {
            return false;
        }
        let stamp = policy_stamp(&self.policy_path);
        if stamp == self.policy_stamp {
            return false;
        }
        self.policy_stamp = stamp;
        match read_policy(&self.policy_path) {
            PolicyRead::Loaded(gate) => {
                self.policy_error = None;
                self.clear_start_refusal();
                self.install(*gate);
            }
            PolicyRead::Missing => {
                self.policy_error = None;
                self.clear_start_refusal();
                self.install(Gate::allow_all());
            }
            PolicyRead::Broken(error) => {
                self.policy_error = Some(error.clone());
                let subject = self.policy_path.display().to_string();
                let decision = Decision::refuse(Some(error));
                record_decision(
                    &self.audit,
                    "policy",
                    &subject,
                    &decision,
                    PolicySource::File,
                );
            }
        }
        true
    }

    /// Let the pane start again once a usable policy is in place.
    ///
    /// A start refused because the policy file did not parse is a refusal about
    /// that file, not a verdict on the shell: without this, fixing the file
    /// reloaded the gate but left the pane refusing forever, and only reopening
    /// the tab recovered it. Enter asks the start gate again, so a policy that
    /// still says no is simply refused a second time.
    fn clear_start_refusal(&mut self) {
        if self.refused {
            self.refused = false;
            self.failure = None;
        }
    }

    /// Put a gate in place, keeping the Arc the pane holds.
    fn install(&self, gate: Gate) {
        match self.gate.lock() {
            Ok(mut current) => *current = gate,
            // A panic inside the evaluator poisoned the lock, and every later
            // decision would refuse with no reason the reader could act on. The
            // engine holds no invariant another thread depends on, so the newly
            // read policy replaces it outright - which is what the reload was
            // for. The policy file still decides; nothing is widened by this.
            Err(poisoned) => {
                self.gate.clear_poison();
                *poisoned.into_inner() = gate;
            }
        }
    }

    /// Every decision the gate has made, oldest first, at most AUDIT_LIMIT.
    ///
    /// A test-facing copy of the ring: the pane itself reads the audit through
    /// 'blocked_decisions' and 'last_refusal', which count and search under the
    /// lock instead of copying it on every frame.
    #[cfg(test)]
    pub(crate) fn audit(&self) -> Vec<Audit> {
        self.audit
            .lock()
            .map(|ring| ring.rows.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many decisions refused something.
    ///
    /// Counted under the ring's lock rather than off 'audit()', which clones
    /// every retained row: the pane reads this from its render path, so a full
    /// copy of the ring per frame bought nothing.
    pub(crate) fn blocked_decisions(&self) -> u64 {
        self.audit.lock().map(|ring| ring.blocked).unwrap_or(0)
    }

    /// The last policy that failed to load, when one did.
    pub(crate) fn policy_error(&self) -> Option<&str> {
        self.policy_error.as_deref()
    }

    /// The most recent refused input, for the status line.
    ///
    /// Derived from the audit ring, so what the pane shows and what a test
    /// asserts cannot disagree. A later allowed submit becomes the newest input
    /// row, which clears the line.
    pub(crate) fn last_refusal(&self) -> Option<InputRefusal> {
        let ring = self.audit.lock().ok()?;
        ring.rows
            .iter()
            .rev()
            .find(|row| row.action == "input")
            .filter(|row| !row.allow)
            .map(|row| InputRefusal {
                line: row.subject.clone(),
                reason: row.reason.clone(),
            })
    }

    /// The lines the input gate refused, oldest first.
    #[cfg(test)]
    pub(crate) fn refused_inputs(&self) -> Vec<String> {
        self.audit()
            .into_iter()
            .filter(|row| row.action == "input" && !row.allow)
            .map(|row| row.subject)
            .collect()
    }
}

/// What the pane reports about the spec's answer for its terminal.
///
/// The spec's own answer names a tier only when the sandbox can join the pane's
/// terminal. Ghostty creates the PTY, so that join cannot happen: a spec that
/// would have confined the shell reports "unsupported-pty" and the note says
/// why, instead of claiming a confinement the shell does not have.
fn sandbox_answer(
    lang: Lang,
    spec: Option<&agent::sandbox::SandboxSpec>,
) -> (Option<&'static str>, Option<String>) {
    let Some(spec) = spec else {
        return (None, None);
    };
    match spec.terminal() {
        agent::sandbox::TerminalSandbox::Unconfined => (Some("full-access"), None),
        agent::sandbox::TerminalSandbox::Sandboxed => (
            Some("unsupported-pty"),
            Some(crate::i18n::tr(lang, "right.terminal.ptyNotHostable")),
        ),
        agent::sandbox::TerminalSandbox::PtyNotHostable { .. } => (
            Some("unsupported-pty"),
            spec.terminal().reason().map(str::to_string),
        ),
    }
}

/// One shell word, quoted for the command string Ghostty parses.
fn shell_word(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "/._-:=@+,".contains(ch))
    {
        return word.to_string();
    }
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for ch in word.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// The shell the terminal runs: the reader's own when it is usable, then the
/// shells a macOS install carries, then /bin/sh as the last resort.
pub(crate) fn shell_program() -> PathBuf {
    let reader = std::env::var_os("SHELL")
        .or_else(|| std::env::var_os("COMSPEC"))
        .map(PathBuf::from);
    shell_choice(
        agent::sandbox::TerminalPlatform::current(),
        reader.as_deref(),
        &lookup_program,
    )
}

/// The shells a platform carries, most preferred first.
///
/// A Windows install has no /bin/sh at all, which is why the list is per
/// platform rather than a Unix list with a fallback: "cmd.exe" is the last
/// resort there, and the pane would be dead if it were not.
pub(crate) fn shell_candidates(
    platform: agent::sandbox::TerminalPlatform,
) -> &'static [&'static str] {
    use agent::sandbox::TerminalPlatform;
    match platform {
        TerminalPlatform::MacOs => &["/bin/zsh", "/bin/bash", "/bin/sh"],
        TerminalPlatform::Linux => &["/bin/bash", "/bin/zsh", "/bin/sh"],
        TerminalPlatform::Windows => &["pwsh.exe", "powershell.exe", "cmd.exe"],
        TerminalPlatform::Other => &["/bin/sh"],
    }
}

/// The shell decision itself, with resolution injected so every platform's
/// table can be tested from any one of them.
fn shell_choice(
    platform: agent::sandbox::TerminalPlatform,
    reader: Option<&Path>,
    lookup: &dyn Fn(&str) -> Option<PathBuf>,
) -> PathBuf {
    if let Some(shell) = reader.and_then(|path| {
        let raw = path.to_str()?;
        lookup(raw)
    }) {
        return shell;
    }
    let candidates = shell_candidates(platform);
    for candidate in candidates {
        if let Some(path) = lookup(candidate) {
            return path;
        }
    }
    PathBuf::from(candidates[0])
}

/// Resolve a program the way the platform would: a path must exist, a bare name
/// is searched on PATH.
fn lookup_program(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-terminal")
            .join(format!("{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch workspace");
        dir
    }

    fn write_policy(root: &Path, source: &str) {
        let dir = root.join(".bsh");
        std::fs::create_dir_all(&dir).expect("policy folder");
        std::fs::write(dir.join("terminal-policy.rego"), source).expect("policy file");
    }

    /// Submit one line through the gate, returning the outcome and what the
    /// shell was told.
    fn submit(terminal: &mut Terminal, line: &str) -> (KeyOutcome, Vec<Emission>) {
        terminal.track(Edit::Text(line.to_string()));
        let mut emitted = Vec::new();
        let outcome = terminal.submit(&mut |emission| {
            emitted.push(emission);
            true
        });
        (outcome, emitted)
    }

    #[test]
    fn the_default_gate_allows_the_shell_to_start() {
        let root = scratch("gate-start");
        let terminal = Terminal::new(root, Lang::En);
        assert!(!terminal.refused);
        assert!(terminal.failure.is_none());
    }

    #[test]
    fn a_refused_command_writes_nothing_and_keeps_the_reason() {
        let root = scratch("gate-refuse");
        write_policy(
            &root,
            r#"
package terminal
decision := {"allow": false, "reason": "no rm here"} if {
    input.action == "input"
    contains(input.command, "rm -rf")
}
"#,
        );
        let mut terminal = Terminal::new(root, Lang::En);
        let (outcome, emitted) = submit(&mut terminal, "rm -rf build");
        assert_eq!(outcome, KeyOutcome::Refused);
        assert_eq!(emitted, vec![Emission::KillLine]);
        assert_eq!(
            terminal.last_refusal(),
            Some(InputRefusal {
                line: String::from("rm -rf build"),
                reason: Some(String::from("no rm here")),
            })
        );
        assert_eq!(terminal.blocked_decisions(), 1);
        assert_eq!(
            terminal.refused_inputs(),
            vec![String::from("rm -rf build")]
        );

        let (outcome, emitted) = submit(&mut terminal, "ls");
        assert_eq!(outcome, KeyOutcome::Sent);
        assert_eq!(emitted, vec![Emission::Enter]);
        assert!(terminal.last_refusal().is_none());
    }

    #[test]
    fn the_mirror_follows_editing_keystrokes() {
        let root = scratch("mirror");
        let mut terminal = Terminal::new(root, Lang::En);
        terminal.track(Edit::Text(String::from("git statu")));
        terminal.track(Edit::Backspace);
        terminal.track(Edit::Text(String::from("us")));
        submit(&mut terminal, "");
        assert_eq!(
            terminal.audit().last().map(|row| row.subject.as_str()),
            Some("git status")
        );

        terminal.track(Edit::Text(String::from("git commit foo bar")));
        terminal.track(Edit::KillWord);
        submit(&mut terminal, "");
        assert_eq!(
            terminal.audit().last().map(|row| row.subject.as_str()),
            Some("git commit foo ")
        );

        terminal.track(Edit::Text(String::from("oops")));
        terminal.track(Edit::KillLine);
        submit(&mut terminal, "");
        assert_eq!(
            terminal.audit().last().map(|row| row.subject.as_str()),
            Some("")
        );
    }

    #[test]
    fn a_broken_policy_fails_closed_and_names_the_file() {
        let root = scratch("gate-broken");
        write_policy(&root, "package terminal\nthis is not rego");
        let terminal = Terminal::new(root, Lang::En);
        assert!(terminal.refused, "an unusable rule must not open the door");
        assert!(
            terminal
                .policy_error()
                .is_some_and(|error| error.contains("terminal-policy.rego")),
            "the refusal names the file: {:?}",
            terminal.policy_error()
        );
        assert!(terminal
            .audit()
            .iter()
            .any(|row| row.action == "policy" && !row.allow));
    }

    #[test]
    fn a_changed_policy_takes_effect_without_a_restart() {
        let root = scratch("gate-reload");
        let mut terminal = Terminal::new(root.clone(), Lang::En);
        let (outcome, _) = submit(&mut terminal, "touch x");
        assert_eq!(outcome, KeyOutcome::Sent);

        write_policy(
            &root,
            r#"
package terminal
decision := {"allow": false, "reason": "touched nothing"} if {
    input.action == "input"
}
"#,
        );
        assert!(terminal.refresh_policy());
        let (outcome, emitted) = submit(&mut terminal, "touch y");
        assert_eq!(outcome, KeyOutcome::Refused);
        assert_eq!(emitted, vec![Emission::KillLine]);
    }

    #[test]
    fn a_broken_reload_keeps_the_last_good_policy() {
        let root = scratch("gate-broken-reload");
        write_policy(
            &root,
            r#"
package terminal
decision := {"allow": false, "reason": "kept"} if { input.action == "input" }
"#,
        );
        let mut terminal = Terminal::new(root.clone(), Lang::En);
        let (outcome, _) = submit(&mut terminal, "ls");
        assert_eq!(outcome, KeyOutcome::Refused);

        write_policy(&root, "package terminal\nnonsense");
        assert!(terminal.refresh_policy());
        assert!(terminal.policy_error().is_some());
        let (outcome, _) = submit(&mut terminal, "ls");
        assert_eq!(outcome, KeyOutcome::Refused);
        assert_eq!(
            terminal.last_refusal().and_then(|refusal| refusal.reason),
            Some(String::from("kept")),
            "the last good rule still decides"
        );
    }

    #[test]
    fn a_deny_start_policy_spawns_nothing() {
        let root = scratch("gate-deny-start");
        write_policy(
            &root,
            r#"
package terminal
decision := {"allow": false, "reason": "no shells"} if { input.action == "start" }
"#,
        );
        let terminal = Terminal::new(root, Lang::En);
        assert!(terminal.refused);
        assert_eq!(terminal.failure.as_deref(), Some("no shells"));
    }

    #[test]
    fn the_command_carries_the_sandbox_answer_and_the_shell() {
        let root = scratch("command");
        let mut terminal = Terminal::new(root, Lang::En);
        terminal.sandbox_env = Some("none");
        terminal.shell = PathBuf::from("/bin/zsh");
        let command = terminal.command();
        assert!(command.contains("BSH_SANDBOX=none"), "{command}");
        assert!(command.ends_with("/bin/zsh"), "{command}");
        assert!(command.starts_with("/usr/bin/env "), "{command}");
    }

    #[test]
    fn a_shell_path_with_a_space_is_quoted() {
        let root = scratch("command-quote");
        let mut terminal = Terminal::new(root, Lang::En);
        terminal.sandbox_env = None;
        terminal.shell = PathBuf::from("/Applications/My Shell.app/bin/zsh");
        let command = terminal.command();
        assert!(
            command.ends_with("'/Applications/My Shell.app/bin/zsh'"),
            "{command}"
        );
    }

    #[test]
    fn a_shell_program_is_found() {
        assert!(!shell_program().as_os_str().is_empty());
    }

    fn lookup_map(
        entries: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, path)| PathBuf::from(path))
        }
    }

    #[test]
    fn the_shell_decision_table_covers_all_three_platforms() {
        use agent::sandbox::TerminalPlatform;
        let present = lookup_map(&[
            ("/bin/zsh", "/bin/zsh"),
            ("/bin/bash", "/bin/bash"),
            ("/bin/sh", "/bin/sh"),
        ]);
        assert_eq!(
            shell_choice(
                TerminalPlatform::MacOs,
                Some(Path::new("/bin/bash")),
                &present
            ),
            PathBuf::from("/bin/bash")
        );
        assert_eq!(
            shell_choice(TerminalPlatform::MacOs, Some(Path::new("/gone")), &present),
            PathBuf::from("/bin/zsh"),
            "a reader shell that does not exist falls back to the platform's own"
        );
        let absent = lookup_map(&[]);
        assert_eq!(
            shell_choice(TerminalPlatform::Windows, None, &absent),
            PathBuf::from("pwsh.exe")
        );
    }

    #[test]
    fn the_program_lookup_resolves_paths_and_path_names() {
        assert_eq!(lookup_program("/bin/sh"), Some(PathBuf::from("/bin/sh")));
        assert_eq!(lookup_program("/definitely/not/here"), None);
        assert!(lookup_program("sh").is_some(), "PATH resolution");
    }

    #[test]
    fn the_shell_candidates_are_platform_appropriate() {
        use agent::sandbox::TerminalPlatform;
        assert_eq!(shell_candidates(TerminalPlatform::MacOs)[0], "/bin/zsh");
        assert_eq!(shell_candidates(TerminalPlatform::Linux)[0], "/bin/bash");
        assert_eq!(shell_candidates(TerminalPlatform::Windows)[0], "pwsh.exe");
        assert_eq!(shell_candidates(TerminalPlatform::Other)[0], "/bin/sh");
    }

    #[test]
    fn the_blocked_count_tracks_the_ring_as_it_trims() {
        let root = scratch("audit-count");
        write_policy(
            &root,
            r#"
package terminal
decision := {"allow": false, "reason": "no"} if { input.action == "input" }
"#,
        );
        let mut terminal = Terminal::new(root, Lang::En);
        for _ in 0..(AUDIT_LIMIT + 8) {
            let (outcome, _) = submit(&mut terminal, "nope");
            assert_eq!(outcome, KeyOutcome::Refused);
        }
        assert_eq!(terminal.blocked_decisions(), AUDIT_LIMIT as u64);
        assert_eq!(terminal.audit().len(), AUDIT_LIMIT);
    }

    #[test]
    fn a_fixed_policy_lets_the_pane_start_again() {
        let root = scratch("policy-fixed");
        write_policy(&root, "package terminal\nbroken");
        let mut terminal = Terminal::new(root.clone(), Lang::En);
        assert!(terminal.refused);
        write_policy(&root, "package terminal\ndefault allow := true\n");
        assert!(terminal.refresh_policy());
        assert!(!terminal.refused, "a reload clears the start refusal");
        assert!(terminal.restart());
    }

    #[test]
    fn the_audit_caps_a_long_reason() {
        let root = scratch("audit-cap");
        let reason = "x".repeat(2000);
        write_policy(
            &root,
            &format!(
                r#"
package terminal
decision := {{"allow": false, "reason": "{reason}"}} if {{ input.action == "input" }}
"#
            ),
        );
        let mut terminal = Terminal::new(root, Lang::En);
        submit(&mut terminal, "ls");
        let row = terminal.audit().pop().expect("one row");
        assert_eq!(
            row.reason.map(|reason| reason.len()),
            Some(AUDIT_REASON_CAP)
        );
    }

    #[test]
    fn a_missing_policy_is_written_from_the_template() {
        let root = scratch("seed");
        let _ = Terminal::new(root.clone(), Lang::En);
        let text = std::fs::read_to_string(root.join(".bsh/terminal-policy.rego"))
            .expect("the starter policy");
        assert_eq!(text, WORKSPACE_POLICY_TEMPLATE);
    }
}
