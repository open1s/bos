//! The shell state: store, settings, active session and streaming bookkeeping.
//!
//! Framework-free: GPUI owns a value of this struct, but nothing here knows
//! that. Every mutation the shell makes goes through a method, so the render
//! pass only reads.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent::goal::GoalState;
use agent::SteerMailbox;

use crate::i18n::Lang;
use crate::model::{
    ChatMessage, CompactCommand, CompactOutcome, CompactionReport, MessageMeta, MessageUsage,
    PlanItem, PlanStore, Role, Session, ToolEvent,
};
use crate::runner::{AgentEvent, Runner};
use crate::settings::Settings;
use crate::store::Store;

/// Whether an allowlist line is an address or a CIDR range.
fn valid_egress_entry(entry: &str) -> bool {
    if entry.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let Some((address, bits)) = entry.split_once('/') else {
        return false;
    };
    let max_bits = if address.parse::<std::net::Ipv4Addr>().is_ok() {
        32
    } else {
        128
    };
    address.parse::<std::net::IpAddr>().is_ok()
        && bits.parse::<u8>().is_ok_and(|bits| bits <= max_bits)
}

/// Wall-clock milliseconds since the Unix epoch, for the trajectory timeline.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record when a turn's first visible output arrived, once.
fn note_first_output(message: &mut ChatMessage, at: u64) {
    message.timing_mut().first_output.get_or_insert(at);
}

/// Close a turn's measured span, when it has a start and no end yet.
fn settle_ms(message: &mut ChatMessage, at: u64) {
    let Some(start) = message.timing().and_then(|timing| timing.start) else {
        return;
    };
    if message.timing().and_then(|timing| timing.ms).is_some() {
        return;
    }
    message.timing_mut().ms = Some(at.saturating_sub(start));
}

/// Wall-clock seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Format a past instant as a UTC date and time.
pub(crate) fn stamp(at: u64) -> String {
    let days = (at / 86_400) as i64;
    let seconds = at % 86_400;
    // Howard Hinnant's civil-from-days, shifted to the 1970 epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60
    )
}

/// The local calendar reading of a Unix instant: year, month, day, hour, minute.
///
/// LOCAL, not UTC. A message clock is read against the reader's own wall clock,
/// while `stamp()`'s UTC reading answers a different question - when the record
/// was written - and the two must not be confused.
///
/// `libc` rather than a date crate: the workspace already builds libc for its
/// terminal, so this costs no dependency, and the only thing needed here is the
/// host's own offset.
#[cfg(unix)]
fn local_parts(at: u64) -> (i32, u32, u32, u32, u32) {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let stamp = at as libc::time_t;
    // SAFETY: `localtime_r` writes into the `tm` this frame owns and reads only
    // the instant; a null return means it refused, which falls back to UTC.
    let ok = unsafe { !libc::localtime_r(&stamp, &mut tm).is_null() };
    if !ok {
        return utc_parts(at);
    }
    (
        tm.tm_year + 1900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
    )
}

/// The UTC calendar reading, for a platform without a local clock.
#[cfg(not(unix))]
fn local_parts(at: u64) -> (i32, u32, u32, u32, u32) {
    utc_parts(at)
}

/// The UTC calendar reading of a Unix instant.
fn utc_parts(at: u64) -> (i32, u32, u32, u32, u32) {
    let days = (at / 86_400) as i64;
    let seconds = at % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y as i32,
        m as u32,
        d as u32,
        (seconds / 3600) as u32,
        ((seconds % 3600) / 60) as u32,
    )
}

/// Format an instant as the reference's message clock.
///
/// Measured from `formatMessageClock`: `HH:MM` for today, `clock.md` plus that
/// clock for another day in this year, and `clock.ymd` plus it otherwise. The
/// reference keeps the day boundary in a hook that re-renders at local midnight;
/// bsh reads the clock on each paint, so a session left open overnight moves to
/// the dated form on its own next frame.
pub(crate) fn clock_at(at: u64, lang: Lang) -> String {
    let (year, month, day, hour, minute) = local_parts(at);
    let (today_year, today_month, today_day, ..) = local_parts(now());
    let clock = format!("{hour:02}:{minute:02}");
    if (year, month, day) == (today_year, today_month, today_day) {
        return clock;
    }
    let template = if year == today_year {
        crate::i18n::tr(lang, "clock.md")
    } else {
        crate::i18n::tr(lang, "clock.ymd")
    };
    // The reference pads neither `m` nor `d`, so a March 9th reads `3/9`.
    let dated = template
        .replace("{y}", &year.to_string())
        .replace("{m}", &month.to_string())
        .replace("{d}", &day.to_string());
    format!("{dated} {clock}")
}

/// Format a past instant as a short age.
pub(crate) fn age(then: u64) -> String {
    let now = now();
    if then == 0 || then > now {
        return String::new();
    }
    let secs = now - then;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// How many trust decisions the audit ring keeps.
pub(crate) const TRUST_AUDIT_LIMIT: usize = 64;

/// One recorded trust decision about an installed extension's commands.
///
/// The record names the package, the directory it was trusted at, the exact
/// command line(s) the decision covers, when it was made and whether it was an
/// approval. Environment VALUES never enter it: a command is the executable and
/// its arguments, never the child's environment.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TrustDecision {
    /// The settings id of the package.
    pub(crate) extension_id: String,
    /// The package directory at the time of the decision.
    pub(crate) path: String,
    /// The exact command line(s) the decision covers.
    pub(crate) commands: Vec<String>,
    /// Unix seconds the decision was made.
    pub(crate) at: u64,
    /// Whether the reader approved the commands.
    pub(crate) allow: bool,
}

/// A full-window surface layered over the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Overlay {
    /// The shell alone.
    #[default]
    None,
    /// The settings dialog.
    Settings,
}

/// The main column's tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    /// The conversation.
    #[default]
    Chat,
    /// The plan and tool trajectory.
    Trajectory,
    /// The deterministic tool-call and refusal audit.
    Audit,
}

/// One message waiting for its session's running turn to finish.
///
/// The reference keeps these in the session's durable inbox over the Host. bsh
/// has no such service, so a queued message lives with the shell for as long as
/// the app runs: it is taken off the front when the turn it waited for ends and
/// becomes the next turn's own input.
#[derive(Clone)]
pub(crate) struct Queued {
    /// Identity, stable for as long as the row is queued.
    ///
    /// A row's own edit and remove name this rather than a position, so an
    /// action can never land on a neighbouring message.
    pub(crate) seq: u64,
    /// The text the reader submitted.
    pub(crate) text: String,
    /// Images submitted with it, sent as content parts on the next turn.
    pub(crate) attachments: Vec<Attachment>,
}

/// One queued row taken off the dock to steer the running turn.
///
/// It travels to the agent as a ticket and waits here until the turn either
/// takes it (the transcript records it) or ends first (the row goes back).
#[derive(Clone)]
pub(crate) struct Steering {
    /// The session whose running turn it was steered into.
    pub(crate) session: String,
    /// The row exactly as it was queued, so a withdrawal can restore it in
    /// its old place.
    pub(crate) queued: Queued,
}

/// The last policy audit the shell drew, kept outside the policy's own locks.
#[derive(Clone, Default)]
pub(crate) struct PolicyAuditView {
    /// The bounded denial ring, oldest first.
    pub(crate) denials: Vec<agent::policy::PolicyAuditEntry>,
    /// Allowed decisions, counted per action.
    pub(crate) allows: agent::policy::AllowCounts,
    /// Every denial ever counted, including ones the ring dropped.
    pub(crate) denials_total: u64,
    /// "bytecode" or "interpreter".
    pub(crate) evaluator: String,
    /// The effective permission the policy applies, as its data carries it.
    ///
    /// The audit page shows this beside the decision counts: a refusal's own
    /// row keeps the permission in force at that decision, while this is the
    /// one currently applied to the workspace.
    pub(crate) permission: String,
    /// The agent preset the policy was built for, as its data carries it.
    ///
    /// `PolicyData.preset` is `Effective.preset_id`, so this is the same value
    /// the composer's preset chip and the audit's applied line read. The
    /// trajectory's policy tab takes it from here rather than inferring one
    /// from a tool name, because a `ToolEvent` carries no preset field.
    pub(crate) preset: String,
    /// Why the set is in its failure mode, when it is.
    pub(crate) error: Option<String>,
}

/// The subagent currently rendered by the chat/trajectory view model.
///
/// It owns a transient Session, exactly the model a normal conversation uses,
/// plus the navigation facts (which run, and the parent's title). The renderers
/// never learn that the transcript came from a child.
pub(crate) struct SubagentView {
    /// Registry id of the run.
    pub(crate) id: String,
    /// The root conversation's title.
    pub(crate) parent_title: String,
    /// The child transcript presented as a Session.
    pub(crate) session: Session,
}

/// Rebuild the in-memory run registry from the child Sessions already on disk.
///
/// A child is saved exactly like a parent conversation: a JSON document with
/// its SubagentMeta and a Parquet transcript log. On startup each such Session
/// is rehydrated, flattened back into the registry rows and restored into the
/// menu; no separate subagent index file exists.
fn restore_subagents(
    store: &mut Store,
    registry: &Arc<crate::subagent::SubagentRegistry>,
) -> HashMap<String, (crate::subagent::SubagentStatus, u64, Option<u64>)> {
    let mut persisted = HashMap::new();
    for id in store.subagent_sessions() {
        store.rehydrate(&id);
        let Some(session) = store.session(&id) else {
            continue;
        };
        let Some(meta) = session.subagent.clone() else {
            continue;
        };
        let title = session.title.clone();
        let transcript = crate::subagent::SubagentRegistry::messages_from_chat(&session.messages);
        let result = meta.result.clone().or_else(|| {
            transcript
                .iter()
                .rev()
                .find(|row| row.role == "assistant")
                .map(|row| row.content.clone())
        });
        let mut status = crate::subagent::SubagentStatus::from_name(&meta.status);
        let mut error = meta.error;
        // No process survives the restart, so a row saved mid-turn is finished
        // with an honest interruption rather than a spinner that never stops.
        if status == crate::subagent::SubagentStatus::Running {
            status = crate::subagent::SubagentStatus::Failed;
            error = Some("interrupted by restart".to_string());
        }
        let run = crate::subagent::SubagentRun {
            id: id.clone(),
            session: meta.root_id,
            parent: meta.parent_id,
            depth: meta.depth,
            title,
            prompt: meta.prompt,
            status,
            started_at: meta.started_at,
            finished_at: meta.finished_at,
            estimated_tokens: meta.estimated_tokens,
            prompt_tokens: meta.prompt_tokens,
            completion_tokens: meta.completion_tokens,
            result,
            error,
            revision: 0,
            transcript,
        };
        persisted.insert(id, (run.status, run.revision, run.finished_at));
        registry.restore(run);
    }
    persisted
}

/// Everything the shell renders from.
pub(crate) struct AppState {
    /// Persisted sessions.
    pub(crate) store: Store,
    /// Persisted settings.
    pub(crate) settings: Settings,
    /// Interface language.
    pub(crate) lang: Lang,
    /// The selected session.
    pub(crate) active_id: Option<String>,
    /// Workspaces whose session list is folded.
    pub(crate) collapsed: BTreeSet<String>,
    /// Session id to the instant its stream started.
    pub(crate) streams: HashMap<String, u64>,
    /// The live background-job roster, shared by the shell tools and the header.
    ///
    /// It is created once per app rather than per turn, so a job a previous
    /// turn started stays readable and killable by the next one and by the
    /// popover.
    pub(crate) jobs: Arc<agent::jobs::JobRegistry>,
    /// Whether the session-name search field is showing.
    pub(crate) search_open: bool,
    /// The session-name filter; empty matches every session.
    pub(crate) search_query: String,
    /// The plugin-name filter on the plugin list pane.
    pub(crate) plugin_query: String,
    /// The model-id filter on the fetched-model picker.
    pub(crate) picker_query: String,
    /// Images attached to the next turn, sent as multimodal content.
    pub(crate) attachments: Vec<Attachment>,

    /// The live plan store of every session whose turn is still writing.
    ///
    /// The agent shares this store with its `update_plan` tool, so a running
    /// turn's plan is read from here and the finished copy is saved onto the
    /// session it belongs to.
    plans: HashMap<String, PlanStore>,
    /// Child runs spawned by any turn, shared with the header and the tool.
    pub(crate) subagents: Arc<crate::subagent::SubagentRegistry>,
    /// The selected child, presented as the same Session model the chat uses.
    pub(crate) subagent: Option<SubagentView>,
    /// The last (status, transcript length, finish time) written for each run.
    persisted_subagents: HashMap<String, (crate::subagent::SubagentStatus, u64, Option<u64>)>,
    /// Messages waiting for a running session's own turn to end, oldest first.
    ///
    /// The reference drains one per finished turn; bsh starts the next turn from
    /// the front of this queue, which is what keeps a submission made while the
    /// agent runs from opening a second stream over the same transcript.
    inbox: HashMap<String, Vec<Queued>>,
    /// Identity for the next queued row; see [`Queued::seq`].
    inbox_seq: u64,
    /// The steer mailbox of each session whose turn is running.
    ///
    /// Attached when the turn is spawned and dropped when it ends, so a
    /// control can only steer a LIVE turn: an idle session's queued row is an
    /// ordinary send, not a message into a mailbox nobody drains.
    steer_mailboxes: HashMap<String, Arc<SteerMailbox>>,
    /// Messages taken off a queue to steer, by ticket.
    ///
    /// A message is removed from the queue the moment the reader asks for it
    /// (the row must leave the dock now), but the turn may end before its next
    /// step takes it. The row is kept here until one of those two happens: the
    /// delivery event records it in the transcript, the withdrawal event puts
    /// it back in the queue, and nothing is silently lost.
    steering: HashMap<u64, Steering>,
    /// Sessions whose `/compact` command is in flight right now.
    ///
    /// A compaction is not a stream, so the streaming registry cannot answer
    /// "is one already running"; the reference's durable lock plays that role
    /// there and reports `busy`. This is the same admission flag for the
    /// manual command, and it is runtime state only - a crash leaves no lock
    /// behind because the next launch starts with nothing in flight.
    compacting: std::collections::HashSet<String>,
    /// The streaming runtime.
    pub(crate) runner: Runner,
    /// The live MCP connections every agent build shares.
    ///
    /// A server is a process or an endpoint, so connecting is expensive and
    /// must survive across turns; the pool reconciles when the settings change.
    pub(crate) mcp: crate::mcp::McpPool,
    /// The bounded ring of trust decisions, oldest first.
    ///
    /// This crate has no extension-wide audit surface, so the trust gate keeps
    /// its own small record beside the state it guards. Env values never enter
    /// it: only the command line, the package and its path.
    trust_audit: std::collections::VecDeque<TrustDecision>,
    /// A transient line of feedback.
    pub(crate) notice: Option<String>,
    /// Whether the usage pill's dialog is open.
    pub(crate) stat_open: bool,
    /// Whether the context meter's panel is open.
    pub(crate) meter_open: bool,
    /// Serialized length of the tool definitions the last built agent carried.
    ///
    /// The meter's breakdown prices the request's tool schemas from exactly this
    /// array, so its figure follows the agent the next turn will run.
    pub(crate) tools_schema_chars: usize,
    /// The policy of the most recent build, kept so the audit pane can read it.
    pub(crate) policy: Option<Arc<agent::policy::PolicySet>>,
    /// The audit snapshot the policy pane last read.
    pub(crate) policy_audit: Option<PolicyAuditView>,
    /// Egress allowlist entries read for the policy pane.
    pub(crate) egress_allowlist: Vec<String>,
    /// The locale key of the standing provider-form refusal, if any.
    pub(crate) provider_refusal: Option<&'static str>,
    /// The name of the provider the form is editing, if any.
    pub(crate) editing_provider: Option<String>,
    /// The provider awaiting a delete confirmation, if any.
    pub(crate) removing_provider: Option<String>,
    /// The open overlay.
    pub(crate) overlay: Overlay,
    /// The active main-column tab.
    pub(crate) tab: Tab,
    /// Tool rows the reader has opened, keyed "message:tool".
    pub(crate) expanded_tools: BTreeSet<String>,
    /// Settings section selected in the dialog.
    pub(crate) settings_section: usize,
    /// The settings being edited; None when the dialog is closed.
    pub(crate) draft: Option<Settings>,
    /// The admin-key vault. It reopens at launch from the app-access sidecar
    /// when that file exists, so provider entries resolve without the admin
    /// passphrase while the Vault pane stays locked; a missing or stale sidecar
    /// leaves the app without access until the vault is unlocked once.
    pub(crate) vault: crate::vault::Vault,
    /// Whether the sidebar is folded away. This is the reference's stored
    /// layoutInfo.sidebar === 0 sentinel, which only governs a wide viewport.
    pub(crate) sidebar_collapsed: bool,
    /// The reference's narrowExpanded: a manual expand below the auto-collapse
    /// breakpoint, which is runtime state and is never stored.
    pub(crate) narrow_expanded: bool,
    /// When the streaming turn started, for the elapsed-time line.
    pub(crate) turn_started: Option<std::time::Instant>,
    /// The session tree view option.
    pub(crate) sort: crate::store::SessionSort,
    /// The context meter's message bucket, priced up to the last message.
    ///
    /// The panel prices the whole transcript, and while it is open a frame
    /// used to re-count every character of every message. Only the LAST
    /// message can change without the window moving (a stream writes into it),
    /// so the prefix is priced once and the tail is priced per frame.
    breakdown_cache: std::cell::RefCell<BreakdownCache>,
}

/// The priced prefix of the context meter's message bucket.
#[derive(Default)]
struct BreakdownCache {
    session: String,
    /// Where the priced window starts in the loaded window.
    compacted: usize,
    /// The global index of the window's first row when that start was priced;
    /// a paging step moves the window under the same local start.
    from: usize,
    /// The number of messages whose price is in `prefix_total`; the message at
    /// this index is the tail that is priced fresh.
    priced: usize,
    /// How many compaction records were folded into `checkpoint_total`.
    checkpoints: usize,
    /// The framed checkpoint the request carries ahead of the window.
    checkpoint_total: u64,
    prefix_total: u64,
}

/// Why one goal mutation was refused.
///
/// The reference maps every GoalError variant onto one of two direct command
/// errors; these variants are the distinction the command needs to choose
/// between them (a missing goal names the verb, every other rejection is the
/// generic invalid-for-this-state message).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalRefusal {
    /// No current goal for an operation that requires one.
    Missing,
    /// A create over an unfinished goal, which must be edited or cleared first.
    AlreadyExists(GoalState),
    /// The phase, activation or round budget refuses the transition.
    InvalidTransition,
    /// The objective trims to nothing.
    InvalidObjective,
}

/// Which goal mutation a verb performed.
///
/// Only the edit path needs the distinction: the reference reports an edit of a
/// completed goal as a CREATE with a fresh identity, not as an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalChange {
    /// A goal was created where there was none.
    Created,
    /// A completed goal was replaced with a fresh identity.
    Replaced,
    /// The objective changed in place, keeping phase and rounds.
    Updated,
}

/// The compaction record whose checkpoint the next request replays.
///
/// Only the NEWEST checkpoint survives on the surface: a later compaction can
/// fold an earlier checkpoint into a fresh summary, so replaying an older one
/// would duplicate history the model already condensed. A session whose boundary
/// moved without a record (a hand-edited or migrated file) has no summary to
/// replay and falls back to the plain window.
fn replayed_checkpoint(session: &Session) -> Option<&crate::model::CompactionRecord> {
    if session.compacted == 0 {
        return None;
    }
    session.compactions.last()
}

/// The exact user-role message one compaction checkpoint replays as.
///
/// The framing is the shipped backend's: the preamble, a blank line, the
/// `<compacted-summary>` tags and the model-written body. Using the agent's own
/// renderer keeps the replay byte-identical to what the summarizer installed.
fn replayed_checkpoint_message(record: &crate::model::CompactionRecord) -> ChatMessage {
    ChatMessage::user(agent::agent::context::render_checkpoint(&record.summary))
}

/// The transcript index a compaction marker sorts at, in the LOADED window.
///
/// The reference anchors it at the checkpoint event; bsh persists the GLOBAL
/// transcript slot at commit (`after`). The window can start at `loaded_from`
/// after a restart or a paging step, so the slot is mapped into it. A legacy
/// record (`after == 0`) has no slot to honour, so it draws at the end - the
/// position the reader looks at right after running the command - rather than
/// at the very top, which is where `after`'s zero would otherwise sort it.
pub(crate) fn compaction_marker_position(
    record: &crate::model::CompactionRecord,
    len: usize,
    loaded_from: usize,
) -> usize {
    if record.after == 0 {
        len
    } else {
        record.after.saturating_sub(loaded_from).min(len)
    }
}

/// Where the next request's stored tail starts in the loaded window.
///
/// `Session.compacted` is a GLOBAL transcript index. The in-memory window can
/// start at `loaded_from` (a restart loads only the tail; paging moves the
/// window), so treating the global index as a local slice offset would elide
/// rows the reader still has - or keep rows that were already folded. A
/// boundary above the window keeps the whole window; one past its end keeps
/// nothing, leaving the checkpoint alone.
pub(crate) fn request_start(session: &Session) -> usize {
    session
        .compacted
        .saturating_sub(session.loaded_from)
        .min(session.messages.len())
}

/// The transcript slot a checkpoint event occupies.
///
/// The reference appends the checkpoint user/message after every event that
/// exists at the moment it lands, so the marker belongs after the last REAL
/// message - not after an empty assistant placeholder a live turn is about to
/// fill. Indexing by the last row that carries text, reasoning, tools or images
/// keeps a streaming reply BELOW the marker it follows, and puts an idle
/// compaction at the end where the command was issued.
fn checkpoint_slot(session: &Session) -> usize {
    session
        .messages
        .iter()
        .rposition(|message| {
            !message.text.is_empty()
                || !message.reasoning.is_empty()
                || !message.tools.is_empty()
                || !message.images.is_empty()
        })
        .map_or(0, |index| index + 1)
}

impl AppState {
    /// Load the store and settings, guaranteeing an active session.
    pub(crate) fn new() -> Self {
        let settings = Settings::load();
        let mut store = Store::open(&session_roots(&settings)).with_secrets(settings.secrets());
        // A session whose workspace the registry no longer names is loaded and
        // then filtered out of every sidebar row, so it is history the reader
        // cannot reach. Attaching it to the workspace in use is the only thing
        // that can happen to it.
        store.rehome_orphans(&settings.global.general.active_workspace);
        Self::assemble(store, settings)
    }

    /// Build the state over an explicit store and the shipped defaults.
    ///
    /// Test-only: a scratch store is how a test keeps real session files out of
    /// the user home, and `Settings::default()` is how it keeps that machine's
    /// own configuration out of the assertions. Reading the real file here made
    /// a measured copy assertion depend on the developer's language and context
    /// budget - a green suite on one machine and a red one on the next.
    /// `new` reads the settings first, because it needs the workspace registry
    /// to know where the store is rooted, so it cannot come through here
    /// without parsing the file twice.
    #[cfg(test)]
    pub(crate) fn with_store(store: Store) -> Self {
        Self::assemble(store, Settings::default())
    }

    /// Build the state from a store and the settings already in hand.
    fn assemble(mut store: Store, settings: Settings) -> Self {
        let lang = Lang::from_locale(&settings.global.general.lang);
        let subagents = Arc::new(crate::subagent::SubagentRegistry::default());
        let persisted_subagents = restore_subagents(&mut store, &subagents);
        let mut state = Self {
            store,
            settings,
            lang,
            sort: crate::store::SessionSort::default(),

            active_id: None,
            collapsed: BTreeSet::new(),
            streams: HashMap::new(),
            jobs: Arc::new(agent::jobs::JobRegistry::new()),
            plans: HashMap::new(),
            subagents,
            subagent: None,
            persisted_subagents,
            inbox: HashMap::new(),
            inbox_seq: 0,
            steer_mailboxes: HashMap::new(),
            steering: HashMap::new(),
            search_open: false,
            search_query: String::new(),
            plugin_query: String::new(),
            picker_query: String::new(),
            attachments: Vec::new(),
            compacting: std::collections::HashSet::new(),
            runner: Runner::new(),
            mcp: crate::mcp::McpPool::default(),
            trust_audit: std::collections::VecDeque::new(),
            notice: None,
            stat_open: false,
            meter_open: false,
            tools_schema_chars: 0,
            policy: None,
            policy_audit: None,
            egress_allowlist: Vec::new(),
            provider_refusal: None,
            editing_provider: None,
            removing_provider: None,
            overlay: Overlay::None,
            tab: Tab::Chat,
            expanded_tools: BTreeSet::new(),
            settings_section: 0,
            draft: None,
            // The path is settled after the scratch redirect below, because a
            // scratch store is a scratch home.
            vault: crate::vault::Vault::disabled(),
            sidebar_collapsed: false,
            narrow_expanded: false,
            turn_started: None,
            breakdown_cache: std::cell::RefCell::new(BreakdownCache::default()),
        };
        // The registry is a fact about the launch, so it is settled before
        // anything that depends on it reads it - a restored session is tagged
        // with a workspace id, and the panes that show a workspace resolve that
        // id through the registry.
        state.settings.ensure_workspace();
        // A scratch store is a scratch home: the settings a test moves are
        // written beside the sessions it makes, never over the user's real
        // configuration.
        if state.store.is_scratch() {
            state.settings.save_root = Some(state.store.root().to_path_buf());
            // A scratch home is a scratch WORKSPACE too. The first workspace a
            // test registers comes from the process's cwd - the crate directory
            // - and the workspace document follows the registry, so a save wrote
            // a `.bsh/settings/settings.toml` into the checkout a reader owns.
            // Both documents land in the scratch tree now.
            let scratch = state.store.root().to_string_lossy().to_string();
            for workspace in &mut state.settings.global.workspaces {
                workspace.root = scratch.clone();
            }
        }
        // The vault lives beside the document this state writes, which is the
        // scratch store for a test and the bsh data directory for the app. It
        // starts LOCKED, whether or not the file exists.
        state.vault = crate::vault::Vault::new(state.settings.vault_path());
        // Installed packages may contribute model providers. This is settled
        // before anything reads the provider tree - the composer's model menu,
        // the Models pane and every build - so a contributed endpoint is
        // present from the first frame rather than appearing after a refresh.
        crate::caps::refresh_plugin_providers(&mut state.settings);
        state.ensure_active();
        state.refresh_egress_allowlist();
        state
    }

    /// Guarantee a valid active session, creating one when the store is empty.
    pub(crate) fn ensure_active(&mut self) {
        let valid = self
            .active_id
            .as_ref()
            .map(|id| self.store.session(id).is_some())
            .unwrap_or(false);
        if !valid {
            self.active_id = self
                .store
                .sessions
                .iter()
                .find(|s| s.archived_at.is_none())
                .map(|s| s.id.clone());
        }
        if self.active_id.is_none() {
            self.new_session();
        }
        if let Some(id) = self.active_id.clone() {
            self.store.rehydrate(&id);
        }
    }

    /// The selected session.
    pub(crate) fn active_session(&self) -> Option<&Session> {
        if let Some(subagent) = &self.subagent {
            return Some(&subagent.session);
        }
        self.active_id
            .as_ref()
            .and_then(|id| self.store.session(id))
    }

    /// What one session contributes to model resolution.
    ///
    /// A session carries its own (provider, nickname) pair, so two chats in the
    /// same workspace can run on different models. The pair is a reference into
    /// the global tree and never a copy of the endpoint. It is the ONLY model
    /// selection: a session that names none has no model and is refused at send
    /// rather than inheriting one from somewhere else.
    pub(crate) fn session_choice(&self, id: &str) -> crate::settings::resolve::SessionChoice {
        self.store
            .session(id)
            .map(Session::choice)
            .unwrap_or_default()
    }

    /// The values a build in the given session uses.
    ///
    /// The session's own choice, resolved against the global tree - one order,
    /// read by the chip, the menu, the send gate and the agent.
    pub(crate) fn effective_in(&self, id: &str) -> crate::settings::Effective {
        self.settings.effective_for(&self.session_choice(id))
    }

    /// The values a build for the session on screen uses.
    ///
    /// No session at all reads as no choice, which is also what a session that
    /// never chose gets: an unchosen model, never a silent fallback.
    pub(crate) fn effective(&self) -> crate::settings::Effective {
        match self.active_id.as_deref() {
            Some(id) => self.effective_in(id),
            None => self.settings.effective(),
        }
    }

    /// Move a legacy workspace model pair onto the session that is open.
    ///
    /// Settings written before the choice became session-scoped carried the
    /// pair under the workspace's `[general]`. That document is deserialize-
    /// only now: this reads the pair ONCE, gives it to the session on screen,
    /// saves that session, and clears the in-memory copy so no later session
    /// can pick it up. A session that already chose keeps its own choice.
    ///
    /// Returns whether a session took the pair.
    pub(crate) fn adopt_legacy_model(&mut self) -> bool {
        let pair = self.settings.workspace.general.legacy_pair();
        // Consumed either way: the pair describes the session that was open
        // when the file was written, and that decision is made once.
        self.settings.workspace.general.legacy_provider.clear();
        self.settings.workspace.general.legacy_model.clear();
        let (Some(pair), Some(id)) = (pair, self.active_id.clone()) else {
            return false;
        };
        let Some(session) = self.store.session_mut(&id) else {
            return false;
        };
        if session.provider.is_some() || session.model.is_some() {
            return false;
        }
        session.provider = Some(pair.provider);
        session.model = Some(pair.model);
        self.save_session(&id);
        true
    }

    /// Whether the reference draws its hero for the active conversation.
    ///
    /// Measured: the hero replaces the transcript while a session exists and
    /// has no turns, so the composer is centred and carries the hero's own
    /// placeholder.
    pub(crate) fn hero_phase(&self) -> bool {
        self.tab == Tab::Chat
            && self
                .active_session()
                .map(|session| session.messages.is_empty())
                .unwrap_or(false)
    }

    /// Create a chat in the active workspace and select it.
    pub(crate) fn new_session(&mut self) -> Option<String> {
        self.settings.ensure_workspace();
        // A session is tagged with the workspace's stable id, which is the key
        // the sidebar and the restore filter both read. Tagging it with the
        // root would make every new session invisible to both.
        let workspace = self
            .settings
            .active_workspace()
            .map(|record| record.id.clone())
            .unwrap_or_default();
        let session = Session::new(
            workspace.as_str(),
            crate::i18n::tr(self.lang, "row.newChat").as_str(),
        );
        let id = session.id.clone();
        let _ = self.store.save(&session);
        self.store.insert(session);
        self.active_id = Some(id.clone());
        self.subagent = None;
        Some(id)
    }

    /// Reopen the session the user was last in, else the newest one here.
    ///
    /// The remembered id is a machine preference, so it can name a chat in a
    /// workspace the reader has since left: the restore only accepts it while it
    /// still belongs to the active workspace. Otherwise the newest visible chat
    /// here wins. Several sessions may be running; this only decides which one
    /// the window shows at launch.
    pub(crate) fn restore_session(&mut self) {
        let remembered = self
            .settings
            .global
            .general
            .active_session
            .trim()
            .to_string();
        let archived: Vec<String> = self
            .store
            .archived()
            .into_iter()
            .map(|session| session.id.clone())
            .collect();
        let workspace = self
            .settings
            .active_workspace()
            .map(|record| record.id.clone())
            .unwrap_or_default();
        if !remembered.is_empty()
            && !archived.contains(&remembered)
            && self
                .store
                .session(&remembered)
                .is_some_and(|session| session.workspace == workspace)
        {
            self.select(&remembered);
            return;
        }
        let newest = self
            .store
            .in_workspace(&workspace)
            .into_iter()
            .filter(|session| !archived.contains(&session.id))
            .max_by_key(|session| session.updated_at)
            .map(|session| session.id.clone());
        if let Some(id) = newest {
            self.select(&id);
        }
    }

    /// Reconcile the MCP pool with the servers the settings name.
    ///
    /// Connecting is a process spawn or an HTTP handshake, so it happens when
    /// the configuration changes or at launch, never per frame. Servers whose
    /// configuration is unchanged keep their live connection.
    pub(crate) fn refresh_mcp(&mut self) {
        // The MCP HTTP path is the tool boundary's network egress. Install the
        // same policy the turn will use BEFORE the first handshake, so a host
        // the allowlist does not name opens no socket here either.
        let choice = self
            .active_id
            .as_deref()
            .map(|id| self.session_choice(id))
            .unwrap_or_default();
        let eff = self.settings.effective_for(&choice);
        let policy = crate::caps::policy_for_effective(&self.settings, &eff);
        self.mcp
            .set_egress_gate(std::sync::Arc::new(agent::policy::EgressGate::new(
                policy,
                std::sync::Arc::new(agent::policy::SystemResolver),
                self.active_id.clone().unwrap_or_default(),
                eff.preset_id.clone(),
                "mcp",
            )));
        let servers = self.settings.mcp_servers();
        let extensions = crate::mcp::extension_servers(&self.settings);
        let failed = self.mcp.sync_all(self.runner.rt(), &servers, &extensions);
        if !failed.is_empty() {
            self.notice(crate::i18n::tr_args(
                self.lang,
                "settings.mcpConnectFailed",
                &[("names", failed.join(", "))],
            ));
        }
    }

    /// Show a session in the window.
    ///
    /// This is a view change, not a runtime one: another session's turn keeps
    /// streaming, and the remembered id is a machine preference, so only the
    /// global document is rewritten.
    pub(crate) fn select(&mut self, id: &str) {
        if self
            .store
            .session(id)
            .is_some_and(|session| session.subagent.is_some())
        {
            self.show_subagent(id);
            self.tab = Tab::Chat;
            return;
        }
        if self.store.session(id).is_some() {
            self.store.rehydrate(id);
            self.active_id = Some(id.to_string());
            self.subagent = None;
            self.tab = Tab::Chat;
            self.settings.global.general.active_session = id.to_string();
            let _ = self.settings.save_global();
        }
    }

    /// Move the active chat into another workspace.
    ///
    /// The reference's hero workspace picker reassigns the blank session it
    /// sits under; bsh keeps the same session and moves its owning workspace,
    /// because the chat the editor sits over already exists.
    pub(crate) fn set_workspace(&mut self, workspace: &str) {
        self.settings.global.general.active_workspace = workspace.to_string();
        if let Some(id) = self.active_id.clone() {
            if let Some(session) = self.store.session_mut(&id) {
                session.workspace = workspace.to_string();
            }
            self.save_session(&id);
        }
        let _ = self.settings.save();
        // The workspace document also names MCP servers, so the effective
        // list just changed.
        self.refresh_mcp();
    }

    /// Re-read the workspace registry into the store's root map.
    ///
    /// Called after the registry changes: adding a workspace, deleting one. The
    /// settings tree is the truth; the store's map only says where the files go.
    fn sync_store_roots(&mut self) {
        let pairs: Vec<(String, String)> = self
            .settings
            .global
            .workspaces
            .iter()
            .map(|workspace| (workspace.id.clone(), workspace.root.clone()))
            .collect();
        self.store.set_roots(&pairs);
    }

    /// Register a directory as a workspace, keeping the store in step.
    pub(crate) fn adopt_workspace(&mut self, name: &str, root: &str) -> String {
        let id = self.settings.note_workspace(name, root);
        self.sync_store_roots();
        id
    }

    /// Retitle a workspace.
    ///
    /// A rename is the display name alone: the id is the key every session is
    /// tagged with and the root stays where it is, so nothing moves and no
    /// transcript is touched.
    pub(crate) fn rename_workspace(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(record) = self
            .settings
            .global
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
        else {
            return false;
        };
        if record.name == name {
            return true;
        }
        record.name = name.to_string();
        let _ = self.settings.save();
        true
    }

    /// Delete a workspace from the list.
    ///
    /// The registry entry goes, and its sessions are REHOMED onto the workspace
    /// that becomes active rather than deleted: a row leaving a list must not
    /// take a transcript with it. Deleting the last one leaves none, which the
    /// sidebar and the hero both handle as "no workspace".
    pub(crate) fn delete_workspace(&mut self, id: &str) -> bool {
        // Park the record first, for the same reason a chat is parked first: a
        // delete nobody can undo is a delete nobody dares to use.
        let Some(record) = self
            .settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned()
        else {
            return false;
        };
        if self
            .stash(
                crate::trash::Kind::Workspace,
                &record.name,
                &record.id,
                &record,
            )
            .is_none()
        {
            return false;
        }
        let before = self.settings.global.workspaces.len();
        self.settings
            .global
            .workspaces
            .retain(|workspace| workspace.id != id);
        if self.settings.global.workspaces.len() == before {
            return false;
        }
        let next = if self.settings.global.general.active_workspace == id {
            self.settings
                .global
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone())
                .unwrap_or_default()
        } else {
            self.settings.global.general.active_workspace.clone()
        };
        self.settings.global.general.active_workspace = next.clone();
        // The store's copy of the registry is a CACHE, so it is re-read here:
        // without it the deleted workspace's chats still read as owned and the
        // orphan sweep below moves nothing.
        self.sync_store_roots();
        if !next.is_empty() {
            // The deleted workspace's sessions are unregistered now, which is
            // exactly the orphan case the store already knows how to move.
            self.store.rehome_orphans(&next);
        }
        let _ = self.settings.save();
        self.refresh_mcp();
        true
    }

    /// Create a chat in the named workspace and select it.
    ///
    /// Different from [`AppState::set_workspace`]: that one MOVES the chat under
    /// the editor, because the hero's picker reassigns the blank session it
    /// sits under. This one leaves every existing chat where it is.
    pub(crate) fn new_session_in(&mut self, workspace: &str) -> Option<String> {
        self.settings.global.general.active_workspace = workspace.to_string();
        self.new_session()
    }

    /// Retitle a session.
    ///
    /// A rename is a title and nothing else: the transcript, the workspace and
    /// the model reference stay put, and `updated_at` does not move - renaming a
    /// conversation must not reorder the reader's list under their hand.
    pub(crate) fn rename_session(&mut self, id: &str, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(session) = self.store.session_mut(id) {
            session.title = title.to_string();
        }
        self.save_session(id);
        self.store.sort();
    }

    /// Delete a chat, parking it in the recycle bin first.
    ///
    /// The parking write comes BEFORE the removal: if the envelope cannot be
    /// written the delete does not happen, because losing the copy is worse than
    /// refusing the delete. The selection then moves to the next chat in the list,
    /// or to none when that was the last one.
    pub(crate) fn delete_session(&mut self, id: &str) -> bool {
        let Some(session) = self.store.session(id).cloned() else {
            return false;
        };
        if self
            .stash(
                crate::trash::Kind::Session,
                &session.title,
                &session.workspace,
                &session,
            )
            .is_none()
        {
            return false;
        }
        if !self.store.remove(id) {
            return false;
        }
        self.plans.remove(id);
        if self.active_id.as_deref() == Some(id) {
            let next = self
                .store
                .sessions
                .first()
                .map(|session| session.id.clone());
            self.active_id = next.clone();
            self.settings.global.general.active_session = next.unwrap_or_default();
            let _ = self.settings.save_global();
        }
        true
    }

    /// Write one item's envelope into the bin.
    fn stash<T: serde::Serialize>(
        &self,
        kind: crate::trash::Kind,
        name: &str,
        workspace: &str,
        payload: &T,
    ) -> Option<std::path::PathBuf> {
        let payload = serde_json::to_value(payload).ok()?;
        let workspace_name = self
            .settings
            .global
            .workspaces
            .iter()
            .find(|record| record.id == workspace)
            .map(|record| record.name.clone())
            .unwrap_or_else(|| workspace.to_string());
        // The id keys both the envelope and the log beside it, so a restore can
        // find the pages without trusting a path recorded earlier.
        let id = payload
            .get("id")
            .and_then(|id| id.as_str())
            .unwrap_or("item")
            .to_string();
        let envelope = crate::trash::Envelope {
            kind: kind.key().to_string(),
            deleted_at: now(),
            workspace: workspace.to_string(),
            workspace_name,
            name: name.to_string(),
            // A chat's transcript lives in Parquet pages beside its document, so
            // the park carries them: the bin holds the whole chat, and restoring
            // does not depend on a directory a purge or a moved workspace could
            // take away.
            log: match kind {
                crate::trash::Kind::Session => format!("log-{id}"),
                crate::trash::Kind::Workspace => String::new(),
            },
            payload,
        };
        let dir = self.settings.trash_dir();
        if kind == crate::trash::Kind::Session {
            let from = crate::history::History::dir_for(&self.store.dir_for_id(&id), &id);
            let _ = crate::trash::park_log(&dir, &from, &envelope);
        }
        crate::trash::stash(&dir, &envelope).ok()
    }

    /// Everything in the bin, newest first.
    pub(crate) fn trash(&self) -> Vec<crate::trash::Entry> {
        crate::trash::entries(&self.settings.trash_dir())
    }

    /// Put a parked item back.
    ///
    /// A chat whose workspace no longer exists lands in the workspace in use - the
    /// alternative is restoring it onto a root nobody has, which is the orphan case
    /// the store already refuses to draw.
    pub(crate) fn restore_trashed(&mut self, path: &std::path::Path) -> bool {
        let Some(entry) = crate::trash::entries(&self.settings.trash_dir())
            .into_iter()
            .find(|entry| entry.path == path)
        else {
            return false;
        };
        match entry.kind() {
            Some(crate::trash::Kind::Session) => {
                let Ok(mut session) =
                    serde_json::from_value::<crate::model::Session>(entry.envelope.payload.clone())
                else {
                    return false;
                };
                let known = self
                    .settings
                    .global
                    .workspaces
                    .iter()
                    .any(|record| record.id == session.workspace);
                if !known {
                    session.workspace = self.settings.global.general.active_workspace.clone();
                }
                let restored = session.id.clone();
                if self.store.save(&session).is_err() {
                    return false;
                }
                self.store.insert(session);
                // The park moved the Parquet pages into the bin, so a restore
                // moves them back before the window is read: a chat comes out of
                // the bin whole, whatever happened to its workspace meanwhile.
                if !entry.envelope.log.is_empty() {
                    let parked = crate::trash::log_dir(&self.settings.trash_dir(), &entry.envelope);
                    let into = crate::history::History::dir_for(
                        &self.store.dir_for_id(&restored),
                        &restored,
                    );
                    let _ =
                        std::fs::create_dir_all(into.parent().unwrap_or(std::path::Path::new(".")));
                    let _ = std::fs::remove_dir_all(&into);
                    let _ = std::fs::rename(&parked, &into);
                }
                self.store.rehydrate(&restored);
                self.store.sort();
            }
            Some(crate::trash::Kind::Workspace) => {
                let Ok(record) = serde_json::from_value::<crate::settings::global::WorkspaceRef>(
                    entry.envelope.payload.clone(),
                ) else {
                    return false;
                };
                if !self
                    .settings
                    .global
                    .workspaces
                    .iter()
                    .any(|existing| existing.id == record.id)
                {
                    self.settings.global.workspaces.push(record);
                }
                self.sync_store_roots();
                let _ = self.settings.save();
            }
            None => return false,
        }
        crate::trash::purge(path);
        true
    }

    /// Delete a parked item for good.
    /// Remove the bsh state folder inside a workspace root.
    ///
    /// Only that one directory: a purge removes bsh own state (annotations, the
    /// terminal policy, the trace index) and never the workspace files. An empty
    /// or unknown root removes nothing.
    fn remove_workspace_state(root: &std::path::Path) {
        if root.as_os_str().is_empty() {
            return;
        }
        let state = root.join(".bsh");
        if state.is_dir() {
            let _ = std::fs::remove_dir_all(state);
        }
    }

    pub(crate) fn purge_trashed(&mut self, path: &std::path::Path) -> bool {
        // A parked session's Parquet log outlives its document: the bin holds the
        // document, the pages never moved. Purging has to take them too, or every
        // deleted chat leaves its transcript behind on disk.
        if let Some(entry) = crate::trash::entries(&self.settings.trash_dir())
            .into_iter()
            .find(|entry| entry.path == path)
        {
            if entry.kind() == Some(crate::trash::Kind::Session) {
                if let Ok(session) =
                    serde_json::from_value::<crate::model::Session>(entry.envelope.payload.clone())
                {
                    // The log lives under the session's WORKSPACE directory, not
                    // the store root. The old path named `root/history/<id>`, a
                    // directory that never existed, so every purge left the
                    // transcript and the trajectory behind on disk.
                    let session_dir = self.store.dir_for_workspace(&session.workspace);
                    let dir = crate::history::History::dir_for(&session_dir, &session.id);
                    let _ = std::fs::remove_dir_all(dir);
                }
            }
            // Removing a workspace also removes the state bsh wrote into it: its
            // `.bsh` folder carries the annotations, the terminal policy and the
            // trace index for that root. The folder itself is never touched - only
            // the one directory bsh owns inside it.
            if entry.kind() == Some(crate::trash::Kind::Workspace) {
                if let Ok(workspace) = serde_json::from_value::<crate::settings::global::WorkspaceRef>(
                    entry.envelope.payload.clone(),
                ) {
                    Self::remove_workspace_state(std::path::Path::new(&workspace.root));
                }
            }
        }
        crate::trash::purge(path)
    }
    /// Whether a message can be the point a branch starts from.
    ///
    /// Measured from the reference: branchUnavailable is set when the tail has
    /// any later chat node, so the branch is offered on the LAST message of a
    /// completed turn and nowhere else. A turn still streaming has no terminal
    /// record to branch from, so its row disables the control too.
    ///
    /// Completion is read from the STREAM, not from a timing record: a turn that
    /// settled before bsh measured durations is still complete, and refusing to
    /// branch it would tie the control to a measurement rather than to the state
    /// of the conversation.
    pub(crate) fn can_branch_at(&self, id: &str, index: usize) -> bool {
        let Some(session) = self.store.session(id) else {
            return false;
        };
        if self.is_streaming(id) {
            return false;
        }
        let Some(message) = session.messages.get(index) else {
            return false;
        };
        message.role == crate::model::Role::Assistant && index + 1 == session.messages.len()
    }

    /// Branch a session at one assistant message: a new conversation holding
    /// the transcript up to and including that message.
    ///
    /// The reference calls this fork({ atSeq, increaseTitle: true }). The copy
    /// is a NEW session - its own id and its own log - so pin, archive and
    /// deletion of the original never reach it. Its title is the original's with
    /// an incremented ordinal, which is what increaseTitle means: two branches
    /// of one chat must not be indistinguishable in the sidebar.
    pub(crate) fn branch_session_at(&mut self, id: &str, index: usize) -> Option<String> {
        if !self.can_branch_at(id, index) {
            return None;
        }
        let mut forked = self.store.session(id)?.clone();
        let branch_len = index + 1;
        forked.id = uuid::Uuid::new_v4().to_string();
        forked.title = self.branch_title(&forked.title, &forked.workspace);
        forked.created_at = now();
        forked.updated_at = forked.created_at;
        forked.pinned = false;
        forked.archived_at = None;
        // The cut is what makes it a branch: everything after the chosen
        // message belongs to the original, not to this conversation.
        forked.messages.truncate(branch_len);
        forked
            .compactions
            .retain(|record| record.after <= branch_len);
        forked.compacted = forked.compacted.min(branch_len);
        // Folded-out rows describe the original's history above the cut.
        forked.archived.clear();
        forked.compact_command = None;
        forked.feedback = None;
        let forked_id = forked.id.clone();
        // The log is copied first so the rows ABOVE the loaded window survive;
        // the save then rewrites it down to the branch point.
        self.store.copy_history(id, &forked_id);
        let _ = self.store.save(&forked);
        self.store.insert(forked);
        self.store.sort();
        Some(forked_id)
    }

    /// The title a branch of a chat takes: the base ordinal, incremented.
    ///
    /// Chat becomes Chat (2), and branching either again gives Chat (3): the
    /// highest ordinal already used in the workspace wins, so two siblings never
    /// collide and a branch of a branch keeps counting from the same base.
    fn branch_title(&self, title: &str, workspace: &str) -> String {
        let title = title.trim();
        let base = match title.rfind(" (") {
            Some(at) if title.ends_with(')') => {
                let inside = &title[at + 2..title.len().saturating_sub(1)];
                if !inside.is_empty() && inside.chars().all(|character| character.is_ascii_digit())
                {
                    &title[..at]
                } else {
                    title
                }
            }
            _ => title,
        };
        let mut highest = 1u32;
        for other in &self.store.sessions {
            if other.workspace != workspace {
                continue;
            }
            let candidate = other.title.trim();
            if candidate == base {
                continue;
            }
            let Some(rest) = candidate
                .strip_prefix(base)
                .and_then(|rest| rest.strip_prefix(" ("))
                .and_then(|rest| rest.strip_suffix(')'))
            else {
                continue;
            };
            if let Ok(ordinal) = rest.parse::<u32>() {
                highest = highest.max(ordinal);
            }
        }
        format!("{base} ({})", highest + 1)
    }
    /// Branch a session: a copy of its transcript as a new conversation.
    ///
    /// This copies the WHOLE transcript; [`Self::branch_session_at`] is the same
    /// operation cut at one message. The copy is a NEW session (its own id, its
    /// own file), laid out as a sibling: pin and archive do not follow it,
    /// because those are decisions about the original.
    pub(crate) fn fork_session(&mut self, id: &str) -> Option<String> {
        let mut forked = self.store.session(id)?.clone();
        forked.id = uuid::Uuid::new_v4().to_string();
        forked.created_at = now();
        forked.updated_at = forked.created_at;
        forked.pinned = false;
        forked.archived_at = None;
        let forked_id = forked.id.clone();
        // The copy carries the whole log, not just the window in memory.
        self.store.copy_history(id, &forked_id);
        let _ = self.store.save(&forked);
        self.store.insert(forked);
        self.store.sort();
        Some(forked_id)
    }

    /// Whether a chat is in the archive.
    pub(crate) fn is_archived(&self, id: &str) -> bool {
        self.store
            .session(id)
            .map(|session| session.archived_at.is_some())
            .unwrap_or(false)
    }

    /// A session's title, for the rename field to start from.
    pub(crate) fn session_title(&self, id: &str) -> Option<String> {
        self.store.session(id).map(|session| session.title.clone())
    }
    /// Fold or unfold a workspace's session list.
    pub(crate) fn toggle_collapse(&mut self, workspace: &str) {
        if !self.collapsed.remove(workspace) {
            self.collapsed.insert(workspace.to_string());
        }
    }

    /// The active workspace's filesystem root: the root the panel browses and
    /// the root the agent's own tools are confined to.
    ///
    /// The reference asks its remote `fileReferences` for the same tree, so the
    /// two views agree by construction. None while no workspace is selected or
    /// its folder has gone; a panel over none says so rather than opening on a
    /// directory it cannot read.
    pub(crate) fn workspace_root(&self) -> Option<std::path::PathBuf> {
        let settings = &self.settings;
        settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == settings.global.general.active_workspace)
            .map(|workspace| std::path::PathBuf::from(&workspace.root))
            .filter(|root| root.is_dir())
    }

    /// Whether a session is currently streaming.
    pub(crate) fn is_streaming(&self, id: &str) -> bool {
        if self.subagent.is_some() && self.active_id.as_deref() == Some(id) {
            return self
                .subagent_view_run()
                .is_some_and(|run| run.status.is_running());
        }
        if self
            .subagents
            .snapshot()
            .iter()
            .any(|run| run.id == id && run.status.is_running())
        {
            return true;
        }
        self.streams.contains_key(id)
    }

    /// Any session streaming right now.
    /// How long the active turn has been streaming, as the reference shows it.
    pub(crate) fn turn_elapsed(&self) -> Option<String> {
        let seconds = self.turn_started?.elapsed().as_secs();
        Some(if seconds < 60 {
            format!("{seconds}s")
        } else {
            format!("{}m {}s", seconds / 60, seconds % 60)
        })
    }

    pub(crate) fn any_streaming(&self) -> bool {
        !self.streams.is_empty()
    }

    /// The background jobs of the session on screen, newest first.
    ///
    /// This is the reference's own scope: the header's chip and its popover
    /// describe the conversation the reader is in, so switching sessions shows
    /// that session's jobs and never another session's. The roster is insertion
    /// order reversed, so the order is stable across frames.
    pub(crate) fn background_jobs(&self) -> Vec<agent::jobs::JobSummary> {
        self.active_id
            .as_deref()
            .map(|id| self.jobs.list_for_session(id))
            .unwrap_or_default()
    }

    /// The running and settled counts for the session on screen.
    pub(crate) fn job_counts(&self) -> (usize, usize) {
        let Some(id) = self.active_id.as_deref() else {
            return (0, 0);
        };
        (self.jobs.running_count(id), self.jobs.settled_count(id))
    }

    /// Whether the session on screen has a job that can still write output.
    pub(crate) fn has_running_jobs(&self) -> bool {
        self.active_id
            .as_deref()
            .is_some_and(|id| self.jobs.running_count(id) > 0)
    }

    /// A transient notice line.
    pub(crate) fn notice(&mut self, text: impl Into<String>) {
        self.notice = Some(text.into());
    }

    /// The active transcript as plain text, for the copy control.
    pub(crate) fn transcript_text(&self) -> String {
        let Some(session) = self.active_session() else {
            return String::new();
        };
        session
            .messages
            .iter()
            .map(|message| message.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(
                "

",
            )
    }

    /// Stop the active session's stream.
    pub(crate) fn stop(&mut self, id: &str) {
        self.runner.stop(id);
        self.streams.remove(id);
        // The task is aborted before it can report a withdrawal, so the rows
        // it never answered for are put back here: an aborted turn never
        // delivered them, and the next turn is seeded from the store.
        self.steer_mailboxes.remove(id);
        self.restore_orphan_steers(id);
        let stopped = now_ms();
        if let Some(session) = self.store.session_mut(id) {
            if let Some(message) = session.messages.last_mut() {
                if message.text.is_empty() && message.error.is_none() {
                    message.text = "…".to_string();
                }
                // A stop is terminal too: the turn's measured span ends here.
                settle_ms(message, stopped);
            }
            session.updated_at = now();
        }
        if let Some(session) = self.store.session(id) {
            let _ = self.store.save(session);
        }
    }

    /// Snapshot the history, append the user turn and the assistant placeholder.
    /// The last turn's prompt and completion tokens, when the run reported any.
    pub(crate) fn last_usage(&self) -> Option<(u64, u64)> {
        let session = self.active_session()?;
        let meta = session.messages.last()?.meta.as_ref()?;
        let usage = meta.usage.as_ref()?;
        Some((usage.prompt_tokens, usage.completion_tokens))
    }

    /// Output rate of the streaming turn, in tokens per second.
    ///
    /// Only the turn in flight has a meaningful rate, so an idle session
    /// reports none rather than a figure averaged over the past.
    pub(crate) fn tokens_per_second(&self) -> Option<u64> {
        self.turn_started?;
        let (_, completion) = self.last_usage()?;
        let seconds = self.turn_started?.elapsed().as_secs_f32();
        (seconds >= 1.0 && completion > 0).then(|| (completion as f32 / seconds) as u64)
    }

    /// The model the status bar names: the last turn's, else the configured one.
    ///
    /// A session that switched models reports the model the newest turn ran
    /// against; a session with no usage yet falls back to what the next turn
    /// will be sent to, so the field is never blank while a model is set.
    #[cfg(test)]
    pub(crate) fn status_model(&self) -> Option<String> {
        let from_turn = self
            .active_session()
            .and_then(|session| session.messages.last())
            .and_then(|message| message.meta.as_ref())
            .and_then(|meta| meta.usage.as_ref())
            .map(|usage| usage.model.clone())
            .filter(|model| !model.is_empty());
        from_turn.or_else(|| {
            let configured = self.effective().model;
            (!configured.is_empty()).then_some(configured)
        })
    }

    /// The last turn's prompt size and the context budget behind the meter.
    pub(crate) fn context_figures(&self) -> Option<(u64, u64)> {
        let budget = self.settings.workspace.agent.context_budget as u64;
        let (prompt, _) = self.last_usage()?;
        (budget > 0 && prompt > 0).then_some((prompt, budget))
    }

    /// The last turn's prompt tokens as a share of the context budget.
    ///
    /// The reference rounds this share (`Math.round`) and caps it at 100, so a
    /// 45.4% occupancy reads 45% in the ring and in its panel alike.
    pub(crate) fn context_percent(&self) -> Option<u64> {
        let (prompt, budget) = self.context_figures()?;
        context_share(prompt, budget)
    }

    /// The heuristic composition of the context the next turn would send.
    ///
    /// The reference's `contextBreakdown` prices its retained surface; bsh
    /// prices the same three buckets from the effective system prompt, the tool
    /// schemas of the last built agent and the transcript. `None` while nothing
    /// is priced, which is when the panel falls back to the reference's single
    /// total segment.
    /// The subagent runs the header lists for the active conversation, newest
    /// last. A run belongs to the session that spawned it; switching sessions
    /// leaves it in the registry but out of this lineage.
    pub(crate) fn subagent_runs(&self) -> Vec<crate::subagent::SubagentRun> {
        let parent = self.active_id.as_deref();
        self.subagents
            .snapshot()
            .into_iter()
            .filter(|run| parent.is_none_or(|id| run.session == id))
            .collect()
    }

    /// The selected child's registry row.
    pub(crate) fn subagent_view_run(&self) -> Option<crate::subagent::SubagentRun> {
        let id = self.subagent.as_ref()?.id.as_str();
        self.subagents
            .snapshot()
            .into_iter()
            .find(|run| run.id == id)
    }

    /// Assemble the Session model shown for one child run.
    fn subagent_session(&self, run: &crate::subagent::SubagentRun) -> Session {
        let parent = self
            .active_id
            .as_deref()
            .and_then(|id| self.store.session(id));
        let workspace = parent
            .map(|session| session.workspace.clone())
            .unwrap_or_default();
        let mut session = Session::new(&workspace, run.title.clone());
        session.id = format!("preview-{}", run.id);
        session.created_at = run.started_at / 1000;
        session.updated_at = run.finished_at.unwrap_or_else(now) / 1000;
        if let Some(parent) = parent {
            session.provider = parent.provider.clone();
            session.model = parent.model.clone();
            session.agent_preset = parent.agent_preset.clone();
        }
        session.messages = run.chat_messages();
        if run.status.is_running() {
            session
                .messages
                .push(crate::model::ChatMessage::assistant());
        }
        session
    }

    /// Switch the chat/trajectory view model to one child run.
    pub(crate) fn show_subagent(&mut self, id: &str) {
        let run = self
            .subagents
            .snapshot()
            .into_iter()
            .find(|run| run.id == id);
        let Some(run) = run else {
            return;
        };
        let parent_title = self
            .active_id
            .as_deref()
            .and_then(|id| self.store.session(id))
            .map(|session| session.title.clone())
            .unwrap_or_default();
        self.subagent = Some(SubagentView {
            id: id.to_string(),
            parent_title,
            session: self.subagent_session(&run),
        });
    }

    /// Rebuild the Session model after a child turn appends or finishes.
    pub(crate) fn refresh_subagent_preview(&mut self) {
        let Some(run) = self.subagent_view_run() else {
            return;
        };
        let session = self.subagent_session(&run);
        if let Some(subagent) = &mut self.subagent {
            subagent.session = session;
        }
    }

    /// The root conversation's title while a child view is open.
    pub(crate) fn subagent_parent_title(&self) -> Option<String> {
        self.subagent
            .as_ref()
            .map(|subagent| subagent.parent_title.clone())
    }

    /// Return to the parent conversation.
    pub(crate) fn leave_subagent(&mut self) {
        self.subagent = None;
    }

    /// Write every changed child run as a normal Session: JSON document plus
    /// the same Parquet transcript log a parent conversation uses.
    pub(crate) fn persist_subagents(&mut self) {
        let runs = self.subagents.snapshot();
        for run in &runs {
            // The log is append-only; a partial in-place row cannot update
            // an in-place row, so only a settled child is written.
            if run.status.is_running() {
                continue;
            }
            let signature = (run.status, run.revision, run.finished_at);
            if self.persisted_subagents.get(&run.id) == Some(&signature) {
                continue;
            }
            self.save_subagent(run);
            self.persisted_subagents.insert(run.id.clone(), signature);
        }
        self.persisted_subagents
            .retain(|id, _| runs.iter().any(|run| &run.id == id));
    }

    fn save_subagent(&mut self, run: &crate::subagent::SubagentRun) {
        let parent = self.store.session(&run.session).cloned();
        let existing = self.store.session(&run.id).cloned();
        let workspace = parent
            .as_ref()
            .map(|session| session.workspace.clone())
            .or_else(|| existing.as_ref().map(|session| session.workspace.clone()))
            .unwrap_or_default();
        let mut session = existing.unwrap_or_else(|| Session::new(&workspace, run.title.clone()));
        session.id = run.id.clone();
        session.title = run.title.clone();
        session.workspace = workspace;
        session.created_at = run.started_at / 1000;
        session.updated_at = run.finished_at.map(|ms| ms / 1000).unwrap_or_else(now);
        if let Some(parent) = &parent {
            session.provider = parent.provider.clone();
            session.model = parent.model.clone();
            session.agent_preset = parent.agent_preset.clone();
        }
        session.messages = run.chat_messages();
        session.subagent = Some(run.meta());
        session.loaded = true;
        let _ = self.store.save(&session);
        if self.store.session(&run.id).is_some() {
            if let Some(slot) = self.store.session_mut(&run.id) {
                *slot = session;
            }
        } else {
            self.store.insert(session);
        }
    }

    pub(crate) fn context_breakdown(&self) -> Option<crate::model::ContextBreakdown> {
        let session = self.active_session()?;
        let prompt_chars = self
            .active_id
            .as_deref()
            .map(|id| self.effective_in(id).llm_system_prompt().chars().count())
            .unwrap_or_else(|| {
                self.settings
                    .effective()
                    .llm_system_prompt()
                    .chars()
                    .count()
            });
        // The meter prices what the NEXT request carries, so a compaction
        // lowers it at the same moment the context actually shrinks.
        let len = session.messages.len();
        let start = request_start(session);
        // Everything except the last message is stable while only that message
        // streams, so its price is kept and the tail is priced per frame. A new
        // turn or a compaction moves `priced`, and the prefix is rebuilt once.
        let priced = len.saturating_sub(1).max(start);
        let prefix_total = {
            let mut cache = self.breakdown_cache.borrow_mut();
            if cache.session != session.id
                || cache.compacted != start
                || cache.from != session.loaded_from
                || cache.priced != priced
                || cache.checkpoints != session.compactions.len()
            {
                cache.prefix_total = session.messages[start..priced]
                    .iter()
                    .map(|message| message.priced_tokens())
                    .sum();
                cache.checkpoint_total = crate::state::replayed_checkpoint(session)
                    .map(|record| replayed_checkpoint_message(record).priced_tokens())
                    .unwrap_or(0);
                cache.session = session.id.clone();
                cache.compacted = start;
                cache.from = session.loaded_from;
                cache.priced = priced;
                cache.checkpoints = session.compactions.len();
            }
            cache.prefix_total + cache.checkpoint_total
        };
        let tail = crate::model::ContextBreakdown::price(
            prompt_chars,
            self.tools_schema_chars,
            &session.messages[priced..],
        );
        let breakdown = crate::model::ContextBreakdown {
            messages: prefix_total + tail.messages,
            ..tail
        };
        (!breakdown.is_empty()).then_some(breakdown)
    }

    /// The active session's token buckets, when any turn reported usage.
    pub(crate) fn stat_summary(&self) -> Option<crate::model::UsageSummary> {
        let summary = self.active_session()?.usage_summary();
        (!summary.is_empty()).then_some(summary)
    }

    /// Snapshot the active session's history and append the user turn.
    pub(crate) fn begin_turn(&mut self, prompt: &str) -> Option<(String, Vec<ChatMessage>, usize)> {
        let id = self.active_id.clone()?;
        let (transcript, budget) = self.begin_turn_in(&id, prompt)?;
        Some((id, transcript, budget))
    }

    /// Begin a turn in one session, whatever put the message there.
    ///
    /// The scheduler runs a task in the session it was created in and the queue
    /// drains into the session whose turn just ended, so neither is necessarily
    /// the one on screen. The events fold into that session either way, so a
    /// reader who opens it sees the turn. `None` when the session is gone, which
    /// is what tells the caller to switch the task off.
    pub(crate) fn begin_turn_in(
        &mut self,
        session_id: &str,
        prompt: &str,
    ) -> Option<(Vec<ChatMessage>, usize)> {
        // The ceiling belongs to the session the turn is FOR, not to whichever
        // chat happens to be on screen: the queue and the scheduler open turns
        // in other sessions.
        let budget = self.effective_in(session_id).context_budget;
        let id = session_id.to_string();
        // One clock for the turn: the trajectory places the user row at the
        // submission and the assistant row at the same instant, so the input
        // lane measures the wait for the reply and the model lane the reply.
        let opened = now_ms();
        // The request starts where the last compaction left it: the elided rows
        // stay in the transcript but leave the context, behind a one-line note
        // that says how many there were.
        let transcript = {
            let session = self.store.session(&id)?;
            self.request_transcript(session)
        };
        {
            let session = self.store.session_mut(&id)?;
            if session.messages.is_empty() {
                session.title = Session::title_from(prompt, self.lang);
            }
            session.messages.push(ChatMessage::user(prompt.to_string()));
            session.messages.push(ChatMessage::assistant());
            for message in session.messages.iter_mut().rev().take(2) {
                message.timing_mut().start = Some(opened);
            }
            session.updated_at = now();
        }
        self.streams.insert(id, now());
        self.turn_started = Some(std::time::Instant::now());
        Some((transcript, budget))
    }

    /// Begin a GOAL round: the continuation of an ongoing goal.
    ///
    /// The difference from [`Self::begin_turn_in`] is what the transcript shows.
    /// A goal round has no human prompt: the model receives the round
    /// instruction as its input, and the session grows only the assistant
    /// placeholder the stream writes into. So a running goal reads as the agent
    /// continuing its own work rather than as somebody typing to it, which is
    /// the same shape the reference's goal context has.
    ///
    /// Returns `None` when the goal is gone, settled, paused, exhausted, or the
    /// session is already streaming.
    pub(crate) fn begin_goal_turn_in(
        &mut self,
        session_id: &str,
    ) -> Option<(Vec<ChatMessage>, usize)> {
        if self.is_streaming(session_id) {
            return None;
        }
        let budget = self.effective_in(session_id).context_budget;
        let opened = now_ms();
        let transcript = {
            let session = self.store.session(session_id)?;
            if !session.goal.as_ref()?.can_advance() {
                return None;
            }
            self.request_transcript(session)
        };
        {
            let session = self.store.session_mut(session_id)?;
            let goal = session.goal.as_mut()?;
            if !goal.advance() {
                return None;
            }
            session.messages.push(ChatMessage::assistant());
            for message in session.messages.iter_mut().rev().take(1) {
                message.timing_mut().start = Some(opened);
            }
            session.updated_at = now();
        }
        self.save_session(session_id);
        self.streams.insert(session_id.to_string(), now());
        self.turn_started = Some(std::time::Instant::now());
        Some((transcript, budget))
    }

    /// Attach the plan store a turn about to start writes through.
    ///
    /// The store is shared with the `update_plan` tool, so the panel can read
    /// a running turn's plan live instead of waiting for the turn to end.
    pub(crate) fn attach_plan(&mut self, id: &str, store: PlanStore) {
        self.plans.insert(id.to_string(), store);
    }

    /// The plan to render for one session.
    ///
    /// A running turn reads through its live store; otherwise the copy its
    /// last finished turn saved is used, which is what survives a restart.
    pub(crate) fn plan_items(&self, id: &str) -> Vec<PlanItem> {
        self.plans.get(id).map(PlanStore::items).unwrap_or_else(|| {
            self.store
                .session(id)
                .map(|session| session.plan.clone())
                .unwrap_or_default()
        })
    }

    /// Save a finished turn's plan onto its session and drop the live store.
    ///
    /// Also runs for a stream that ends without a terminal event (a stop),
    /// which is why it tolerates a store that is already gone.
    pub(crate) fn finish_plan(&mut self, id: &str) {
        let Some(store) = self.plans.remove(id) else {
            return;
        };
        let items = store.items();
        if let Some(session) = self.store.session_mut(id) {
            session.plan = items;
        }
        self.save_session(id);
    }

    /// Set, replace or clear one session's goal.
    ///
    /// Test-only: the product paths are the typed verbs (so a refusal can be
    /// reported) and the note editor (which never clears).
    #[cfg(test)]
    pub(crate) fn set_goal(&mut self, id: &str, objective: Option<&str>) -> bool {
        let Some(objective) = objective else {
            return self.clear_goal(id);
        };
        if self.edit_goal(id, objective).is_ok() {
            return true;
        }
        self.create_goal(id, objective).is_ok()
    }

    /// Create and arm a goal, or replace a completed one with a fresh identity.
    ///
    /// An unfinished goal must be edited, paused, resumed, blocked or cleared
    /// before another takes its place, which is the reference's create rule.
    pub(crate) fn create_goal(
        &mut self,
        id: &str,
        objective: &str,
    ) -> Result<GoalChange, GoalRefusal> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(GoalRefusal::InvalidObjective);
        }
        let replaced = {
            let Some(session) = self.store.session_mut(id) else {
                return Err(GoalRefusal::Missing);
            };
            if let Some(current) = session.goal.as_ref() {
                if current.state != GoalState::Completed {
                    return Err(GoalRefusal::AlreadyExists(current.state));
                }
            }
            let replaced = session.goal.is_some();
            let Some(goal) = crate::model::Goal::new(objective, None) else {
                return Err(GoalRefusal::InvalidObjective);
            };
            session.goal = Some(goal);
            session.updated_at = now();
            replaced
        };
        self.save_session(id);
        Ok(if replaced {
            GoalChange::Replaced
        } else {
            GoalChange::Created
        })
    }

    /// Rewrite a goal's objective without changing its phase, activation or
    /// round count. A COMPLETED goal is replaced by a fresh one, which is the
    /// reference's rule for /goal edit <objective>.
    pub(crate) fn edit_goal(
        &mut self,
        id: &str,
        objective: &str,
    ) -> Result<GoalChange, GoalRefusal> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(GoalRefusal::InvalidObjective);
        }
        let replaced = {
            let Some(session) = self.store.session_mut(id) else {
                return Err(GoalRefusal::Missing);
            };
            let completed = match session.goal.as_ref() {
                None => return Err(GoalRefusal::Missing),
                Some(goal) => goal.state == GoalState::Completed,
            };
            if completed {
                let Some(fresh) = crate::model::Goal::new(objective, None) else {
                    return Err(GoalRefusal::InvalidObjective);
                };
                session.goal = Some(fresh);
            } else {
                let Some(goal) = session.goal.as_mut() else {
                    return Err(GoalRefusal::Missing);
                };
                goal.objective = objective.to_string();
                goal.sanitize();
            }
            session.updated_at = now();
            completed
        };
        self.save_session(id);
        Ok(if replaced {
            GoalChange::Replaced
        } else {
            GoalChange::Updated
        })
    }

    /// The goal one session is pursuing, if it has one.
    pub(crate) fn goal(&self, id: &str) -> Option<crate::model::Goal> {
        self.store
            .session(id)
            .and_then(|session| session.goal.clone())
    }

    /// Whether one session's goal is armed: active, not exhausted, and its
    /// process-local continuation flag is set.
    pub(crate) fn goal_can_advance(&self, id: &str) -> bool {
        self.goal(id).is_some_and(|goal| goal.can_advance())
    }

    /// Pause an active goal and disarm it, keeping the durable phase.
    pub(crate) fn pause_goal(&mut self, id: &str) -> Result<(), GoalRefusal> {
        {
            let Some(session) = self.store.session_mut(id) else {
                return Err(GoalRefusal::Missing);
            };
            let Some(goal) = session.goal.as_mut() else {
                return Err(GoalRefusal::Missing);
            };
            if !goal.pause() {
                return Err(GoalRefusal::InvalidTransition);
            }
            session.updated_at = now();
        }
        self.save_session(id);
        Ok(())
    }

    /// Resume a paused, blocked or disarmed goal and arm it, provided its round
    /// cap still has room. Resuming an active ARMED goal is refused, which is
    /// the reference's already-active-and-armed rejection.
    pub(crate) fn resume_goal(&mut self, id: &str) -> Result<(), GoalRefusal> {
        {
            let Some(session) = self.store.session_mut(id) else {
                return Err(GoalRefusal::Missing);
            };
            let Some(goal) = session.goal.as_mut() else {
                return Err(GoalRefusal::Missing);
            };
            if !goal.resume() {
                return Err(GoalRefusal::InvalidTransition);
            }
            session.updated_at = now();
        }
        self.save_session(id);
        Ok(())
    }

    /// Drop continuation authority without changing the durable phase. A failed
    /// or cancelled round leaves the goal where it is and disarms it, which is
    /// the reference driver's rule; the reader resumes explicitly.
    pub(crate) fn disarm_goal(&mut self, id: &str) -> bool {
        let changed = {
            let Some(session) = self.store.session_mut(id) else {
                return false;
            };
            let Some(goal) = session.goal.as_mut() else {
                return false;
            };
            let was_armed = goal.activation.is_armed();
            goal.disarm();
            if was_armed {
                session.updated_at = now();
            }
            was_armed
        };
        if changed {
            self.save_session(id);
        }
        changed
    }

    /// Record a policy-owned blocker on an active goal and disarm it.
    ///
    /// The round driver uses this at the cap: the reference leaves a durable
    /// blocked goal with the stable code round-limit rather than an active goal
    /// that silently stops advancing.
    pub(crate) fn block_goal(&mut self, id: &str, code: &str, message: &str) -> bool {
        let changed = {
            let Some(session) = self.store.session_mut(id) else {
                return false;
            };
            let Some(goal) = session.goal.as_mut() else {
                return false;
            };
            let changed = goal.block(code, message);
            if changed {
                session.updated_at = now();
            }
            changed
        };
        if changed {
            self.save_session(id);
        }
        changed
    }

    /// Remove the current goal while retaining the session's history.
    pub(crate) fn clear_goal(&mut self, id: &str) -> bool {
        let Some(session) = self.store.session_mut(id) else {
            return false;
        };
        if session.goal.take().is_none() {
            return false;
        }
        session.updated_at = now();
        self.save_session(id);
        true
    }

    /// Submit the attachments a successful create or edit admitted as ONE
    /// ordinary user message ahead of the goal's next round.
    ///
    /// The reference submits the blocks in selection order plus the fixed text
    /// naming their role, so later goal rounds read them from ordinary session
    /// history. The goal record stores no attachment state. Returns false when
    /// there is nothing to submit or the session is gone.
    pub(crate) fn append_goal_attachments(&mut self, id: &str, attachments: &[Attachment]) -> bool {
        if attachments.is_empty() {
            return false;
        }
        let Some(session) = self.store.session_mut(id) else {
            return false;
        };
        let mut message = ChatMessage::user(GOAL_ATTACHMENT_NOTE);
        message.images = attachments
            .iter()
            .map(|attachment| format!("data:{};base64,{}", attachment.mime, attachment.base64))
            .collect();
        message.timing_mut().start = Some(now_ms());
        session.messages.push(message);
        session.updated_at = now();
        self.save_session(id);
        true
    }

    /// Record the reader's note about one session, or clear it with blank text.
    pub(crate) fn set_session_feedback(&mut self, id: &str, note: &str) -> bool {
        let note = note.trim();
        {
            let Some(session) = self.store.session_mut(id) else {
                return false;
            };
            session.feedback = (!note.is_empty()).then(|| note.to_string());
            session.updated_at = now();
        }
        self.save_session(id);
        true
    }

    /// Whether one session is in plan mode.
    pub(crate) fn plan_mode(&self, id: &str) -> bool {
        self.store
            .session(id)
            .is_some_and(|session| session.plan_mode)
    }

    /// Whether the session on screen is in plan mode.
    pub(crate) fn active_plan_mode(&self) -> bool {
        self.active_id
            .as_deref()
            .is_some_and(|id| self.plan_mode(id))
    }

    /// Switch one session's plan mode and persist it.
    pub(crate) fn set_plan_mode(&mut self, id: &str, on: bool) {
        {
            let Some(session) = self.store.session_mut(id) else {
                return;
            };
            if session.plan_mode == on {
                return;
            }
            session.plan_mode = on;
            session.updated_at = now();
        }
        self.save_session(id);
    }

    /// The recent tail a compaction keeps verbatim, in estimated tokens.
    ///
    /// The manual command asks for ZERO, which is the shipped contract's own
    /// rule: `/compact` condenses everything it safely can rather than waiting
    /// for pressure, so the boundary walks back to the head and then forward to
    /// the nearest tool-pairing balanced cut. The number here is the fallback
    /// the automatic pre-send path prices a tail with.
    pub(crate) const COMPACT_RETAIN: usize = 0;

    /// Whether one session has older history a compaction could fold.
    ///
    /// The command asks this before building an agent, so "nothing to compact"
    /// costs no request at all. The question is the SAME one the summarizer
    /// answers - the priced, pairing-aware selection - so the gate cannot
    /// promise a compaction the selection then refuses.
    pub(crate) fn has_compactable_history(&self, id: &str) -> bool {
        self.store.session(id).is_some_and(|session| {
            // The SAME window the summarizer will be handed: a replayed
            // checkpoint plus the rows after the boundary. Asking the whole
            // on-screen transcript would promise a fold that the selection
            // then refuses once every foldable row has already left context.
            let window = self.request_transcript(session);
            crate::caps::compactable_span(&window, Self::COMPACT_RETAIN).is_some()
        })
    }

    /// Mark one session's older history as folded into a checkpoint.
    ///
    /// The transcript is untouched: every row stays on screen. The next
    /// request starts after the folded span and carries the checkpoint the
    /// model wrote as its first message, so nothing has to be synthesized.
    ///
    /// `start` is a GLOBAL transcript index: the first message the next request
    /// still carries, counted from the start of the log rather than from the
    /// loaded window. The caller derives it from the same span the summarizer
    /// was handed, so the two cannot disagree - and a restart or a page cannot
    /// shift it.
    ///
    /// Returns how many messages left the context.
    pub(crate) fn mark_compacted(&mut self, id: &str, start: usize) -> usize {
        let dropped;
        {
            let Some(session) = self.store.session_mut(id) else {
                return 0;
            };
            let end = session.loaded_from + session.messages.len();
            if start <= session.compacted || start > end {
                return 0;
            }
            dropped = start - session.compacted;
            session.compacted = start;
        }
        self.save_session(id);
        dropped
    }

    /// Record one compaction on its session.
    ///
    /// The transcript rows that were folded away stay on screen, so this is
    /// what lets the reader see that a compaction happened, how much history it
    /// condensed, and what the model was told in its place. The marker is drawn
    /// at the checkpoint event (after the retained tail), which is the slot the
    /// reference anchors it to; the Trajectory tab lists the same record.
    pub(crate) fn archive_checkpoint(&mut self, id: &str, report: CompactionReport) {
        {
            let Some(session) = self.store.session_mut(id) else {
                return;
            };
            session.compactions.push(crate::model::CompactionRecord {
                at: now(),
                items: report.items,
                tokens: report.tokens,
                summary: report.summary,
                manual: report.manual,
                after: session.loaded_from + checkpoint_slot(session),
            });
        }
        self.save_session(id);
    }

    /// Install one landed manual checkpoint: move the boundary, record the
    /// marker, and drop the command row the marker replaces.
    ///
    /// This is the reference's commit step exactly once: `mark_compacted` is the
    /// surface replacement, `archive_checkpoint` is the visible marker, and
    /// clearing `compact_command` is the `CompactionCommandCard` rule that the
    /// landed item takes the command card's place. Returns 0 when the boundary
    /// did not move, which means there is nothing to record.
    pub(crate) fn land_manual_compaction(
        &mut self,
        id: &str,
        start: usize,
        report: CompactionReport,
    ) -> usize {
        let dropped = self.mark_compacted(id, start);
        if dropped == 0 {
            return 0;
        }
        self.archive_checkpoint(id, report);
        self.clear_compact_command(id);
        dropped
    }

    /// The transcript one session's next request starts from.
    ///
    /// The reference replaces the shadowed surface span with ONE user-role
    /// checkpoint message and derives the request from it, so the next model
    /// call starts with the summary and then the retained tail. bsh keeps every
    /// folded row on screen and rebuilds that same request here: the newest
    /// `CompactionRecord` supplies the framed checkpoint, and the stored tail
    /// begins at the global `session.compacted` mapped into the loaded window.
    /// Because both halves are derived from the same durable record, the
    /// boundary and the summary cannot drift apart across a reload or a page.
    pub(crate) fn request_transcript(&self, session: &Session) -> Vec<ChatMessage> {
        let start = request_start(session);
        let mut transcript = Vec::with_capacity(
            session.messages.len().saturating_sub(start)
                + usize::from(replayed_checkpoint(session).is_some()),
        );
        if let Some(record) = replayed_checkpoint(session) {
            transcript.push(replayed_checkpoint_message(record));
        }
        transcript.extend_from_slice(&session.messages[start..]);
        transcript
    }

    /// Whether the next request already starts on a replayed checkpoint.
    pub(crate) fn has_replayed_checkpoint(&self, session: &Session) -> bool {
        replayed_checkpoint(session).is_some()
    }

    /// Map a summarizer span onto the stored transcript.
    ///
    /// `removed` is the number of messages the agent was handed and
    /// `replayed` how many of those were a synthesized checkpoint rather than
    /// stored rows (0 or 1). The result is the GLOBAL index of the first
    /// message the next request keeps; with every stored row removed it is the
    /// end of the window, so the checkpoint alone is the new context.
    pub(crate) fn compaction_boundary(session: &Session, removed: usize, replayed: usize) -> usize {
        let start = request_start(session);
        let stored = removed.saturating_sub(replayed);
        if stored == 0 {
            return session.loaded_from + start;
        }
        let mut seen = 0;
        for (index, message) in session.messages.iter().enumerate().skip(start) {
            let counts = matches!(
                message.role,
                crate::model::Role::User | crate::model::Role::Assistant
            ) && !message.text.is_empty();
            if counts {
                seen += 1;
                if seen > stored {
                    return session.loaded_from + index;
                }
            }
        }
        session.loaded_from + session.messages.len()
    }

    /// Mark the start of one `/compact` command, as the transcript row.
    ///
    /// The reference appends `command/run` and draws a running command card;
    /// this is the same lifecycle step on the one row bsh persists. Returns
    /// false when the session is gone.
    pub(crate) fn begin_compact_command(&mut self, id: &str) -> bool {
        let Some(session) = self.store.session_mut(id) else {
            return false;
        };
        session.compact_command = Some(CompactCommand {
            started_at: now(),
            outcome: None,
        });
        self.save_session(id);
        true
    }

    /// Record the text one `/compact` command ended with.
    ///
    /// `error` is the reference's own `kind` mapping: `error` draws the
    /// summary in the failure colour, `success` in the quiet one.
    pub(crate) fn finish_compact_command(
        &mut self,
        id: &str,
        text: impl Into<String>,
        error: bool,
    ) {
        let Some(session) = self.store.session_mut(id) else {
            return;
        };
        let Some(command) = session.compact_command.as_mut() else {
            return;
        };
        command.outcome = Some(CompactOutcome {
            text: text.into(),
            error,
        });
        self.save_session(id);
    }

    /// Drop the `/compact` row because a checkpoint marker replaces it.
    pub(crate) fn clear_compact_command(&mut self, id: &str) {
        let Some(session) = self.store.session_mut(id) else {
            return;
        };
        if session.compact_command.take().is_some() {
            self.save_session(id);
        }
    }

    /// Admit one manual compaction, or report that one is already running.
    pub(crate) fn start_compacting(&mut self, id: &str) -> bool {
        self.compacting.insert(id.to_string())
    }

    /// Release the manual-compaction admission flag.
    pub(crate) fn stop_compacting(&mut self, id: &str) {
        self.compacting.remove(id);
    }

    /// Whether a manual compaction is in flight for one session.
    pub(crate) fn is_compacting(&self, id: &str) -> bool {
        self.compacting.contains(id)
    }

    /// The share of the message budget an automatic compaction keeps verbatim.
    ///
    /// This is the shipped backend's `retainRatio` default (0.16 of the window
    /// less the routed output reservation). bsh has no per-model capacity
    /// metadata, so the workspace's `context_budget` is the ceiling that plays
    /// the reference's resolved threshold role; 16% of it is the same tail.
    pub(crate) const COMPACT_RETAIN_RATIO: f64 = 0.16;

    /// Whether the next request has reached the automatic compaction threshold,
    /// and the token tail to keep if it has.
    ///
    /// The reference measures the latest routed request's canonical envelope
    /// once per step and triggers above `floor(min(W * 0.8, W - O - B))`. bsh
    /// prices the same three buckets (system prompt, tool schemas, messages)
    /// against the configured ceiling, and returns `None` below it or when
    /// auto-compaction is switched off with a zero budget.
    pub(crate) fn auto_compaction_retain(
        &self,
        id: &str,
        transcript: &[ChatMessage],
        appended: &[ChatMessage],
        budget: usize,
    ) -> Option<usize> {
        let effective = self.effective_in(id);
        if budget == 0 {
            return None;
        }
        let prompt_chars = effective.llm_system_prompt().chars().count();
        let priced = crate::model::ContextBreakdown::price(
            prompt_chars,
            self.tools_schema_chars,
            transcript,
        );
        // begin_turn_in has already appended the user prompt and its assistant
        // placeholder to the store, but the window handed to the runner
        // predates them. Price them too, so the trigger sees the request the
        // turn is actually about to send.
        let appended = crate::model::ContextBreakdown::price(prompt_chars, 0, appended).messages;
        if priced.total() + appended < budget as u64 {
            return None;
        }
        Some((budget as f64 * Self::COMPACT_RETAIN_RATIO) as usize)
    }

    /// Fold one automatic compaction into its session.
    ///
    /// The runner reports what the summarizer removed from the request window
    /// it was handed; this installs the same boundary on the stored transcript,
    /// records the marker the reader sees, and clears any stale manual row. The
    /// title the transcript draws comes from `manual` being false, which is the
    /// reference's automatic-vs-command distinction.
    ///
    /// Returns the number of messages the context shed, or 0 when there was
    /// nothing to fold.
    pub(crate) fn land_automatic_compaction(
        &mut self,
        id: &str,
        before_tokens: usize,
        dropped: usize,
        summary: String,
    ) -> usize {
        let Some(session) = self.store.session_mut(id) else {
            return 0;
        };
        let replayed = usize::from(replayed_checkpoint(session).is_some());
        let start = Self::compaction_boundary(session, dropped, replayed);
        let previous = session.compacted;
        if start <= previous || start > session.loaded_from + session.messages.len() {
            return 0;
        }
        session.compacted = start;
        session.compact_command = None;
        session.compactions.push(crate::model::CompactionRecord {
            at: now(),
            items: dropped.saturating_sub(replayed),
            tokens: before_tokens,
            summary,
            manual: false,
            after: session.loaded_from + checkpoint_slot(session),
        });
        let dropped = start - previous;
        self.save_session(id);
        dropped
    }

    /// Whether one session already has a submission waiting for its turn.
    ///
    /// The reference's manual compaction requires an idle agent with no waking
    /// queued work, so the command's busy gate reads this beside the stream and
    /// compaction registries.
    pub(crate) fn has_queued(&self, id: &str) -> bool {
        self.inbox.get(id).is_some_and(|queue| !queue.is_empty())
    }

    /// Queue a message for a session whose turn is already running.
    ///
    /// Returns the row's identity, which is what its own edit and remove act on.
    pub(crate) fn enqueue(&mut self, id: &str, text: String, attachments: Vec<Attachment>) -> u64 {
        self.inbox_seq += 1;
        let seq = self.inbox_seq;
        self.inbox.entry(id.to_string()).or_default().push(Queued {
            seq,
            text,
            attachments,
        });
        seq
    }

    /// The messages one session is waiting to send, oldest first.
    pub(crate) fn inbox(&self, id: &str) -> &[Queued] {
        self.inbox.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Take the oldest queued message, which is the next turn's own.
    pub(crate) fn pop_inbox(&mut self, id: &str) -> Option<Queued> {
        let queue = self.inbox.get_mut(id)?;
        if queue.is_empty() {
            return None;
        }
        let queued = queue.remove(0);
        let empty = queue.is_empty();
        if empty {
            self.inbox.remove(id);
        }
        Some(queued)
    }

    /// Drop one queued message, which is a row's own remove.
    pub(crate) fn remove_queued(&mut self, id: &str, seq: u64) {
        let empty = match self.inbox.get_mut(id) {
            Some(queue) => {
                queue.retain(|queued| queued.seq != seq);
                queue.is_empty()
            }
            None => false,
        };
        if empty {
            self.inbox.remove(id);
        }
    }

    /// Replace one queued message's text, which is a row's own save.
    ///
    /// Only the text changes, which is also the reference's rule: a row that
    /// carries an image is not editable in place.
    pub(crate) fn edit_queued(&mut self, id: &str, seq: u64, text: String) {
        if let Some(queue) = self.inbox.get_mut(id) {
            if let Some(queued) = queue.iter_mut().find(|queued| queued.seq == seq) {
                queued.text = text;
            }
        }
    }

    /// Take one queued message off the dock by its identity.
    ///
    /// The row leaves the dock the moment the reader asks for it, so a steer
    /// that the turn cannot honour has to be able to put it back - which is
    /// what [`Self::restore_queued`] is for.
    pub(crate) fn take_queued(&mut self, id: &str, seq: u64) -> Option<Queued> {
        let queue = self.inbox.get_mut(id)?;
        let index = queue.iter().position(|queued| queued.seq == seq)?;
        let queued = queue.remove(index);
        let empty = queue.is_empty();
        if empty {
            self.inbox.remove(id);
        }
        Some(queued)
    }

    /// Put a queued row back in its identity order.
    ///
    /// Inserting by ticket rather than appending restores the row where it
    /// was, so a withdrawal cannot reorder the messages around it.
    pub(crate) fn restore_queued(&mut self, id: &str, queued: Queued) {
        let queue = self.inbox.entry(id.to_string()).or_default();
        let index = queue
            .iter()
            .position(|waiting| waiting.seq > queued.seq)
            .unwrap_or(queue.len());
        queue.insert(index, queued);
    }

    /// Attach the mailbox a newly spawned turn drains.
    pub(crate) fn attach_steers(&mut self, id: &str, mailbox: Arc<SteerMailbox>) {
        self.steer_mailboxes.insert(id.to_string(), mailbox);
    }

    /// The steer mailbox of a session whose turn is running.
    ///
    /// None for an idle session: a queued row there is an ordinary send, not a
    /// message into a mailbox nobody will drain.
    pub(crate) fn steers(&self, id: &str) -> Option<Arc<SteerMailbox>> {
        self.steer_mailboxes.get(id).cloned()
    }

    /// The rows this session has steered and is still waiting to have taken.
    ///
    /// The dock draws one disabled "Sending…" echo per row: it left the queue
    /// when the reader pressed Send now, but the turn has not admitted it yet,
    /// and the reference shows that pending span rather than dropping the row
    /// out of sight.
    pub(crate) fn pending_steers(&self, id: &str) -> Vec<&Queued> {
        let mut rows: Vec<&Queued> = self
            .steering
            .values()
            .filter(|steering| steering.session == id)
            .map(|steering| &steering.queued)
            .collect();
        rows.sort_by_key(|queued| queued.seq);
        rows
    }

    /// Keep the row a steer took until the turn answers for it.
    pub(crate) fn mark_steering(&mut self, id: &str, queued: Queued) {
        self.steering.insert(
            queued.seq,
            Steering {
                session: id.to_string(),
                queued,
            },
        );
    }

    /// Whether a ticket the shell reported is still waiting for the turn.
    #[cfg(test)]
    pub(crate) fn is_steering(&self, ticket: u64) -> bool {
        self.steering.contains_key(&ticket)
    }

    /// Put back every steering row this session never got an answer for.
    ///
    /// The runner reports a withdrawal BEFORE the terminal event, so an entry
    /// still here when the turn ends means its delivery report was lost - the
    /// row goes back rather than vanishing with the turn.
    fn restore_orphan_steers(&mut self, id: &str) {
        let orphans: Vec<u64> = self
            .steering
            .iter()
            .filter(|(_, steering)| steering.session == id)
            .map(|(ticket, _)| *ticket)
            .collect();
        for ticket in orphans {
            if let Some(steering) = self.steering.remove(&ticket) {
                self.restore_queued(&steering.session, steering.queued);
            }
        }
    }

    /// Record a steered message in the transcript at the point the turn took
    /// it.
    ///
    /// The running assistant row is closed at this instant, the message becomes
    /// a STEERING user row, and a fresh assistant row takes over: tokens after
    /// the interjection belong after it. The two assistant rows are the two
    /// assistant turns the running loop really produced - the request history
    /// alternates assistant, user, assistant - and the steering flag is what
    /// keeps the rail, the trajectory and the turn count treating the whole
    /// span as ONE turn.
    fn land_steered(&mut self, id: &str, ticket: u64) {
        let Some(steering) = self.steering.remove(&ticket) else {
            return;
        };
        let at = now_ms();
        let display = display_text(&steering.queued.text, &steering.queued.attachments);
        {
            let Some(session) = self.store.session_mut(id) else {
                return;
            };
            if let Some(last) = session.messages.last_mut() {
                if last.role == Role::Assistant {
                    settle_ms(last, at);
                }
            }
            let mut user = ChatMessage::steer(display);
            user.timing_mut().start = Some(at);
            session.messages.push(user);
            let mut placeholder = ChatMessage::assistant();
            placeholder.timing_mut().start = Some(at);
            session.messages.push(placeholder);
            session.updated_at = now();
        }
        self.save_session(id);
    }

    /// Put a withdrawn steering row back where it was queued.
    fn restore_steer(&mut self, ticket: u64) {
        let Some(steering) = self.steering.remove(&ticket) else {
            return;
        };
        self.restore_queued(&steering.session, steering.queued);
    }

    /// Fold one runner event into the transcript.
    pub(crate) fn apply_event(&mut self, id: &str, event: AgentEvent) {
        // An automatic compaction lands BEFORE the turn's first token: the
        // runner summarizes the over-threshold window and reports the span it
        // removed. The boundary and the marker are installed here, so the
        // request already streaming carries the checkpoint and the reader sees
        // the same "Context compacted" row a later reload restores. There is no
        // notice: the reference draws the marker in the transcript, not a
        // status line.
        if let AgentEvent::AutoCompacted {
            before,
            dropped,
            summary,
        } = &event
        {
            self.land_automatic_compaction(id, *before, *dropped, summary.clone());
        }
        // A steered message and a withdrawn one are transcript/queue events,
        // not stream deltas: they touch no message the turn is filling in, so
        // they are handled before the store borrow below.
        if let AgentEvent::Steered { ticket } = &event {
            self.land_steered(id, *ticket);
            return;
        }
        if let AgentEvent::SteerWithdrawn { ticket } = &event {
            self.restore_steer(*ticket);
            return;
        }
        let terminal = matches!(event, AgentEvent::Done | AgentEvent::Error(_));
        // One clock for the event: it stamps a call's start, the turn's first
        // output, and - on the terminal event - the turn's settled span.
        let at = now_ms();
        // The usage that lands belongs to the model the turn was sent to. Only a
        // Usage event reads it: resolving the settings for every streamed text
        // delta was work the event could not use.
        let model = matches!(event, AgentEvent::Usage { .. })
            .then(|| self.effective_in(id).model)
            .unwrap_or_default();
        {
            let Some(session) = self.store.session_mut(id) else {
                return;
            };
            match event {
                AgentEvent::Text(text) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.text.push_str(&text);
                        note_first_output(message, at);
                    }
                }
                AgentEvent::Reasoning(text) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.reasoning.push_str(&text);
                        note_first_output(message, at);
                    }
                }
                AgentEvent::ToolCall { name, args } => {
                    if let Some(message) = session.messages.last_mut() {
                        message.tools.push(ToolEvent {
                            name,
                            args,
                            output: None,
                            ms: None,
                            at: Some(at),
                        });
                        note_first_output(message, at);
                    }
                }
                AgentEvent::ToolResult { name, output, ms } => {
                    if let Some(message) = session.messages.last_mut() {
                        if let Some(tool) = message
                            .tools
                            .iter_mut()
                            .rev()
                            .find(|tool| tool.name == name && tool.output.is_none())
                        {
                            tool.output = Some(output);
                            tool.ms = Some(ms);
                        }
                    }
                }
                AgentEvent::Usage {
                    prompt,
                    completion,
                    cached,
                } => {
                    if let Some(message) = session.messages.last_mut() {
                        let meta = message.meta.get_or_insert_with(MessageMeta::default);
                        let usage = meta.usage.get_or_insert_with(MessageUsage::default);
                        usage.prompt_tokens += prompt;
                        usage.completion_tokens += completion;
                        usage.cached_tokens += cached;
                        usage.model = model.clone();
                    }
                }
                AgentEvent::Error(error) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.error = Some(error);
                    }
                }
                // Both steering events returned before this borrow; the
                // arms exist so the match stays exhaustive by the type.
                AgentEvent::AutoCompacted { .. }
                | AgentEvent::Steered { .. }
                | AgentEvent::SteerWithdrawn { .. }
                | AgentEvent::Done => {}
            }
            if terminal {
                session.updated_at = now();
                if let Some(message) = session.messages.last_mut() {
                    settle_ms(message, at);
                }
            }
        }
        if terminal {
            self.streams.remove(id);
            // The turn is over: nothing may steer it any more, and a steering
            // row it never answered for goes back to the queue.
            self.steer_mailboxes.remove(id);
            self.restore_orphan_steers(id);
            self.turn_started = None;
            self.refresh_policy_audit();
            if let Some(session) = self.store.session(id) {
                let _ = self.store.save(session);
            }
        }
    }

    /// Persist one session.
    pub(crate) fn save_session(&self, id: &str) {
        if let Some(session) = self.store.session(id) {
            let _ = self.store.save(session);
        }
    }

    /// Return an archived session to the sidebar.
    pub(crate) fn unarchive_session(&mut self, id: &str) {
        if let Some(session) = self.store.session_mut(id) {
            session.archived_at = None;
            session.updated_at = now();
        }
        self.save_session(id);
        self.store.sort();
        self.active_id = Some(id.to_string());
        self.subagent = None;
    }

    /// Archive a session so it leaves the sidebar without being destroyed.
    pub(crate) fn archive_session(&mut self, id: &str) {
        self.runner.stop(id);
        self.streams.remove(id);
        self.steer_mailboxes.remove(id);
        self.restore_orphan_steers(id);
        if let Some(session) = self.store.session_mut(id) {
            session.archived_at = Some(now());
            session.updated_at = now();
        }
        self.save_session(id);
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
            self.ensure_active();
        }
    }

    /// Pin or unpin a session.
    pub(crate) fn toggle_pin(&mut self, id: &str) {
        if let Some(session) = self.store.session_mut(id) {
            session.pinned = !session.pinned;
        }
        self.store.sort();
        self.save_session(id);
    }

    /// Record a good/bad verdict on one assistant message.
    pub(crate) fn set_feedback(&mut self, id: &str, index: usize, value: &str) {
        if let Some(session) = self.store.session_mut(id) {
            if let Some(message) = session.messages.get_mut(index) {
                let meta = message.meta.get_or_insert_with(MessageMeta::default);
                meta.feedback = if meta.feedback.as_deref() == Some(value) {
                    None
                } else {
                    Some(value.to_string())
                };
            }
        }
        self.save_session(id);
    }

    /// Delete one message and everything after it.
    pub(crate) fn truncate_from(&mut self, id: &str, index: usize) {
        if let Some(session) = self.store.session_mut(id) {
            session.messages.truncate(index);
            session.updated_at = now();
        }
        self.save_session(id);
    }

    /// Toggle one tool row open or closed.
    pub(crate) fn toggle_tool(&mut self, message: usize, tool: usize) {
        let key = format!("{message}:{tool}");
        if !self.expanded_tools.remove(&key) {
            self.expanded_tools.insert(key);
        }
    }

    /// Whether a tool row is open.
    /// The window of older chat rows waiting to be read, if the log has one.
    ///
    /// Building this is index-only; the rows are decoded by the shell on the
    /// background executor, so the render path never touches the files.
    pub(crate) fn begin_older_read(&self) -> Option<crate::store::OlderRead> {
        let id = self.active_id.as_deref()?;
        self.store.older_read(id, crate::store::WINDOW)
    }

    /// Apply one decoded window above the loaded chat and bound the window.
    ///
    /// Returns how many rows arrived; zero means the page was stale or empty.
    pub(crate) fn finish_older_read(
        &mut self,
        page: crate::store::OlderPage,
        keep_windows: usize,
    ) -> usize {
        let id = page.session.clone();
        let added = self.store.apply_older(page);
        if added > 0 {
            self.store
                .trim_front(&id, crate::store::WINDOW * keep_windows);
        }
        added
    }

    /// The trajectory page waiting to be read, if the log has one.
    pub(crate) fn begin_trace_page(
        &self,
        id: &str,
        page: usize,
    ) -> Option<crate::store::TraceRead> {
        self.store.trace_read(id, page)
    }

    pub(crate) fn tool_open(&self, message: usize, tool: usize) -> bool {
        self.expanded_tools.contains(&format!("{message}:{tool}"))
    }

    /// Open the usage pill's dialog, or close it when it is already open.
    pub(crate) fn toggle_stat(&mut self) {
        self.stat_open = !self.stat_open;
    }

    /// Open the context meter's panel, or close it when it is already open.
    pub(crate) fn toggle_meter(&mut self) {
        self.meter_open = !self.meter_open;
    }

    /// Record the serialized length of the tool schemas a built agent carries.
    pub(crate) fn set_tools_schema(&mut self, chars: usize) {
        self.tools_schema_chars = chars;
    }

    /// Keep the latest turn's policy and refresh the pane's audit snapshot.
    pub(crate) fn set_policy(&mut self, policy: Arc<agent::policy::PolicySet>) {
        self.policy = Some(policy);
        self.refresh_policy_audit();
    }

    /// The policy verdict behind one tool call of the active session.
    ///
    /// bsh-only: every call is made through the policy seam, so a call has a
    /// decision attached even when it succeeded. The permission and the ring
    /// are read together so a row cannot show one conversation's permission
    /// beside another's refusal.
    ///
    /// `tool` and `target` name the call being explained, `event` the
    /// trajectory row it belongs to is not needed here - the verdict is a
    /// function of the call, not of where it is drawn.
    pub(crate) fn policy_verdict_for(
        &self,
        tool: &str,
        target: &str,
    ) -> crate::ui::trajectory::PolicyVerdict {
        let permission = self
            .policy_audit
            .as_ref()
            .map(|audit| audit.permission.clone())
            .unwrap_or_default();
        // The preset follows the same rule as the permission: there is no
        // preset on the per-call record, so the policy the turn built answers
        // for it, and a denial keeps the preset recorded at its decision.
        let preset = self.applied_preset();
        // The ring is scoped to THIS session, so opening another chat cannot
        // show its refusals as this one's.
        let session = self.active_id.clone().unwrap_or_default();
        let refusals: Vec<crate::ui::trajectory::PolicyVerdict> = self
            .policy_audit
            .as_ref()
            .map(|audit| {
                audit
                    .denials
                    .iter()
                    .filter(|entry| entry.session == session)
                    .map(|entry| crate::ui::trajectory::PolicyVerdict {
                        permission: entry.permission.clone(),
                        preset: entry.preset.clone(),
                        action: entry.action.as_str().to_string(),
                        target: entry.target.clone(),
                        allowed: false,
                        rule: entry.rule.clone(),
                        reason: entry.reason.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        crate::ui::trajectory::verdict_for(tool, target, &permission, &preset, &refusals)
    }

    /// The agent preset the active session's calls run under.
    ///
    /// A `ToolEvent` records the tool, its arguments and its result, and has no
    /// preset field, so the applied preset is read from the policy the turn
    /// built - `PolicyData.preset`, which `build_policy` fills from
    /// `Effective.preset_id` - exactly as the permission beside it is. The
    /// snapshot is preferred, then the live set, so the drawer and the audit tab
    /// answer from the same fact. A session whose turn has not been built in
    /// this process has neither, and then the session-level effective preset is
    /// the honest answer rather than an empty cell or a guess from the tool.
    pub(crate) fn applied_preset(&self) -> String {
        if let Some(audit) = self.policy_audit.as_ref() {
            return audit.preset.clone();
        }
        if let Some(policy) = self.policy.as_ref() {
            return policy.data().preset.clone();
        }
        self.effective().preset_id
    }

    /// The label a preset id is shown under.
    ///
    /// One rule for every surface: the preset's declared display name when it
    /// has one, else the id itself, so an unnamed custom preset stays
    /// identifiable. An empty id is the built-in default, and it names that
    /// default the way the composer's preset chip does instead of leaving the
    /// cell blank.
    pub(crate) fn preset_label(&self, id: &str) -> String {
        let id = id.trim();
        if id.is_empty() {
            return crate::i18n::tr(self.lang, "input.defaultPreset");
        }
        self.settings
            .global
            .preset(id)
            .and_then(|preset| preset.name)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.to_string())
    }
    /// Re-read the denial ring and counters into the pane's snapshot.
    pub(crate) fn refresh_policy_audit(&mut self) {
        self.policy_audit = self.policy.as_ref().map(|policy| PolicyAuditView {
            denials: policy.denials(),
            allows: policy.allows(),
            denials_total: policy.denials_total(),
            evaluator: policy.evaluator().to_string(),
            permission: policy.data().permission.clone(),
            preset: policy.data().preset.clone(),
            error: policy.error().map(ToString::to_string),
        });
    }

    /// Empty the denial ring and reset the counters on the live policy.
    pub(crate) fn clear_policy_audit(&mut self) {
        if let Some(policy) = &self.policy {
            policy.clear_audit();
        }
        self.refresh_policy_audit();
    }

    /// The egress allowlist file of the active workspace.
    fn egress_allowlist_path(&self) -> Option<PathBuf> {
        let root = self.settings.active_workspace_root();
        (!root.trim().is_empty()).then(|| PathBuf::from(root).join(".bsh").join("egress-allowlist"))
    }

    /// Reload the allowlist entries from disk.
    pub(crate) fn refresh_egress_allowlist(&mut self) {
        self.egress_allowlist = self
            .egress_allowlist_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| {
                text.lines()
                    .map(|line| line.split('#').next().unwrap_or("").trim())
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
    }

    /// Add one exact address or CIDR range and persist the file.
    pub(crate) fn add_egress_allowlist(&mut self, entry: &str) -> Result<(), String> {
        let entry = entry.trim();
        if !valid_egress_entry(entry) {
            return Err("an entry must be an IP address or a CIDR range".to_string());
        }
        if !self.egress_allowlist.iter().any(|known| known == entry) {
            self.egress_allowlist.push(entry.to_string());
        }
        self.save_egress_allowlist()?;
        self.refresh_egress_allowlist();
        Ok(())
    }

    /// Remove one entry and persist the file.
    pub(crate) fn remove_egress_allowlist(&mut self, entry: &str) -> Result<(), String> {
        self.egress_allowlist.retain(|known| known != entry);
        self.save_egress_allowlist()?;
        self.refresh_egress_allowlist();
        Ok(())
    }

    fn save_egress_allowlist(&self) -> Result<(), String> {
        let path = self
            .egress_allowlist_path()
            .ok_or_else(|| "no active workspace".to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut text = String::from(
            "# Egress allowlist for the tool-boundary policy.
# One exact IP or CIDR range per line; # starts a comment.
",
        );
        for entry in &self.egress_allowlist {
            text.push_str(entry);
            text.push('\n');
        }
        std::fs::write(&path, text).map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Open the settings dialog on a copy of the saved settings.
    ///
    /// Nothing the dialog edits reaches the agent until it is committed, which
    /// is what Cancel undoes: the copy is simply dropped.
    pub(crate) fn open_settings(&mut self) {
        self.draft = Some(self.settings.clone());
        self.settings_section = 0;
        self.overlay = Overlay::Settings;
    }

    /// Discard the draft and close the dialog.
    pub(crate) fn cancel_settings(&mut self) {
        self.draft = None;
        self.overlay = Overlay::None;
    }

    /// Keep the draft, persist it, and LEAVE the dialog open.
    ///
    /// Save is a commit, not a close: a reader who just changed one row keeps
    /// the pane, its scroll position and every other edit in view. Closing is
    /// the close control's own action (or Cancel, which discards the draft).
    pub(crate) fn commit_settings(&mut self) -> std::io::Result<()> {
        if let Some(draft) = self.draft.take() {
            self.settings = draft;
        }
        self.settings.ensure_workspace();
        match self.settings.save() {
            Ok(()) => {
                // The MCP pane is config, and config is only real once the
                // pool reflects it: reconcile now rather than at the next turn.
                self.refresh_mcp();
                Ok(())
            }
            Err(error) => {
                // A refused write keeps the dialog open, with the draft the
                // reader had, because the refusal is something they can fix in
                // it - closing the pane over a notice would take the only place
                // the fix can be made away with it.
                self.draft = Some(self.settings.clone());
                Err(error)
            }
        }
    }

    /// Commit ONE field: the permission mode the composer chip selected.
    ///
    /// The turn build reads the committed settings, so a chip that wrote only
    /// the draft looked inert until something pressed Save. This copies exactly
    /// `workspace.agent.permission` across - never providers, the theme or any
    /// other unsaved draft field - saves, and leaves the value in both trees so
    /// a later Save cannot resurrect the old mode. On a refused write the
    /// in-memory value is put back, because a write that did not land must not
    /// be shown as the mode in force.
    pub(crate) fn commit_permission(&mut self, permission: &str) -> std::io::Result<()> {
        let previous = self.settings.workspace.agent.permission.clone();
        let drafted = self
            .draft
            .as_ref()
            .map(|draft| draft.workspace.agent.permission.clone());
        self.settings.workspace.agent.permission = permission.to_string();
        if let Some(draft) = self.draft.as_mut() {
            draft.workspace.agent.permission = permission.to_string();
        }
        self.settings.ensure_workspace();
        match self.settings.save() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.settings.workspace.agent.permission = previous;
                if let (Some(draft), Some(previous)) = (self.draft.as_mut(), drafted) {
                    draft.workspace.agent.permission = previous;
                }
                Err(error)
            }
        }
    }

    /// The settings being edited, falling back to the saved ones.
    pub(crate) fn draft(&self) -> &Settings {
        self.draft.as_ref().unwrap_or(&self.settings)
    }

    /// The settings being edited, creating the draft on first use.
    pub(crate) fn draft_mut(&mut self) -> &mut Settings {
        if self.draft.is_none() {
            self.draft = Some(self.settings.clone());
        }
        match self.draft.as_mut() {
            Some(draft) => draft,
            None => unreachable!("the draft was just created"),
        }
    }

    /// Switch one tool on or off in the draft.
    pub(crate) fn toggle_tool_off(&mut self, name: &str) {
        let draft = self.draft_mut();
        match draft
            .workspace
            .disabled
            .tools
            .iter()
            .position(|tool| tool == name)
        {
            Some(index) => {
                draft.workspace.disabled.tools.remove(index);
            }
            None => draft.workspace.disabled.tools.push(name.to_string()),
        }
    }

    /// Make one model the ACTIVE SESSION's choice.
    ///
    /// Session-scoped on purpose: the pair is stored on the session document,
    /// so a model chosen in one chat leaves every sibling alone and leaves
    /// `settings.workspace` untouched. A new session is not given a copy of
    /// this choice: it starts with no model and asks for one.
    ///
    /// A (provider, nickname) pair is stored whole or not at all. A half-made
    /// pair would resolve to nothing, so a blank half is refused here rather
    /// than written and reported later as an unknown name.
    pub(crate) fn select_model_from(&mut self, provider: &str, nickname: &str) {
        let (provider, nickname) = (provider.trim(), nickname.trim());
        if provider.is_empty() || nickname.is_empty() {
            return;
        }
        let Some(id) = self.active_id.clone() else {
            return;
        };
        if let Some(session) = self.store.session_mut(&id) {
            session.provider = Some(provider.to_string());
            session.model = Some(nickname.to_string());
        }
        // Saved like any other session mutation. `updated_at` deliberately does
        // not move: choosing a model is metadata, and a choice must not reorder
        // the reader's list under their hand.
        self.save_session(&id);
    }

    /// Choose the active session's model by nickname under the provider it
    /// already names, or the first provider that carries the nickname.
    ///
    /// The composer always passes the pair explicitly, so this is a test
    /// convenience: it looks in the saved tree AND the draft the settings pane
    /// edits, because a pane test seeds a provider in the draft.
    #[cfg(test)]
    pub(crate) fn select_model(&mut self, nickname: &str) {
        let chosen = self
            .effective()
            .model_ref
            .map(|reference| reference.provider);
        let find =
            |providers: &std::collections::BTreeMap<String, crate::settings::global::Provider>| {
                providers
                    .iter()
                    .find(|(_, provider)| provider.models.contains_key(nickname))
                    .map(|(id, _)| id.clone())
            };
        let provider = chosen
            .or_else(|| find(&self.settings.global.providers))
            .or_else(|| find(&self.draft().global.providers));
        if let Some(provider) = provider {
            self.select_model_from(&provider, nickname);
        }
    }

    /// Set the reasoning effort the composer's menu offers.
    ///
    /// `None` hands the choice back to the model's own declared effort; the
    /// literal `off` asks for NO effort on the wire (the reference's "Off turns
    /// reasoning off and needs no value"), which `caps::build_agent` maps to an
    /// absent field rather than the string.
    pub(crate) fn set_reasoning_effort(&mut self, level: Option<&str>) {
        let value = level
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(str::to_string);
        self.settings.workspace.agent.reasoning_effort = value.clone();
        if let Some(draft) = self.draft.as_mut() {
            draft.workspace.agent.reasoning_effort = value;
        }
        let _ = self.settings.save();
    }
    /// Save a provider into the global tree, or say why it cannot be saved.
    ///
    /// A refused call changes nothing, so pressing create with an id the tree
    /// already holds no longer quietly rewrites the provider behind it.
    ///
    /// A new provider may start with no models: the add card asks only for the
    /// provider's own facts, and the provider's editor adds model rows later.
    /// Its catalogue is therefore empty until that editor is used.
    pub(crate) fn add_custom_provider(
        &mut self,
        id: &str,
        name: &str,
        base_url: &str,
        models: &[crate::settings::global::ModelEntry],
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        if id.trim().is_empty() {
            return Err(crate::settings::provider::ProviderRefusal::NameEmpty);
        }
        if self.draft_mut().global.providers.contains_key(id) {
            return Err(crate::settings::provider::ProviderRefusal::IdTaken);
        }
        if base_url.trim().is_empty() {
            return Err(crate::settings::provider::ProviderRefusal::NeedsBaseUrl);
        }
        if let Some(refusal) = crate::settings::provider::base_url_refusal(base_url) {
            return Err(refusal);
        }
        self.draft_mut()
            .global
            .add_custom_provider(id, name, base_url, models);
        Ok(())
    }

    /// Add a shipped family provider, taking its own endpoint and protocol.
    ///
    /// The catalogue route of the add card lands here: an id the tree already
    /// holds is refused, because the id is the registry key the engine
    /// registers the vendor under and two of them cannot share it.
    pub(crate) fn add_shipped_provider(
        &mut self,
        id: &str,
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        if self.draft_mut().global.providers.contains_key(id) {
            return Err(crate::settings::provider::ProviderRefusal::IdTaken);
        }
        let Some((_, base_url, _)) = crate::settings::global::built_in(id) else {
            return Err(crate::settings::provider::ProviderRefusal::NeedsModel);
        };
        self.draft_mut().global.add_provider(id, "", base_url, &[]);
        Ok(())
    }

    /// Switch one input type on one model leaf of a provider.
    pub(crate) fn toggle_model_input(&mut self, provider: &str, key: &str, kind: &str) {
        self.draft_mut().global.set_input_type(provider, key, kind);
        self.provider_refusal = None;
    }
    /// Edit the connection and the model leaves of a provider already in the
    /// tree.
    pub(crate) fn set_provider_connection(&mut self, id: &str, base_url: &str) {
        self.draft_mut().global.set_connection(id, base_url);
    }

    /// Name the vault entry a provider reads its API key from.
    ///
    /// This writes the DRAFT like every other provider field, so the choice is
    /// committed by the dialog's own Save and dropped by its Cancel.
    pub(crate) fn set_provider_api_key_vault(&mut self, id: &str, entry: Option<&str>) {
        self.draft_mut().global.set_api_key_vault(id, entry);
    }

    /// Rename a provider. The id is the wire prefix and never changes; the
    /// display name is the pane's to edit.
    pub(crate) fn set_provider_name(&mut self, id: &str, name: &str) {
        self.draft_mut().global.set_provider_name(id, name);
    }

    /// Add one model leaf under a provider, or say why it is refused.
    pub(crate) fn add_model(
        &mut self,
        provider: &str,
        nickname: &str,
        display: &str,
        model: &str,
        api_mode: &str,
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        self.draft_mut()
            .global
            .add_model(provider, nickname, display, model, api_mode)
    }

    /// Replace one model leaf in place.
    pub(crate) fn update_model(
        &mut self,
        provider: &str,
        nickname: &str,
        entry: &crate::settings::global::ModelEntry,
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        self.draft_mut()
            .global
            .update_model(provider, nickname, entry)
    }

    /// Drop one model leaf; the connection is left alone.
    pub(crate) fn remove_model(&mut self, provider: &str, nickname: &str) -> bool {
        self.draft_mut().global.remove_model(provider, nickname)
    }

    /// Load a provider into the form and put the pane into edit mode.
    pub(crate) fn begin_provider_edit(&mut self, id: &str) {
        self.editing_provider = Some(id.to_string());
        self.provider_refusal = None;
        self.removing_provider = None;
    }

    /// Drop a provider from the global tree.
    ///
    /// Sessions that chose a model under it keep the reference and report it as
    /// unavailable at send, rather than being quietly repointed at another
    /// provider: the reader who chose it is the one who can correct it.
    pub(crate) fn remove_provider(&mut self, name: &str) {
        self.draft_mut().global.remove_provider(name);
        self.removing_provider = None;
    }

    /// Ask for confirmation before dropping a provider.
    pub(crate) fn begin_provider_remove(&mut self, name: &str) {
        self.removing_provider = Some(name.to_string());
    }

    /// Dismiss a pending delete confirmation.
    pub(crate) fn cancel_provider_remove(&mut self) {
        self.removing_provider = None;
    }

    /// Turn concurrent tool dispatch for one assistant turn on or off.
    pub(crate) fn set_parallel_tool_calls(&mut self, parallel: bool) {
        self.draft_mut().workspace.agent.parallel_tool_calls = parallel;
    }

    /// Make one preset the default a session takes when it names none.
    /// Trust (or untrust) one preset's policy contribution.
    ///
    /// The write lands on the draft, so Cancel leaves the previous trust state
    /// in place; Save persists it with the preset declaration.
    pub(crate) fn set_preset_trusted(&mut self, id: &str, trusted: bool) {
        self.draft_mut()
            .global
            .presets
            .entry(id.to_string())
            .or_default()
            .trusted = trusted;
    }

    pub(crate) fn select_preset(&mut self, name: &str) {
        self.draft_mut().global.default_preset = name.to_string();
    }

    /// Create a custom preset from the safe read-only template and return its
    /// new id. The picker offers this from the Custom section; the card is
    /// drafed immediately and the reader can edit the declaration in the
    /// settings file.
    pub(crate) fn add_custom_preset(&mut self) -> String {
        let mut n = 1usize;
        let id = loop {
            let candidate = if n == 1 {
                "custom".to_string()
            } else {
                format!("custom-{n}")
            };
            if self.draft().global.preset(&candidate).is_none() {
                break candidate;
            }
            n += 1;
        };
        let preset = crate::settings::global::Preset {
            name: Some("Custom".to_string()),
            level: Some("read-only".to_string()),
            permission: Some("read-only".to_string()),
            read: Some(vec![".".to_string()]),
            write: Some(Vec::new()),
            deny: [".git", ".bsh", ".env", "secrets", ".ssh", ".aws"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            plugins: vec!["files".to_string(), "plan".to_string()],
            tools: Some(vec![
                "read_file".to_string(),
                "list_dir".to_string(),
                "update_plan".to_string(),
            ]),
            skills: Some(Vec::new()),
            mcps: Some(Vec::new()),
            extensions: Some(Vec::new()),
            egress: Some(Vec::new()),
            web_search: Some(true),
            builtin_policy: vec![
                "deny-secrets".to_string(),
                "read-only".to_string(),
                "no-shell".to_string(),
                "no-egress".to_string(),
            ],
            system_prompt: Some(
                "Describe what this custom preset should do; edit permission, paths, tools and persona in the settings file.".to_string(),
            ),
            order: Some(100),
            ..Default::default()
        };
        self.draft_mut().global.presets.insert(id.clone(), preset);
        id
    }

    /// The preset the active session names explicitly, if any.
    pub(crate) fn active_session_preset(&self) -> Option<String> {
        self.active_session()
            .and_then(|session| session.agent_preset.clone())
            .filter(|preset| !preset.trim().is_empty())
    }

    /// Enable one preset for THIS session, or clear it back to the default.
    ///
    /// A session's own preset outlives the blank window and is what every turn
    /// of that chat builds against; it never changes the global default. The
    /// next turn reads it through `Session::choice`.
    pub(crate) fn select_session_preset(&mut self, id: &str, preset: &str) {
        let preset = preset.trim();
        if let Some(session) = self.store.session_mut(id) {
            session.agent_preset = (!preset.is_empty()).then(|| preset.to_string());
            session.updated_at = now();
        }
        self.save_session(id);
    }

    /// Switch one plugin on or off in the draft.
    pub(crate) fn toggle_plugin_off(&mut self, name: &str) {
        let draft = self.draft_mut();
        match draft
            .workspace
            .disabled
            .extensions
            .iter()
            .position(|plugin| plugin == name)
        {
            Some(index) => {
                draft.workspace.disabled.extensions.remove(index);
            }
            None => draft.workspace.disabled.extensions.push(name.to_string()),
        }
    }

    /// The trust decisions recorded this run, oldest first.
    ///
    /// The pane shows the state, not the log; this is the test-facing copy of
    /// the ring. Bounded by [`TRUST_AUDIT_LIMIT`].
    #[cfg(test)]
    pub(crate) fn trust_audit(&self) -> Vec<TrustDecision> {
        self.trust_audit.iter().cloned().collect()
    }

    /// Append one trust decision, trimming the oldest so the ring stays bounded.
    fn record_trust(
        &mut self,
        allow: bool,
        id: &str,
        path: &str,
        launches: &[crate::mcp::McpLaunch],
    ) {
        while self.trust_audit.len() >= TRUST_AUDIT_LIMIT {
            self.trust_audit.pop_front();
        }
        self.trust_audit.push_back(TrustDecision {
            extension_id: id.to_string(),
            path: path.to_string(),
            commands: launches
                .iter()
                .map(|launch| launch.command.clone())
                .collect(),
            at: now(),
            allow,
        });
    }

    /// The declared MCP launches of one installed package, with its stored path.
    ///
    /// `None` when the package is not installed, has no path, or declares no
    /// MCP server - the three cases where a trust decision has nothing to
    /// approve. Env values are absent from the launches by construction.
    fn declared_launches(&self, id: &str) -> Option<(String, Vec<crate::mcp::McpLaunch>)> {
        let record = self
            .draft()
            .global
            .extensions
            .iter()
            .find(|record| record.id == id)?;
        let path = record
            .path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())?
            .to_string();
        let root = record.path_root()?;
        let launches = crate::mcp::declared_servers(&root, &record.id)
            .iter()
            .map(crate::mcp::ExtensionMcp::launch)
            .collect::<Vec<_>>();
        (!launches.is_empty()).then_some((path, launches))
    }

    /// Approve one installed package's declared commands, enable it and persist.
    ///
    /// This is the handler the pane's Trust and enable button calls: the store
    /// marks the package trusted AT THE PATH it is installed at, the workspace
    /// disabled list drops it, and the decision is written immediately - a
    /// package's commands are concrete state on disk, not a preference waiting
    /// for the dialog's Save.
    ///
    /// The audit row is appended BEFORE the pool reconciles, so the approval is
    /// recorded even if the child then fails to start. If the record cannot be
    /// saved, the in-memory flag is reverted and no pool reconcile runs: a
    /// decision that did not reach disk must not start a process for this
    /// session either.
    pub(crate) fn trust_extension(&mut self, id: &str) -> bool {
        let Some((path, launches)) = self.declared_launches(id) else {
            return false;
        };
        let was_disabled = self
            .draft()
            .workspace
            .disabled
            .extensions
            .iter()
            .any(|disabled| disabled == id);
        {
            let draft = self.draft_mut();
            let Some(record) = draft
                .global
                .extensions
                .iter_mut()
                .find(|record| record.id == id)
            else {
                return false;
            };
            record.trusted = true;
            record.trusted_path = Some(path.clone());
            draft
                .workspace
                .disabled
                .extensions
                .retain(|disabled| disabled != id);
        }
        self.record_trust(true, id, &path, &launches);
        if let Err(error) = self.persist_extension_state() {
            log::warn!("trust decision for extension '{id}' was not saved: {error}");
            let draft = self.draft_mut();
            if let Some(record) = draft
                .global
                .extensions
                .iter_mut()
                .find(|record| record.id == id)
            {
                record.trusted = false;
                record.trusted_path = None;
            }
            if was_disabled
                && !draft
                    .workspace
                    .disabled
                    .extensions
                    .iter()
                    .any(|disabled| disabled == id)
            {
                draft.workspace.disabled.extensions.push(id.to_string());
            }
            return false;
        }
        true
    }

    /// Record that the reader DECLINED one package's declared commands.
    ///
    /// Nothing is written to the setting: the package stays untrusted, which is
    /// already the state. The audit ring keeps the refusal, because a gate that
    /// only records approvals cannot answer "was this package ever offered?".
    pub(crate) fn decline_trust(&mut self, id: &str) -> bool {
        let Some((path, launches)) = self.declared_launches(id) else {
            return false;
        };
        self.record_trust(false, id, &path, &launches);
        true
    }

    /// Register one market plugin in the draft extension inventory.
    ///
    /// Ownership: the MARKET row is what installs, so the record carries the
    /// metadata the row already read (name, display name, description, version,
    /// standard, source label, install target) plus the package directory and
    /// whether that directory is BOS's managed copy. The path is authoritative:
    /// the pane re-reads the manifest from it, so a stale cached field can never
    /// make a package loadable or not.
    ///
    /// Installing is a draft write, like every other pane edit, but the caller
    /// persists it immediately through `persist_extension_state` - an installed
    /// package is concrete state already on disk, not a preference waiting for
    /// Save. A package whose id is already installed is left alone, so a
    /// repeated press cannot create a duplicate the settings validation would
    /// reject on Save.
    pub(crate) fn install_extension(
        &mut self,
        plugin: &crate::market::MarketPlugin,
        path: &std::path::Path,
        managed: bool,
        install_target: &str,
    ) -> bool {
        let draft = self.draft_mut();
        if draft
            .global
            .extensions
            .iter()
            .any(|extension| extension.id == plugin.id)
        {
            return false;
        }
        draft
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: plugin.id.clone(),
                // The manifest's package name is the id for both standards;
                // the display name is the market row's own title.
                name: plugin.id.clone(),
                display_name: plugin.name.clone(),
                description: plugin.description.clone(),
                version: plugin.version.clone(),
                standard: crate::settings::global::InstalledStandard::from(plugin.standard),
                source_label: plugin.source_label.clone(),
                install_target: install_target.to_string(),
                path: Some(path.to_string_lossy().to_string()),
                managed,
                installed_at: crate::settings::unix_now(),
                // A fresh package starts untrusted: its declared commands (MCP
                // servers) do not run until the reader approves the directory.
                trusted: false,
                trusted_path: None,
            });
        draft
            .workspace
            .disabled
            .extensions
            .retain(|disabled| disabled != &plugin.id);
        // A package can carry directories the skill scan reads, and the
        // settings fields it changed do not describe them: forget the cached
        // catalogue so the next resolution sees the install.
        draft.invalidate_skills();
        true
    }

    /// Add one remote market index URL to the draft.
    ///
    /// The URL is validated and normalized first; a duplicate or malformed URL
    /// is refused with a sentence the pane can show. Save remains the commit.
    pub(crate) fn add_market_index(&mut self, raw: &str) -> Result<String, String> {
        let url = crate::market::validate_index_url(raw)?;
        let draft = self.draft_mut();
        if draft
            .global
            .market_indexes
            .iter()
            .any(|existing| existing == &url)
        {
            return Err("that market index is already configured".to_string());
        }
        draft.global.market_indexes.push(url.clone());
        Ok(url)
    }

    /// Remove one remote market index URL from the draft.
    pub(crate) fn remove_market_index(&mut self, url: &str) -> bool {
        let draft = self.draft_mut();
        let before = draft.global.market_indexes.len();
        draft
            .global
            .market_indexes
            .retain(|existing| existing != url);
        draft.global.market_indexes.len() != before
    }

    /// Remove one installed market plugin from the draft inventory.
    ///
    /// Only an entry that carries a package path can be uninstalled: the
    /// built-in bundles have no directory to remove, so a stray call cannot
    /// hide one from the pane. The workspace's disabled entry goes with it.
    ///
    /// Ownership: a MANAGED package (one the market downloaded into
    /// `~/.bos/bsh/extensions`) has its directory deleted; a local package
    /// registered in place is the reader's own directory and is never deleted.
    /// `remove_managed` also refuses a path outside the managed root, so a
    /// hand-edited record cannot turn `managed = true` into an arbitrary
    /// directory delete.
    pub(crate) fn uninstall_extension(&mut self, id: &str) -> bool {
        self.uninstall_extension_in(&crate::settings::dir().join("extensions"), id)
    }

    /// Uninstall against an explicit managed root, for tests and for a
    /// sandboxed home. The production path supplies `~/.bos/bsh/extensions`.
    pub(crate) fn uninstall_extension_in(
        &mut self,
        managed_root: &std::path::Path,
        id: &str,
    ) -> bool {
        let draft = self.draft_mut();
        let Some(index) = draft
            .global
            .extensions
            .iter()
            .position(|extension| extension.id == id && extension.path.is_some())
        else {
            return false;
        };
        let record = draft.global.extensions.remove(index);
        draft
            .workspace
            .disabled
            .extensions
            .retain(|disabled| disabled != id);
        if record.managed {
            if let Some(raw) = record.path {
                let path = crate::caps::expand_path(&raw);
                crate::market::remove_managed(managed_root, &path);
            }
        }
        // The package's directories are gone; the cached catalogue still
        // describes them until this says otherwise.
        draft.invalidate_skills();
        true
    }

    /// Commit the extension inventory and its per-workspace disabled list
    /// without committing unrelated draft settings.
    ///
    /// Installing, uninstalling, enabling and disabling are concrete package
    /// actions, not preferences the reader expects to have to Save: the files
    /// are already on disk, and a restart must find the same inventory. Only
    /// these two fields are copied out of the draft; a provider or theme edit
    /// still waits for the dialog's Save.
    pub(crate) fn persist_extension_state(&mut self) -> std::io::Result<()> {
        let mut next = self.settings.clone();
        next.global.extensions = self.draft().global.extensions.clone();
        next.workspace.disabled.extensions = self.draft().workspace.disabled.extensions.clone();
        next.save()?;
        self.settings = next;
        // The pool is configuration-derived: a newly enabled MCP plugin must
        // connect now, and a disabled one must drop its child before the next
        // turn. Centralized here so every persistence caller gets the same pool.
        self.refresh_mcp();
        Ok(())
    }

    /// Commit the skills this workspace switched off, without committing any
    /// other draft edit.
    ///
    /// A skill's switch is a concrete per-workspace fact, not a preference the
    /// reader expects to Save: quitting and relaunching must find the same
    /// list. Only `workspace.disabled.skills` is copied out of the draft; a
    /// provider, theme, market-URL or even an executor-directory edit still
    /// waits for the dialog's Save. The write goes to the same two documents
    /// Save writes, so a reload cannot tell which path wrote it.
    pub(crate) fn persist_disabled_skills(&mut self) -> std::io::Result<()> {
        let mut next = self.settings.clone();
        next.workspace.disabled.skills = self.draft().workspace.disabled.skills.clone();
        next.save()?;
        self.settings = next;
        Ok(())
    }

    /// Commit ONE executor directory's enabled flag, leaving every other draft
    /// edit alone.
    ///
    /// The saved source list is re-read and only the named entry's flag is
    /// changed: a directory the reader added or removed in the dialog is NOT
    /// written by moving a sibling's switch. A name that exists only in the
    /// draft (an add nobody Saved) has no saved row to flip, so nothing is
    /// written for it.
    pub(crate) fn persist_skill_source_enabled(&mut self, name: &str) -> std::io::Result<()> {
        let enabled = self
            .draft()
            .global
            .skill_sources()
            .into_iter()
            .find(|source| source.name == name)
            .map(|source| source.enabled);
        let Some(enabled) = enabled else {
            return Ok(());
        };
        let mut next = self.settings.clone();
        let mut sources = next.global.skill_sources();
        let Some(saved) = sources.iter_mut().find(|source| source.name == name) else {
            return Ok(());
        };
        if saved.enabled == enabled {
            return Ok(());
        }
        saved.enabled = enabled;
        next.global.set_skill_sources(sources);
        next.save()?;
        self.settings = next;
        Ok(())
    }

    /// Switch one skill on or off in the draft.
    pub(crate) fn toggle_skill_off(&mut self, name: &str) {
        let draft = self.draft_mut();
        match draft
            .workspace
            .disabled
            .skills
            .iter()
            .position(|skill| skill == name)
        {
            Some(index) => {
                draft.workspace.disabled.skills.remove(index);
            }
            None => draft.workspace.disabled.skills.push(name.to_string()),
        }
    }
}

/// The reference's display-ready cache-hit share, as text.
///
/// Its `formatCacheHitPercent` rounds to whole percent with ties up, but never
/// lets a partial hit read as a full one: when rounding would reach 100 it
/// widens the precision until the miss shows, so 199 of 200 reads `99.5`. No
/// prompt input at all reads as unknown rather than zero.
pub(crate) fn cache_hit_percent(cache_read: u64, prompt: u64) -> Option<String> {
    if prompt == 0 {
        return None;
    }
    let missed = prompt.saturating_sub(cache_read);
    if missed == 0 {
        return Some("100".to_string());
    }
    let units = (cache_read as u128 * 200 + prompt as u128) / (prompt as u128 * 2);
    if units < 100 {
        return Some(units.to_string());
    }
    // A partial hit whose whole percent rounds to 100: walk the precision out
    // until the remaining miss shows, the way the reference does.
    let mut places = 1usize;
    let mut gap = missed as u128 * 200;
    let tens = (prompt / 10) as u128;
    while gap <= tens {
        gap *= 10;
        places += 1;
    }
    let ones = (prompt % 10) as u128;
    let mut loss = 5u64;
    for candidate in 1..5u64 {
        let factor = candidate as u128 * 2 + 1;
        if gap <= factor * tens + factor * ones / 10 {
            loss = candidate;
            break;
        }
    }
    Some(format!("99.{}{}", "9".repeat(places - 1), 10 - loss))
}

/// A token count with the reference's own digit grouping: `17,734`.
pub(crate) fn exact_tokens(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Compact token counts the way the reference's own `formatTokens` reads them:
/// `947`, `12.3K`, `518K`, `1.2M`.
///
/// Under a thousand it prints the count; above that it scales to K or M, where
/// a scaled value of 100 or more rounds to a whole number and a smaller one
/// keeps a single decimal. The reference localizes the two suffix keys, but its
/// own doc comment pins the shipped values to K and M.
pub(crate) fn human_tokens(tokens: u64) -> String {
    fn scaled(value: u64, scale: u64) -> String {
        let whole = value / scale;
        let remainder = value % scale;
        if whole >= 100 {
            return (whole + u64::from(remainder * 2 >= scale)).to_string();
        }
        let tenths = whole * 10 + (remainder * 10 + scale / 2) / scale;
        if tenths.is_multiple_of(10) {
            (tenths / 10).to_string()
        } else {
            format!("{}.{}", tenths / 10, tenths % 10)
        }
    }
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{}K", scaled(tokens, 1_000))
    } else {
        format!("{}M", scaled(tokens, 1_000_000))
    }
}

/// The reference's own rounding of an occupancy share, capped at 100.
///
/// Its `contextPressure` computes `Math.min(100, Math.round(used / window *
/// 100))`, so a 45.4% occupancy reads 45% in the ring and in the panel alike. A
/// missing budget or an unmeasured prompt reads as unknown rather than zero.
pub(crate) fn context_share(prompt: u64, budget: u64) -> Option<u64> {
    (budget > 0 && prompt > 0)
        .then(|| ((prompt as f64 / budget as f64) * 100.0).round().min(100.0) as u64)
}

/// Whether a session title matches a case-insensitive substring query.
///
/// An empty or whitespace-only query matches everything, so the filter stays
/// inert while the search field is empty.
pub(crate) fn matches(query: &str, title: &str) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty() || title.to_lowercase().contains(&needle)
}

/// One image attached to the next user turn.
///
/// bsh stores the payload already base64-encoded, because that is the form
/// both providers want and the file is read once, at attach time.
#[derive(Clone)]
pub(crate) struct Attachment {
    /// The file name, shown on the composer chip and in the transcript.
    pub(crate) name: String,
    /// The MIME type declared to the provider.
    pub(crate) mime: String,
    /// The base64 payload.
    pub(crate) base64: String,
}

impl Attachment {
    /// Read an image file into an attachment.
    ///
    /// Returns None for anything that is not one of the image types bsh sends,
    /// so the caller can fall back to mentioning the path as text.
    pub(crate) fn from_path(path: &std::path::Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        let mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => return None,
        };
        let bytes = std::fs::read(path).ok()?;
        Some(Self {
            name: path.file_name()?.to_string_lossy().to_string(),
            mime: mime.to_string(),
            base64: base64_encode(&bytes),
        })
    }
}

/// Encode bytes as standard base64 with padding.
///
/// Hand-rolled rather than pulled from a crate: bsh has one call site and the
/// workspace ships no base64 dependency, the same trade the UTC stamp made.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Build the content of one user turn from its text and its attachments.
///
/// A turn with no attachment stays plain text. One with attachments becomes the
/// multimodal form both vendors already map onto their own image blocks.
/// The directive a plan-mode turn carries, in front of the reader's own text.
///
/// Plan mode is a promise about what the model may DO, so the instruction has
/// to travel on the request itself: the reader's text stays what the transcript
/// shows, while the model receives the plan-first preamble.
pub(crate) const PLAN_DIRECTIVE: &str = "[Plan mode] Inspect what the task needs and reply with a concrete, step-by-step plan. Do not edit files, run mutating commands, or begin the work until the reader approves the plan.";

/// One typed slash command line, parsed the way the reference's command
/// registry parses it: the name is the first whitespace-delimited token, and
/// everything after it is the argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashLine {
    /// `/plan ...`
    Plan(PlanRequest),
    /// `/goal ...`
    Goal(GoalRequest),
    /// `/compact` with its raw argument (the command takes none).
    Compact(String),
}

/// What a `/plan` line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanRequest {
    /// Bare `/plan`: enter plan mode.
    On,
    /// `/plan off`: leave it without model input.
    Off,
    /// `/plan <task>`: enter plan mode and submit the task as the turn.
    Task(String),
}

/// What a `/goal` line asked for.
///
/// The control words are recognized only when they fill the whole argument, so
/// `/goal pause after verification` creates that literal objective.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalRequest {
    /// Bare `/goal`: report the goal.
    Show,
    /// `/goal <objective>`: create and arm.
    Create(String),
    /// `/goal edit <objective>`: rewrite the objective in place.
    Edit(String),
    /// `/goal pause`
    Pause,
    /// `/goal resume`
    Resume,
    /// `/goal clear`
    Clear,
    /// `/goal edit` with no replacement.
    InvalidEdit,
}

/// Parse a typed slash command line.
///
/// The commands the reference's command plane owns here take a line: `/plan`
/// (hint `[off|message]`), `/goal`
/// (`[<objective>|clear|edit <objective>|pause|resume]`) and `/compact`.
/// `/compact` takes NO arguments - the shipped command returns its usage line
/// when `rawInput.trim()` is non-empty - so the raw argument travels to the
/// handler, which owns that refusal rather than the parser. Anything else, or a
/// leading word that merely starts with one of them, is not a command line and
/// is sent as a normal message.
pub(crate) fn parse_slash_line(text: &str) -> Option<SlashLine> {
    let text = text.trim();
    let (name, rest) = match text.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (text, ""),
    };
    match name {
        "/plan" => Some(SlashLine::Plan(match rest {
            "" => PlanRequest::On,
            "off" => PlanRequest::Off,
            task => PlanRequest::Task(task.to_string()),
        })),
        "/goal" => Some(SlashLine::Goal(parse_goal_request(rest))),
        "/compact" => Some(SlashLine::Compact(rest.to_string())),
        _ => None,
    }
}

/// The `/goal` grammar: exact control words, `edit <objective>`, else an
/// objective.
fn parse_goal_request(rest: &str) -> GoalRequest {
    if rest.is_empty() {
        return GoalRequest::Show;
    }
    match rest.to_lowercase().as_str() {
        "clear" => return GoalRequest::Clear,
        "pause" => return GoalRequest::Pause,
        "resume" => return GoalRequest::Resume,
        "edit" => return GoalRequest::InvalidEdit,
        _ => {}
    }
    if let Some(end) = rest.find(char::is_whitespace) {
        if rest[..end].eq_ignore_ascii_case("edit") {
            let objective = rest[end..].trim();
            return if objective.is_empty() {
                GoalRequest::InvalidEdit
            } else {
                GoalRequest::Edit(objective.to_string())
            };
        }
    }
    GoalRequest::Create(rest.to_string())
}

/// The instruction one GOAL ROUND carries.
///
/// This is the reference driver's retained prompt: the JSON-quoted objective,
/// the round number against the cap, and the working instructions. A goal round
/// has no human sentence to answer, so the round itself is the prompt; the
/// session's Goal.rounds is the durable half.
pub(crate) fn goal_round_input(objective: &str, round: u32, max_rounds: u32) -> String {
    let objective = serde_json::to_string(objective)
        .unwrap_or_else(|_| format!("\"{}\"", objective.replace('"', "\\\"")));
    format!(
        "<goal_round>\nObjective: {objective}\nRound: {round}/{max_rounds}\n\nContinue working toward the objective in this same session. Treat the current workspace, tool results, and durable session state as authoritative; inspect them instead of assuming earlier narration is still current. Make concrete progress and verify the result. Before claiming completion, gather evidence that the whole objective is achieved, read the current goal, and mark it complete. If work remains, leave the goal active for the next round. Follow the configured goal-tool policy before reporting a blocker.\n</goal_round>"
    )
}

/// Everything one request carries in front of the reader's own line.
///
/// The reference does NOT inject goal state into an ordinary model request: a
/// goal mutation is a UI and durable-log fact, and the only goal text a model
/// sees is the round prompt plus any attachment message the command submitted.
/// So this adds only the reference guidance (how to resolve an @token) and plan
/// mode (what it may do), each when it applies; an ordinary turn's input stays
/// exactly the reader's text.
pub(crate) fn turn_text(text: &str, plan_mode: bool) -> String {
    let mut out = String::new();
    if crate::reference::has_references(text) {
        out.push_str(crate::reference::FILE_REFERENCE_PROMPT);
        out.push_str("\n\n");
    }
    if plan_mode {
        out.push_str(PLAN_DIRECTIVE);
        out.push_str("\n\n");
    }
    out.push_str(text);
    out
}

pub(crate) fn turn_input(text: &str, attachments: &[Attachment]) -> react::llm::Content {
    if attachments.is_empty() {
        return react::llm::Content::Text(text.to_string());
    }
    let mut parts = vec![react::llm::ContentPart::Text {
        text: text.to_string(),
    }];
    for attachment in attachments {
        if let react::llm::Content::Parts(mut image) =
            react::llm::Content::binary(attachment.mime.clone(), attachment.base64.clone())
        {
            parts.append(&mut image);
        }
    }
    react::llm::Content::Parts(parts)
}

/// The text a turn shows in the transcript.
///
/// gpui-kit has no image view here, so an attachment is named rather than drawn;
/// this is bsh own presentation and is recorded as such.
pub(crate) fn display_text(text: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let names: Vec<String> = attachments
        .iter()
        .map(|a| format!("[image: {}]", a.name))
        .collect();
    format!("{}{}{}", text, "\n", names.join(" "))
}

/// The fixed text the /goal command puts before the attachments it admits.
///
/// The reference submits one ordinary user message carrying the admitted image
/// and file blocks plus this exact sentence; later goal rounds read it from
/// session history, and the goal domain stores no attachment state.
pub(crate) const GOAL_ATTACHMENT_NOTE: &str = "Reference attachments for the goal objective.";

/// The model-visible content of one stored message.
///
/// A stored user row with images replays as the same multimodal content its live
/// turn sent: the text plus the data-URL blocks. An empty row contributes
/// nothing. This is what makes the /goal attachment message readable by a later
/// goal round instead of only visible in the transcript.
pub(crate) fn message_content(message: &ChatMessage) -> Option<react::llm::Content> {
    if message.images.is_empty() {
        return (!message.text.is_empty()).then(|| react::llm::Content::Text(message.text.clone()));
    }
    let mut parts: Vec<react::llm::ContentPart> = Vec::new();
    if !message.text.is_empty() {
        parts.push(react::llm::ContentPart::Text {
            text: message.text.clone(),
        });
    }
    for url in &message.images {
        let Some((mime, base64)) = parse_data_url(url) else {
            continue;
        };
        if let react::llm::Content::Parts(mut block) =
            react::llm::Content::binary(mime.to_string(), base64.to_string())
        {
            parts.append(&mut block);
        }
    }
    (!parts.is_empty()).then_some(react::llm::Content::Parts(parts))
}

/// Split a data URL into its MIME type and base64 payload.
fn parse_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, payload) = rest.split_once(";base64,")?;
    (!mime.is_empty() && !payload.is_empty()).then_some((mime, payload))
}

/// The `(workspace id, root)` pairs the session store scatters sessions across.
///
/// The store is rooted per workspace, so a session file lives beside the
/// workspace it belongs to; the registry is what turns an id into a path.
fn session_roots(settings: &Settings) -> Vec<(String, String)> {
    settings
        .global
        .workspaces
        .iter()
        .map(|workspace| (workspace.id.clone(), workspace.root.clone()))
        .collect()
}
#[cfg(test)]
mod tests {
    fn image_fixture() -> super::Attachment {
        super::Attachment {
            name: "shot.png".to_string(),
            mime: "image/png".to_string(),
            base64: "Zg==".to_string(),
        }
    }

    /// The /plan and /goal line grammar is the reference's command registry
    /// grammar: exact control words only when they fill the argument, every
    /// other suffix an objective, and `/plan off` the one non-objective word.
    #[test]
    fn the_slash_command_line_grammar_is_the_references() {
        use super::{parse_slash_line, GoalRequest, PlanRequest, SlashLine};
        assert_eq!(
            parse_slash_line("/plan"),
            Some(SlashLine::Plan(PlanRequest::On))
        );
        assert_eq!(
            parse_slash_line("  /plan   "),
            Some(SlashLine::Plan(PlanRequest::On))
        );
        assert_eq!(
            parse_slash_line("/plan off"),
            Some(SlashLine::Plan(PlanRequest::Off))
        );
        assert_eq!(
            parse_slash_line("/plan build the thing"),
            Some(SlashLine::Plan(PlanRequest::Task(
                "build the thing".to_string()
            )))
        );
        assert_eq!(parse_slash_line("/planner hi"), None);
        assert_eq!(parse_slash_line("just a message"), None);
        assert_eq!(parse_slash_line("mail a@b.com"), None);

        assert_eq!(
            parse_slash_line("/goal"),
            Some(SlashLine::Goal(GoalRequest::Show))
        );
        assert_eq!(
            parse_slash_line("/goal pause"),
            Some(SlashLine::Goal(GoalRequest::Pause))
        );
        assert_eq!(
            parse_slash_line("/goal PAUSE"),
            Some(SlashLine::Goal(GoalRequest::Pause)),
            "control words are case-insensitive"
        );
        assert_eq!(
            parse_slash_line("/goal pause after verification"),
            Some(SlashLine::Goal(GoalRequest::Create(
                "pause after verification".to_string()
            ))),
            "a control word only acts when it fills the whole argument"
        );
        assert_eq!(
            parse_slash_line("/goal edit a better objective"),
            Some(SlashLine::Goal(GoalRequest::Edit(
                "a better objective".to_string()
            )))
        );
        assert_eq!(
            parse_slash_line("/goal edit"),
            Some(SlashLine::Goal(GoalRequest::InvalidEdit))
        );
        assert_eq!(
            parse_slash_line("/goal clear"),
            Some(SlashLine::Goal(GoalRequest::Clear))
        );
        assert_eq!(
            parse_slash_line("/goal resume"),
            Some(SlashLine::Goal(GoalRequest::Resume))
        );
        assert_eq!(
            parse_slash_line("/goal CLEAR"),
            Some(SlashLine::Goal(GoalRequest::Clear))
        );
        assert_eq!(
            parse_slash_line("/goal ReSuMe"),
            Some(SlashLine::Goal(GoalRequest::Resume))
        );
        assert_eq!(
            parse_slash_line("/goal EdIt a better objective"),
            Some(SlashLine::Goal(GoalRequest::Edit(
                "a better objective".to_string()
            )))
        );
        assert_eq!(
            parse_slash_line("/goal edit\ta better objective"),
            Some(SlashLine::Goal(GoalRequest::Edit(
                "a better objective".to_string()
            ))),
            "edit is recognized before any Unicode whitespace"
        );
        assert_eq!(
            parse_slash_line("/goal edit \u{00a0}"),
            Some(SlashLine::Goal(GoalRequest::InvalidEdit)),
            "an edit whose replacement trims to nothing is invalid"
        );
        assert_eq!(
            parse_slash_line("/goal editx"),
            Some(SlashLine::Goal(GoalRequest::Create("editx".to_string()))),
            "edit must be a whole word"
        );
        assert_eq!(
            parse_slash_line("/goal    "),
            Some(SlashLine::Goal(GoalRequest::Show)),
            "bare /goal with trailing whitespace shows"
        );
        assert_eq!(
            parse_slash_line("/goal edit   "),
            Some(SlashLine::Goal(GoalRequest::InvalidEdit))
        );
        assert_eq!(
            parse_slash_line("/goal clear now"),
            Some(SlashLine::Goal(GoalRequest::Create(
                "clear now".to_string()
            ))),
            "a control word plus an argument is an objective"
        );
        assert_eq!(parse_slash_line("/goal/clear"), None);
        assert_eq!(parse_slash_line("/goals clear"), None);
    }

    /// A scratch state with one active session, rooted under target.
    fn goal_state(label: &str) -> (super::AppState, String) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-goal-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = super::AppState::with_store(crate::store::Store::open_at(dir));
        let id = state.active_id.clone().expect("a session");
        (state, id)
    }

    /// Every /goal verb's transition and refusal, against the reference domain.
    #[test]
    fn goal_transitions_mirror_the_reference_domain() {
        use super::{GoalChange, GoalRefusal};
        use agent::goal::GoalState;
        let (mut state, id) = goal_state("transitions");

        // No goal: the two direct errors the command distinguishes.
        assert_eq!(state.pause_goal(&id), Err(GoalRefusal::Missing));
        assert_eq!(state.resume_goal(&id), Err(GoalRefusal::Missing));
        assert_eq!(state.edit_goal(&id, "anything"), Err(GoalRefusal::Missing));
        assert!(!state.clear_goal(&id));

        // create: trims, arms, starts at round zero, takes the default cap.
        assert_eq!(
            state.create_goal(&id, "  ship it  "),
            Ok(GoalChange::Created)
        );
        let created = state.goal(&id).expect("the goal");
        assert_eq!(created.objective, "ship it");
        assert_eq!(created.state, GoalState::Active);
        assert_eq!(created.rounds, 0);
        assert_eq!(created.max_rounds, 256);
        assert!(created.activation.is_armed());
        assert_eq!(
            state.create_goal(&id, "another"),
            Err(GoalRefusal::AlreadyExists(GoalState::Active))
        );

        // edit rewrites in place and keeps phase, rounds and activation.
        state
            .store
            .session_mut(&id)
            .expect("the session")
            .goal
            .as_mut()
            .expect("the goal")
            .rounds = 3;
        assert_eq!(
            state.edit_goal(&id, "ship it better"),
            Ok(GoalChange::Updated)
        );
        let edited = state.goal(&id).expect("the goal");
        assert_eq!(edited.objective, "ship it better");
        assert_eq!(edited.rounds, 3, "an edit keeps the admitted rounds");
        assert_eq!(edited.state, GoalState::Active);
        assert!(edited.activation.is_armed(), "an edit keeps activation");
        assert_eq!(
            state.edit_goal(&id, "   "),
            Err(GoalRefusal::InvalidObjective)
        );

        // pause only from active; it disarms.
        assert_eq!(state.pause_goal(&id), Ok(()));
        let paused = state.goal(&id).expect("the goal");
        assert_eq!(paused.state, GoalState::Paused);
        assert!(!paused.activation.is_armed());
        assert_eq!(state.pause_goal(&id), Err(GoalRefusal::InvalidTransition));
        assert_eq!(
            state.create_goal(&id, "another"),
            Err(GoalRefusal::AlreadyExists(GoalState::Paused))
        );

        // resume from paused: active and armed again.
        assert_eq!(state.resume_goal(&id), Ok(()));
        let resumed = state.goal(&id).expect("the goal");
        assert_eq!(resumed.state, GoalState::Active);
        assert!(resumed.activation.is_armed());
        assert_eq!(
            state.resume_goal(&id),
            Err(GoalRefusal::InvalidTransition),
            "an active armed goal is already running"
        );

        // blocked resumes and clears its reason.
        state
            .store
            .session_mut(&id)
            .expect("the session")
            .goal
            .as_mut()
            .expect("the goal")
            .block("round-limit", "the cap is spent");
        let blocked = state.goal(&id).expect("the goal");
        assert_eq!(blocked.state, GoalState::Blocked);
        assert!(!blocked.activation.is_armed());
        assert_eq!(state.resume_goal(&id), Ok(()), "a blocked goal can resume");
        let recovered = state.goal(&id).expect("the goal");
        assert_eq!(recovered.state, GoalState::Active);
        assert_eq!(recovered.blocked_reason, None);

        // complete: a completed goal is replaced, never edited in place.
        state
            .store
            .session_mut(&id)
            .expect("the session")
            .goal
            .as_mut()
            .expect("the goal")
            .complete();
        assert_eq!(
            state.resume_goal(&id),
            Err(GoalRefusal::InvalidTransition),
            "a completed goal cannot resume"
        );
        assert_eq!(state.pause_goal(&id), Err(GoalRefusal::InvalidTransition));
        assert_eq!(
            state.create_goal(&id, "fresh"),
            Ok(GoalChange::Replaced),
            "a completed goal may be replaced"
        );
        let fresh = state.goal(&id).expect("the goal");
        assert_eq!(fresh.objective, "fresh");
        assert_eq!(fresh.rounds, 0, "a replacement starts a fresh identity");
        assert!(fresh.activation.is_armed());
        state
            .store
            .session_mut(&id)
            .expect("the session")
            .goal
            .as_mut()
            .expect("the goal")
            .complete();
        assert_eq!(
            state.edit_goal(&id, "edited after complete"),
            Ok(GoalChange::Replaced),
            "editing a completed goal creates a fresh identity"
        );
        let edited = state.goal(&id).expect("the goal");
        assert_eq!(edited.objective, "edited after complete");
        assert_eq!(edited.rounds, 0);
        assert_eq!(edited.state, GoalState::Active);

        // An exhausted cap refuses resume even from paused.
        {
            let goal = state
                .store
                .session_mut(&id)
                .expect("the session")
                .goal
                .as_mut()
                .expect("the goal");
            goal.rounds = goal.max_rounds;
            assert!(goal.pause());
        }
        assert_eq!(
            state.resume_goal(&id),
            Err(GoalRefusal::InvalidTransition),
            "no rounds remain"
        );

        // clear always succeeds while a goal is current.
        assert!(state.clear_goal(&id));
        assert!(state.goal(&id).is_none());
        assert!(!state.clear_goal(&id));
    }

    /// A session reload keeps the goal and its rounds, and disarms it.
    #[test]
    fn a_goal_loaded_from_disk_is_active_but_disarmed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-goal-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id;
        {
            let mut state = super::AppState::with_store(crate::store::Store::open_at(dir.clone()));
            id = state.active_id.clone().expect("a session");
            assert!(state.create_goal(&id, "keep me").is_ok());
            state
                .store
                .session_mut(&id)
                .expect("the session")
                .goal
                .as_mut()
                .expect("the goal")
                .rounds = 7;
            state.save_session(&id);
        }
        let reopened = super::AppState::with_store(crate::store::Store::open_at(dir));
        assert_eq!(reopened.active_id.as_deref(), Some(id.as_str()));
        let goal = reopened.goal(&id).expect("the restored goal");
        assert_eq!(goal.objective, "keep me");
        assert_eq!(goal.rounds, 7);
        assert_eq!(goal.max_rounds, 256);
        assert_eq!(goal.state, agent::goal::GoalState::Active);
        assert!(
            !goal.activation.is_armed(),
            "a session start edge disarms continuation"
        );
        assert!(
            !reopened.goal_can_advance(&id),
            "a restored goal waits for an explicit resume"
        );
    }

    /// A compaction moves transcript rows out of the context without touching
    /// the goal: the durable record is independent of message history.
    #[test]
    fn a_compaction_does_not_touch_the_goal() {
        use crate::model::ChatMessage;
        let (mut state, id) = goal_state("compaction");
        assert!(state.create_goal(&id, "survive compaction").is_ok());
        let before = state.goal(&id).expect("the goal");
        {
            let session = state.store.session_mut(&id).expect("the session");
            session.messages = vec![
                ChatMessage::user("one"),
                ChatMessage::assistant(),
                ChatMessage::user("two"),
                ChatMessage::assistant(),
                ChatMessage::user("three"),
            ];
            session.messages[1].text = "reply one".to_string();
            session.messages[3].text = "reply two".to_string();
        }
        assert_eq!(state.mark_compacted(&id, 2), 2);
        let session = state.active_session().expect("the session");
        let request = state.request_transcript(session);
        assert_eq!(
            request.first().map(|message| message.text.as_str()),
            Some("two")
        );
        assert_eq!(
            state.goal(&id),
            Some(before),
            "the goal record is not part of the compacted history"
        );
        assert!(state.goal_can_advance(&id));
    }

    /// The next request opens on the checkpoint the summarizer stored, framed
    /// exactly the way the compaction contract frames it.
    #[test]
    fn the_next_request_replays_the_stored_checkpoint() {
        use crate::model::{ChatMessage, CompactionRecord};
        let (mut state, id) = goal_state("compact-replay");
        let summary = "## Current Work\n- replay me";
        {
            let session = state.store.session_mut(&id).expect("the session");
            session.messages = vec![
                ChatMessage::user("old one"),
                ChatMessage::assistant(),
                ChatMessage::user("old two"),
                ChatMessage::assistant(),
                ChatMessage::user("kept one"),
                ChatMessage::assistant(),
                ChatMessage::user("kept two"),
                ChatMessage::assistant(),
            ];
            session.messages[1].text = "reply one".to_string();
            session.messages[3].text = "reply two".to_string();
            session.messages[5].text = "kept reply one".to_string();
            session.messages[7].text = "kept reply two".to_string();
            session.compacted = 4;
            session.compactions.push(CompactionRecord {
                at: 1,
                items: 4,
                tokens: 321,
                summary: summary.to_string(),
                manual: true,
                after: 8,
            });
        }
        let session = state.active_session().expect("the session");
        assert!(state.has_replayed_checkpoint(session));
        let request = state.request_transcript(session);
        assert_eq!(request.len(), 5, "one checkpoint plus the four kept rows");
        assert_eq!(request[0].role, crate::model::Role::User);
        assert!(
            request[0]
                .text
                .starts_with(agent::agent::CHECKPOINT_PREAMBLE),
            "the reference's own preamble: {}",
            request[0].text
        );
        assert!(request[0].text.contains("<compacted-summary>"));
        assert!(request[0].text.contains(summary));
        assert!(request[0].text.ends_with("</compacted-summary>"));
        assert_eq!(request[1].text, "kept one");

        // The meter prices the request, so the checkpoint it carries is in the
        // message bucket. Leaving it out under-reported a compacted context by
        // exactly the summary's price.
        let checkpoint =
            crate::model::ChatMessage::user(agent::agent::context::render_checkpoint(summary))
                .priced_tokens();
        let kept: u64 = session.messages[4..]
            .iter()
            .map(crate::model::ChatMessage::priced_tokens)
            .sum();
        let priced = state
            .context_breakdown()
            .expect("the meter prices the window")
            .messages;
        assert_eq!(priced, checkpoint + kept, "the checkpoint is priced in");
    }

    /// A marker with no persisted slot draws at the end, never at the top, and
    /// a slot past the loaded window clamps to its end.
    #[test]
    fn a_legacy_compaction_marker_draws_at_the_end() {
        use crate::model::CompactionRecord;
        let mut record = CompactionRecord {
            at: 0,
            items: 93,
            tokens: 66_471,
            summary: "legacy".to_string(),
            manual: true,
            after: 0,
        };
        assert_eq!(super::compaction_marker_position(&record, 12, 0), 12);
        record.after = 5;
        assert_eq!(super::compaction_marker_position(&record, 12, 0), 5);
        record.after = 99;
        assert_eq!(super::compaction_marker_position(&record, 12, 0), 12);
        // A window that starts at 40 maps a global slot into it.
        record.after = 45;
        assert_eq!(super::compaction_marker_position(&record, 12, 40), 5);
        record.after = 10;
        assert_eq!(
            super::compaction_marker_position(&record, 12, 40),
            0,
            "a slot above the window clamps to its top"
        );
    }

    /// The agent's removed-message count maps onto the stored transcript,
    /// including the synthetic checkpoint head.
    #[test]
    fn a_compaction_boundary_maps_the_summarizer_span() {
        use crate::model::ChatMessage;
        let mut session = crate::model::Session::new("w", "boundary");
        session.messages = vec![
            ChatMessage::user("a"),
            ChatMessage::assistant(),
            ChatMessage::user("b"),
            ChatMessage::assistant(),
            ChatMessage::user("c"),
        ];
        // The assistant placeholders stay empty, so they do not count: the
        // counting rows are 0, 2 and 4.
        // Removing one keeps the second counting row at index 2.
        assert_eq!(super::AppState::compaction_boundary(&session, 1, 0), 2);
        assert_eq!(super::AppState::compaction_boundary(&session, 2, 0), 4);
        assert_eq!(
            super::AppState::compaction_boundary(&session, 3, 0),
            session.messages.len(),
            "everything folded leaves an empty tail"
        );
        // A replayed checkpoint is a head, not a stored row.
        assert_eq!(super::AppState::compaction_boundary(&session, 2, 1), 2);
        assert_eq!(super::AppState::compaction_boundary(&session, 3, 1), 4);
        // A zero-row removal never moves the boundary, even with a checkpoint.
        assert_eq!(super::AppState::compaction_boundary(&session, 0, 1), 0);
    }

    /// The boundary is a GLOBAL index, so a window that starts part-way through
    /// the log maps the summarizer's count onto the right absolute row.
    #[test]
    fn a_compaction_boundary_is_global_across_a_window() {
        use crate::model::ChatMessage;
        let mut session = crate::model::Session::new("w", "global");
        session.loaded_from = 40;
        session.messages = vec![
            ChatMessage::user("a"),
            ChatMessage::assistant(),
            ChatMessage::user("b"),
            ChatMessage::assistant(),
            ChatMessage::user("c"),
            ChatMessage::assistant(),
        ];
        for (index, text) in [(1, "ra"), (3, "rb"), (5, "rc")] {
            session.messages[index].text = text.to_string();
        }
        session.compacted = 41;
        assert_eq!(super::request_start(&session), 1);
        assert_eq!(super::AppState::compaction_boundary(&session, 3, 0), 44);
        assert_eq!(super::AppState::compaction_boundary(&session, 2, 0), 43);
        // A replayed checkpoint is a head, not a counted store row.
        assert_eq!(super::AppState::compaction_boundary(&session, 3, 1), 43);
        assert_eq!(super::AppState::compaction_boundary(&session, 6, 0), 46);
    }

    /// An automatic compaction installs the same boundary and marker, with the
    /// automatic title, and leaves the goal, plan and plan mode alone.
    #[test]
    fn an_automatic_compaction_records_the_marker_and_keeps_durable_state() {
        use crate::model::{ChatMessage, PlanItem, PlanStatus};
        let (mut state, id) = goal_state("compact-auto");
        assert!(state
            .create_goal(&id, "survive automatic compaction")
            .is_ok());
        state.set_plan_mode(&id, true);
        {
            let session = state.store.session_mut(&id).expect("the session");
            session.messages = vec![
                ChatMessage::user("one"),
                ChatMessage::assistant(),
                ChatMessage::user("two"),
                ChatMessage::assistant(),
                ChatMessage::user("three"),
                ChatMessage::assistant(),
            ];
            for (index, text) in [(1, "r1"), (3, "r2"), (5, "r3")] {
                session.messages[index].text = text.to_string();
            }
            session.plan = vec![PlanItem {
                text: "keep the plan".to_string(),
                status: PlanStatus::InProgress,
            }];
        }
        let goal_before = state.goal(&id).expect("the goal");
        let dropped = state.land_automatic_compaction(&id, 500, 4, "auto body".to_string());
        assert_eq!(dropped, 4, "the span up to the last kept row");
        {
            let session = state.active_session().expect("the session");
            assert_eq!(session.compacted, 4);
            let record = session.compactions.last().expect("the record");
            assert!(!record.manual, "the automatic path is not a command");
            assert_eq!(record.items, 4);
            assert_eq!(record.tokens, 500);
            assert_eq!(record.summary, "auto body");
            assert!(session.compact_command.is_none());
        }
        // The very next request carries the checkpoint, so the model resumes
        // from the summary rather than from a silently truncated history.
        let session = state.active_session().expect("the session");
        let request = state.request_transcript(session);
        assert!(request[0].text.contains("auto body"));
        assert_eq!(request[1].text, "three");
        // Durable state above the transcript is untouched.
        assert_eq!(state.goal(&id), Some(goal_before));
        assert!(state.plan_mode(&id), "plan mode survives");
        assert_eq!(
            state.active_session().expect("the session").plan.len(),
            1,
            "the plan survives"
        );
    }

    /// An automatic checkpoint lands after the last real message but BEFORE
    /// the empty assistant placeholder the live turn is about to fill, so the
    /// reply streams below the marker it follows.
    #[test]
    fn an_automatic_checkpoint_sits_before_the_live_reply() {
        use crate::model::ChatMessage;
        let (mut state, id) = goal_state("compact-auto-slot");
        {
            let session = state.store.session_mut(&id).expect("the session");
            session.messages = vec![ChatMessage::user("prompt"), ChatMessage::assistant()];
        }
        let dropped = state.land_automatic_compaction(&id, 100, 1, "auto".to_string());
        assert_eq!(dropped, 2, "both rows leave the context");
        let session = state.active_session().expect("the session");
        let record = session.compactions.last().expect("the record");
        assert_eq!(
            record.after, 1,
            "the marker sorts after the user row, before the placeholder"
        );
        assert!(
            session.messages[1].text.is_empty(),
            "the live reply is empty"
        );
    }

    /// The automatic trigger prices the request against the workspace ceiling
    /// and keeps the reference's 16% tail.
    #[test]
    fn the_automatic_threshold_prices_the_request_and_keeps_the_reference_tail() {
        use crate::model::ChatMessage;
        let (mut state, id) = goal_state("compact-threshold");
        {
            let session = state.store.session_mut(&id).expect("the session");
            session.messages = vec![ChatMessage::user("x".repeat(400)), ChatMessage::assistant()];
            session.messages[1].text = "y".repeat(400);
        }
        let session = state.active_session().expect("the session");
        let transcript = state.request_transcript(session);
        assert_eq!(
            state.auto_compaction_retain(&id, &transcript, &[], 0),
            None,
            "a zero budget switches automatic compaction off"
        );
        assert_eq!(
            state.auto_compaction_retain(&id, &transcript, &[], 1_000_000),
            None,
            "a request below the ceiling is left alone"
        );
        assert_eq!(
            state.auto_compaction_retain(&id, &transcript, &[], 100),
            Some(16),
            "16% of the ceiling is retained"
        );
        assert!(
            (100.0 * super::AppState::COMPACT_RETAIN_RATIO) as usize == 16,
            "the ratio is the shipped backend's default"
        );
    }

    /// The compact command takes no arguments; any suffix is the usage
    /// refusal, and it is a command even when the slash card is closed.
    #[test]
    fn the_compact_grammar_is_argument_free() {
        use super::{parse_slash_line, SlashLine};
        assert_eq!(
            parse_slash_line("/compact"),
            Some(SlashLine::Compact(String::new()))
        );
        assert_eq!(
            parse_slash_line("  /compact   "),
            Some(SlashLine::Compact(String::new())),
            "trailing whitespace trims to the bare command"
        );
        assert_eq!(
            parse_slash_line("/compact last 5"),
            Some(SlashLine::Compact("last 5".to_string())),
            "the argument reaches the handler, which owns the refusal"
        );
        assert_eq!(parse_slash_line("/compactnow"), None);
    }

    /// The command row's lifecycle, and its persistence, is the reference's
    /// command/done card: running, then the outcome text with its error state.
    #[test]
    fn the_compact_command_row_persists_its_outcome() {
        let (mut state, id) = goal_state("compact-row");
        assert!(state.begin_compact_command(&id));
        {
            let command = state
                .active_session()
                .and_then(|session| session.compact_command.as_ref())
                .expect("the running row");
            assert!(command.outcome.is_none(), "no outcome while it runs");
        }
        state.finish_compact_command(&id, "No compactable history yet.", false);
        {
            let outcome = state
                .active_session()
                .and_then(|session| session.compact_command.as_ref())
                .and_then(|command| command.outcome.as_ref())
                .expect("the outcome");
            assert_eq!(outcome.text, "No compactable history yet.");
            assert!(!outcome.error);
        }
        state.clear_compact_command(&id);
        assert!(
            state
                .active_session()
                .expect("the session")
                .compact_command
                .is_none(),
            "a landed checkpoint takes the command row's place"
        );
    }

    /// The checkpoint surface survives a reload: the boundary, the record and
    /// the command row are read back from the session document, not rebuilt
    /// from this process's memory.
    #[test]
    fn a_compaction_survives_a_reload() {
        use crate::model::{ChatMessage, CompactionReport};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-compact-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id;
        {
            let mut state = super::AppState::with_store(crate::store::Store::open_at(dir.clone()));
            id = state.active_id.clone().expect("a session");
            {
                let session = state.store.session_mut(&id).expect("the session");
                session.messages = vec![
                    ChatMessage::user("one"),
                    ChatMessage::assistant(),
                    ChatMessage::user("two"),
                    ChatMessage::assistant(),
                    ChatMessage::user("three"),
                    ChatMessage::assistant(),
                ];
                for (index, text) in [(1, "r1"), (3, "r2"), (5, "r3")] {
                    session.messages[index].text = text.to_string();
                }
            }
            assert!(state.create_goal(&id, "reload survives").is_ok());
            let report = CompactionReport {
                items: 4,
                tokens: 500,
                summary: "reload body".to_string(),
                manual: true,
            };
            assert_eq!(state.land_manual_compaction(&id, 4, report), 4);
        }
        let reopened = super::AppState::with_store(crate::store::Store::open_at(dir));
        assert_eq!(reopened.active_id.as_deref(), Some(id.as_str()));
        let session = reopened.active_session().expect("the session");
        assert_eq!(session.compacted, 4);
        let record = session.compactions.last().expect("the restored record");
        assert!(record.manual);
        assert_eq!(record.summary, "reload body");
        let request = reopened.request_transcript(session);
        assert!(
            request[0].text.contains("reload body"),
            "the reloaded session replays the checkpoint"
        );
        assert_eq!(request[1].text, "three");
        assert!(reopened.goal(&id).is_some(), "the goal reloaded too");
    }

    /// A restart loads only the LOG'S TAIL, so the compaction boundary must be
    /// global: a long session must reload its checkpoint and the retained tail,
    /// not lose the tail or keep rows that were already folded.
    #[test]
    fn a_paged_session_reloads_its_checkpoint_and_retained_tail() {
        use crate::model::{ChatMessage, CompactionRecord};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-compact-paged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id;
        {
            let mut state = super::AppState::with_store(crate::store::Store::open_at(dir.clone()));
            id = state.active_id.clone().expect("a session");
            {
                let session = state.store.session_mut(&id).expect("the session");
                session.messages = (0..600)
                    .map(|index| {
                        if index % 2 == 0 {
                            ChatMessage::user(format!("m{index}"))
                        } else {
                            let mut reply = ChatMessage::assistant();
                            reply.text = format!("r{index}");
                            reply
                        }
                    })
                    .collect();
                session.compacted = 500;
                session.compactions.push(CompactionRecord {
                    at: 1,
                    items: 500,
                    tokens: 1_000,
                    summary: "paged summary".to_string(),
                    manual: true,
                    after: 600,
                });
            }
            state.save_session(&id);
        }
        let mut reopened = super::AppState::with_store(crate::store::Store::open_at(dir));
        let session = reopened.active_session().expect("the session");
        assert_eq!(
            session.loaded_from,
            600 - crate::store::WINDOW,
            "the restart loads only the tail"
        );
        assert_eq!(session.compacted, 500, "the GLOBAL boundary survived");
        let (transcript, _) = reopened
            .begin_turn_in(&id, "next")
            .expect("a turn opens after the reload");
        assert_eq!(
            transcript.len(),
            1 + 100,
            "the checkpoint plus messages 500..600"
        );
        assert!(transcript[0].text.contains("paged summary"));
        assert_eq!(transcript[1].text, "m500");
        assert_eq!(transcript.last().expect("the tail").text, "r599");
        // The meter prices exactly that request, not the whole 600-row log.
        let session = reopened.active_session().expect("the session");
        let checkpoint =
            ChatMessage::user(agent::agent::context::render_checkpoint("paged summary"))
                .priced_tokens();
        let retained: u64 = session.messages[super::request_start(session)..]
            .iter()
            .map(ChatMessage::priced_tokens)
            .sum();
        assert_eq!(
            reopened.context_breakdown().expect("the meter").messages,
            checkpoint + retained
        );
    }

    /// A refusal row is durable as well: the reference logs command/done, so a
    /// reload still shows why the compact command produced nothing.
    #[test]
    fn a_compact_refusal_survives_a_reload() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-compact-refusal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id;
        {
            let mut state = super::AppState::with_store(crate::store::Store::open_at(dir.clone()));
            id = state.active_id.clone().expect("a session");
            assert!(state.begin_compact_command(&id));
            state.finish_compact_command(&id, "No compactable history yet.", false);
        }
        let reopened = super::AppState::with_store(crate::store::Store::open_at(dir));
        let outcome = reopened
            .active_session()
            .and_then(|session| session.compact_command.as_ref())
            .and_then(|command| command.outcome.as_ref())
            .expect("the reloaded refusal row");
        assert_eq!(outcome.text, "No compactable history yet.");
        assert!(!outcome.error);
    }

    /// A goal's attachments become ONE ordinary user message whose blocks the
    /// model can read on later rounds.
    #[test]
    fn goal_attachments_are_one_model_readable_user_message() {
        use crate::model::Role;
        let (mut state, id) = goal_state("attachments");
        assert!(state.create_goal(&id, "look at the screenshot").is_ok());
        let attachments = vec![image_fixture()];
        assert!(state.append_goal_attachments(&id, &attachments));
        let session = state.active_session().expect("the session");
        let last = session.messages.last().expect("the attachment message");
        assert_eq!(last.role, Role::User);
        assert_eq!(last.text, super::GOAL_ATTACHMENT_NOTE);
        assert_eq!(last.images, vec!["data:image/png;base64,Zg=="]);
        let content = super::message_content(last).expect("multimodal content");
        let react::llm::Content::Parts(parts) = content else {
            panic!("an attachment message must replay as parts");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(
            &parts[0],
            react::llm::ContentPart::Text { text } if text == super::GOAL_ATTACHMENT_NOTE
        ));
        assert!(matches!(
            &parts[1],
            react::llm::ContentPart::Binary { binary }
                if binary.content_type == "image/png"
                    && binary.url() == "data:image/png;base64,Zg=="
        ));
        assert!(
            !state.append_goal_attachments(&id, &[]),
            "no attachments, no message"
        );
    }

    /// The occupancy share rounds like the reference's own `Math.round` and
    /// caps at 100.
    #[test]
    fn the_context_share_rounds_like_the_reference() {
        assert_eq!(super::context_share(454, 1_000), Some(45));
        assert_eq!(super::context_share(455, 1_000), Some(46));
        assert_eq!(super::context_share(999, 1_000), Some(100));
        // A prompt past its budget still reads full, never 101%.
        assert_eq!(super::context_share(50_000, 1_000), Some(100));
        assert_eq!(super::context_share(0, 1_000), None);
        assert_eq!(super::context_share(10, 0), None);
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(super::base64_encode(b""), "");
        assert_eq!(super::base64_encode(b"f"), "Zg==");
        assert_eq!(super::base64_encode(b"fo"), "Zm8=");
        assert_eq!(super::base64_encode(b"foo"), "Zm9v");
        assert_eq!(super::base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(super::base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(super::base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn only_image_extensions_become_attachments() {
        let dir = std::env::temp_dir();
        let note = dir.join("bsh-attach-note.txt");
        std::fs::write(&note, b"x").ok();
        assert!(super::Attachment::from_path(&note).is_none());
        assert!(super::Attachment::from_path(&dir.join("missing.png")).is_none());
    }

    #[test]
    fn a_turn_without_attachments_stays_plain_text() {
        assert_eq!(
            super::turn_input("hello", &[]),
            react::llm::Content::Text("hello".to_string())
        );
        assert_eq!(super::display_text("hello", &[]), "hello");
    }

    #[test]
    fn a_turn_with_an_attachment_becomes_multimodal() {
        let attachment = image_fixture();
        let content = super::turn_input("look", std::slice::from_ref(&attachment));
        let react::llm::Content::Parts(parts) = content else {
            panic!("an attached turn must be multimodal");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], react::llm::ContentPart::Text { text } if text == "look"));
        match &parts[1] {
            react::llm::ContentPart::Binary { binary } => {
                assert_eq!(binary.content_type, "image/png");
                assert_eq!(binary.url(), "data:image/png;base64,Zg==");
            }
            other => panic!("expected an image part, got {other:?}"),
        }
    }

    #[test]
    fn the_transcript_names_an_attachment() {
        let attachment = image_fixture();
        assert_eq!(
            super::display_text("look", std::slice::from_ref(&attachment)),
            "look\n[image: shot.png]"
        );
    }
    use super::{cache_hit_percent, exact_tokens, human_tokens, matches, stamp};

    #[test]
    fn an_empty_query_matches_every_title() {
        assert!(matches("", "anything"));
        assert!(matches("   ", "anything"));
    }

    #[test]
    fn the_match_ignores_case_and_trims_the_query() {
        assert!(matches("  FIX ", "fix the parser"));
        assert!(matches("parser", "Fix the Parser"));
        assert!(!matches("parser", "fix the lexer"));
    }

    /// The status bar names the model the newest usage ran against, and falls
    /// back to the configured model before any usage has landed.
    #[test]
    fn the_status_model_follows_the_usage_then_the_setting() {
        let mut state = super::AppState::with_store(crate::store::Store::open_at(
            "target/bsh-status-home".into(),
        ));
        // The tree owns the model: the workspace records which one it uses, and
        // the status bar reads the leaf the reference resolves to.
        let mut global = state.settings.global.clone();
        global.add_custom_provider(
            "local",
            "Local",
            "https://local.example/v1",
            &[
                crate::settings::global::ModelEntry::named("first-model"),
                crate::settings::global::ModelEntry::named("second-model"),
            ],
        );
        state.settings.global = global;
        state.select_model_from("local", "first-model");
        assert_eq!(
            state.status_model().as_deref(),
            Some("first-model"),
            "with no turn yet the session's chosen model is the honest answer"
        );

        let (id, _, _) = state.begin_turn("hello").expect("a turn");
        state.apply_event(
            &id,
            super::AgentEvent::Usage {
                prompt: 5,
                completion: 7,
                cached: 0,
            },
        );
        assert_eq!(state.status_model().as_deref(), Some("first-model"));

        state.select_model_from("local", "second-model");
        state.apply_event(
            &id,
            super::AgentEvent::Usage {
                prompt: 1,
                completion: 1,
                cached: 0,
            },
        );
        assert_eq!(
            state.status_model().as_deref(),
            Some("second-model"),
            "the model rides with the usage, not with the current setting"
        );
    }

    /// A session with no model anywhere names none, rather than an empty chip.
    #[test]
    fn the_status_model_is_absent_when_nothing_names_one() {
        let state = super::AppState::with_store(crate::store::Store::open_at(
            "target/bsh-status-empty".into(),
        ));
        // Nothing was chosen in this session, so there is no configured model.
        assert_eq!(state.status_model(), None);
    }

    /// A child transcript is saved as an ordinary Session (JSON + Parquet log)
    /// and comes back after a restart, with its menu row and trajectory tools.
    #[test]
    fn subagent_sessions_survive_a_restart() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-subagent-persist");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch");
        let mut state = super::AppState::with_store(crate::store::Store::open_at(base.clone()));
        let parent = state.new_session().expect("a parent session");
        let workspace = state
            .store
            .session(&parent)
            .expect("parent row")
            .workspace
            .clone();
        let id = state
            .subagents
            .begin(&parent, &parent, 1, 0, "Joke child", "tell a joke");
        state.subagents.finish(
            &id,
            "why did the chicken cross the road?".to_string(),
            (12, 34),
            vec![
                crate::subagent::SubagentMessage {
                    role: "user".to_string(),
                    content: "tell a joke".to_string(),
                    tool: None,
                },
                crate::subagent::SubagentMessage {
                    role: "assistant".to_string(),
                    content: "why did the chicken cross the road?".to_string(),
                    tool: None,
                },
                crate::subagent::SubagentMessage {
                    role: "tool".to_string(),
                    content: "read_file {\"path\":\"joke.txt\"}".to_string(),
                    tool: Some("read_file".to_string()),
                },
                crate::subagent::SubagentMessage {
                    role: "tool".to_string(),
                    content: "why did the chicken cross the road?".to_string(),
                    tool: None,
                },
            ],
        );
        state.persist_subagents();
        assert!(
            state
                .store
                .in_workspace(&workspace)
                .iter()
                .all(|session| session.id != id),
            "a child is not a sidebar row"
        );

        let mut reopened = super::AppState::with_store(crate::store::Store::open_at(base));
        let run = reopened
            .subagents
            .snapshot()
            .into_iter()
            .find(|run| run.id == id)
            .expect("the child row is restored");
        assert_eq!(run.title, "Joke child");
        assert_eq!(run.prompt, "tell a joke");
        assert_eq!(run.status, crate::subagent::SubagentStatus::Done);
        assert_eq!(run.total_tokens(), 46);
        assert!(run.transcript.len() >= 2);

        let parent_id = reopened.active_id.clone().expect("the parent is active");
        reopened.select(&id);
        assert_eq!(reopened.active_id.as_deref(), Some(parent_id.as_str()));
        assert_eq!(
            reopened.subagent.as_ref().map(|view| view.id.as_str()),
            Some(id.as_str())
        );
        let session = reopened.active_session().expect("the child preview");
        assert_eq!(session.title, "Joke child");
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].text, "tell a joke");
        assert_eq!(
            session.messages[1].text,
            "why did the chicken cross the road?"
        );
        assert_eq!(session.messages[1].tools.len(), 1);
        assert_eq!(session.messages[1].tools[0].name, "read_file");
        assert_eq!(
            session.messages[1].tools[0].output.as_deref(),
            Some("why did the chicken cross the road?")
        );
    }

    /// A turn stamps both of its rows, notes the first output, and closes its
    /// span on the terminal event - which is what the trajectory reads.
    #[test]
    fn a_turn_records_its_measured_span() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join("bsh-turn-timing");
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = super::AppState::with_store(crate::store::Store::open_at(dir));
        let id = state.active_id.clone().expect("a session");
        state.begin_turn_in(&id, "hello").expect("a turn opens");

        // Back-date the turn so the measured span is a known, positive number
        // rather than whatever the two calls happened to land on.
        let opened = super::now_ms().saturating_sub(400);
        if let Some(session) = state.store.session_mut(&id) {
            for message in session.messages.iter_mut().rev().take(2) {
                message.timing_mut().start = Some(opened);
            }
        }
        state.apply_event(&id, super::AgentEvent::Text("hi".to_string()));
        state.apply_event(&id, super::AgentEvent::Done);

        let session = state.store.session(&id).expect("the session");
        let (user, assistant) = (&session.messages[0], &session.messages[1]);
        assert_eq!(
            user.timing().and_then(|timing| timing.start),
            Some(opened),
            "the submission is stamped on the user row"
        );
        assert_eq!(
            assistant.timing().and_then(|timing| timing.start),
            Some(opened),
            "and on the reply row, so both measure from the turn's opening"
        );
        let first = assistant
            .timing()
            .and_then(|timing| timing.first_output)
            .expect("the first output is noted");
        assert!(first >= opened, "it lands after the turn opened");
        let ms = assistant
            .timing()
            .and_then(|timing| timing.ms)
            .expect("the terminal event closes the span");
        assert!(ms >= 400, "the span covers the back-dated clock: {ms}");
        assert_eq!(
            user.timing().and_then(|timing| timing.ms),
            None,
            "the user row keeps no span of its own; the view derives its wait"
        );
    }

    #[test]
    fn token_counts_switch_units_at_a_thousand() {
        assert_eq!(human_tokens(0), "0");
        assert_eq!(human_tokens(947), "947");
        assert_eq!(human_tokens(12_345), "12.3K");
        assert_eq!(human_tokens(1_000), "1K");
        assert_eq!(human_tokens(517_500), "518K", "a scaled 100+ rounds");
        assert_eq!(human_tokens(69_200_000), "69.2M");
    }

    #[test]
    fn exact_counts_group_in_threes() {
        assert_eq!(exact_tokens(0), "0");
        assert_eq!(exact_tokens(947), "947");
        assert_eq!(exact_tokens(17_734), "17,734");
        assert_eq!(exact_tokens(1_234_567), "1,234,567");
    }

    #[test]
    fn the_cache_hit_never_rounds_a_partial_hit_to_full() {
        assert_eq!(cache_hit_percent(67, 100).as_deref(), Some("67"));
        assert_eq!(
            cache_hit_percent(2, 3).as_deref(),
            Some("67"),
            "ties round up"
        );
        assert_eq!(cache_hit_percent(199, 200).as_deref(), Some("99.5"));
        assert_eq!(cache_hit_percent(999, 1000).as_deref(), Some("99.9"));
        assert_eq!(cache_hit_percent(200, 200).as_deref(), Some("100"));
        assert_eq!(cache_hit_percent(0, 0), None);
        assert_eq!(cache_hit_percent(0, 100).as_deref(), Some("0"));
    }

    #[test]
    fn a_stamp_reads_as_a_utc_date_and_time() {
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(86_399), "1970-01-01 23:59");
        assert_eq!(stamp(86_400), "1970-01-02 00:00");
        assert_eq!(stamp(1_000_000_000), "2001-09-09 01:46");
        assert_eq!(stamp(1_752_000_000), "2025-07-08 18:40");
    }
    #[test]
    fn a_blank_first_prompt_keeps_the_localized_default_title() {
        let zh = crate::i18n::Lang::from_locale("zh");
        assert_eq!(
            crate::model::Session::title_from("", zh),
            crate::i18n::tr(zh, "row.newChat")
        );
        assert_eq!(crate::model::Session::title_from("hello", zh), "hello");
    }

    /// Register the provider the model tests choose from.
    fn seed_models(state: &mut super::AppState) {
        state.settings.global.add_custom_provider(
            "p",
            "P",
            "https://p.example/v1",
            &[
                crate::settings::global::ModelEntry::named("alpha"),
                crate::settings::global::ModelEntry::named("beta"),
                crate::settings::global::ModelEntry::named("gamma"),
            ],
        );
    }

    /// A state over a scratch directory that can be reopened, with the provider
    /// registered. No session carries a model until one chooses.
    fn state_with_models(label: &str) -> super::AppState {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-models-{label}"));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = super::AppState::with_store(crate::store::Store::open_at(dir));
        seed_models(&mut state);
        state
    }

    /// Two chats in ONE workspace choose different models, and the choice is a
    /// session document fact: reopening the store finds both.
    #[test]
    fn two_sessions_in_one_workspace_keep_their_own_models() {
        let mut state = state_with_models("two");
        let first = state.active_id.clone().expect("the first session");
        let second = state.new_session().expect("the second session");
        assert!(
            state.effective_in(&first).model_ref.is_none(),
            "a new session starts with no model"
        );
        assert!(
            state.effective_in(&second).model_ref.is_none(),
            "and so does a second one: the choice is not copied eagerly"
        );

        state.select(&first);
        state.select_model_from("p", "beta");
        state.select(&second);
        state.select_model_from("p", "gamma");

        // Switching sessions switches what the chip and the next turn see.
        let chip = |state: &super::AppState| {
            crate::ui::composer::model_label(&state.effective().model_name, crate::i18n::Lang::En)
        };
        assert_eq!(state.effective().model, "gamma");
        assert_eq!(
            chip(&state),
            "gamma",
            "the chip names the active session's model"
        );
        state.select(&first);
        assert_eq!(state.effective().model, "beta");
        assert_eq!(chip(&state), "beta", "and switching chats moves it");
        state.select(&second);
        assert_eq!(state.effective().model, "gamma");

        // The same two ids read back from disk.
        let dir = state.store.root().to_path_buf();
        let mut reopened = super::AppState::with_store(crate::store::Store::open_at(dir));
        seed_models(&mut reopened);
        assert_eq!(reopened.effective_in(&first).model, "beta");
        assert_eq!(reopened.effective_in(&second).model, "gamma");
    }

    /// Several chats in one workspace can be streaming at once; which one the
    /// window shows is a view choice, and it is the only thing recorded.
    ///
    /// A background job is a detached shell process, not another chat's turn:
    /// the roster is scoped to the session whose turn started it, so a job the
    /// bash tool just started is visible while its own session is on screen and
    /// absent under its sibling.
    #[test]
    fn sessions_run_concurrently_while_the_window_shows_one() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join("bsh-concurrent-sessions");
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = super::AppState::with_store(crate::store::Store::open_at(dir));
        let first = state.active_id.clone().expect("the first session");
        let second = state.new_session().expect("the second session");
        state.begin_turn_in(&first, "one").expect("turn one");
        state.begin_turn_in(&second, "two").expect("turn two");
        assert!(
            state.is_streaming(&first) && state.is_streaming(&second),
            "two turns run at the same time"
        );

        state.select(&first);

        assert!(
            state.is_streaming(&second),
            "showing a chat must not stop the turn running in another one"
        );
        assert!(
            state.background_jobs().is_empty(),
            "a streaming turn is not a shell job: no process was started"
        );
        // Start a real detached shell through the same roster and session id
        // the turn's bash tool uses. The spawn registers the job before the
        // tool returns, so the very next read sees it.
        use agent::tools::Tool as _;
        let tool =
            agent::tools::BashTool::new("bash").with_jobs(state.jobs.clone(), second.clone());
        let started = tool
            .run(&serde_json::json!({
                "command": "sleep 30",
                "run_in_background": true
            }))
            .expect("the background spawn is accepted");
        let job = started["job_id"]
            .as_str()
            .expect("the spawn returns a job id")
            .to_string();
        assert!(
            state.background_jobs().is_empty(),
            "the job belongs to the session that is not on screen"
        );

        state.select(&second);
        let visible = state.background_jobs();
        assert_eq!(
            visible.len(),
            1,
            "the started job is visible for its own session"
        );
        assert_eq!(visible[0].id, job);
        assert_eq!(visible[0].session, second);
        assert!(visible[0].is_running(), "it is listed at spawn");

        state.select(&first);
        assert!(
            state.background_jobs().is_empty(),
            "and it is absent for the other session"
        );
        assert_eq!(
            state.settings.global.general.active_session, first,
            "the preference follows the view, not the stream"
        );
        assert!(
            state
                .settings
                .workspace
                .general
                .legacy_active_session
                .is_empty(),
            "the workspace document holds no session identity"
        );
        // The view preference is persisted, in the machine document only.
        let root = state.settings.save_root.clone().expect("a scratch root");
        let global = std::fs::read_to_string(root.join("settings.toml"))
            .expect("the global settings document");
        assert!(
            global.contains(&format!("active_session = \"{first}\"")),
            "the visible session is remembered across a relaunch: {global}"
        );
        assert!(
            !root.join(".bsh/settings/settings.toml").exists(),
            "selecting a chat does not write a workspace document at all"
        );
        assert!(state.jobs.kill(&job), "the test reaps the child it started");
    }

    /// A chat that never chose has NO model: the chip falls back to its own
    /// "Select model" copy and the send gate refuses it.
    #[test]
    fn a_session_without_a_choice_has_no_model_and_is_refused() {
        let mut state = state_with_models("unchosen");
        let chosen = state.active_id.clone().expect("the chosen session");
        state.select_model_from("p", "beta");
        let unchosen = state.new_session().expect("the unchosen session");
        assert_eq!(state.effective_in(&chosen).model, "beta");
        assert!(
            state.effective_in(&unchosen).model_ref.is_none(),
            "the sibling never inherits its neighbour's model"
        );
        assert!(state.effective_in(&unchosen).model.is_empty());
        assert!(
            state.effective_in(&unchosen).model_name.is_empty(),
            "there is no name for the chip to show"
        );
        assert_eq!(
            crate::ui::composer::model_label(
                &state.effective_in(&unchosen).model_name,
                crate::i18n::Lang::En,
            ),
            crate::i18n::tr(crate::i18n::Lang::En, "composer.selectModel"),
            "so the chip shows the reference's own select-model copy"
        );
        assert_eq!(
            state.settings.refusal_for(&state.session_choice(&unchosen)),
            Some("composer.chooseModel"),
            "the send gate asks for a model before opening a turn"
        );
    }

    /// Choosing a model writes the ACTIVE session and nothing else.
    #[test]
    fn choosing_a_model_touches_only_its_session() {
        let mut state = state_with_models("isolated");
        let first = state.active_id.clone().expect("the first session");
        let second = state.new_session().expect("the second session");
        state.select(&first);
        let workspace_before = state.settings.workspace.clone();
        let sibling_before = state
            .store
            .session(&second)
            .map(|session| (session.provider.clone(), session.model.clone()));

        state.select_model_from("p", "beta");

        assert_eq!(
            state.settings.workspace, workspace_before,
            "the workspace document is untouched"
        );
        let sibling_after = state
            .store
            .session(&second)
            .map(|session| (session.provider.clone(), session.model.clone()));
        assert_eq!(
            sibling_before, sibling_after,
            "the sibling session is untouched"
        );
        let chosen = state.store.session(&first).expect("the chosen session");
        assert_eq!(chosen.provider.as_deref(), Some("p"));
        assert_eq!(chosen.model.as_deref(), Some("beta"));
    }

    /// A workspace file written before the model was session-scoped still names
    /// one. It is handed to the open session ONCE, and never to a later one.
    #[test]
    fn a_legacy_workspace_pair_seeds_the_open_session_once() {
        let mut state = state_with_models("legacy");
        let first = state.active_id.clone().expect("the open session");
        state.settings.workspace.general.legacy_provider = "p".to_string();
        state.settings.workspace.general.legacy_model = "beta".to_string();
        assert!(state.adopt_legacy_model(), "the old pair is adopted");
        assert_eq!(state.effective_in(&first).model, "beta");
        assert!(
            state.settings.workspace.general.legacy_pair().is_none(),
            "the pair is consumed, so it cannot be read twice"
        );

        let second = state.new_session().expect("a later session");
        assert!(
            !state.adopt_legacy_model(),
            "there is nothing left to adopt"
        );
        assert!(
            state.effective_in(&second).model_ref.is_none(),
            "a later session does not pick the retired pair up"
        );

        // A session that already chose is never overwritten.
        state.select(&first);
        state.settings.workspace.general.legacy_provider = "p".to_string();
        state.settings.workspace.general.legacy_model = "gamma".to_string();
        assert!(!state.adopt_legacy_model(), "an existing choice wins");
        assert_eq!(state.effective_in(&first).model, "beta");
    }

    /// Uninstall drops the inventory record in both cases, but deletes a
    /// directory ONLY for a managed copy. A local package is the user's own
    /// directory and is left exactly where it was.
    #[test]
    fn uninstall_deletes_only_a_managed_directory() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-uninstall-{}", uuid::Uuid::new_v4()));
        let managed_root = base.join("managed");
        let managed = managed_root
            .join("plugin-a")
            .join("node_modules")
            .join("plugin-a");
        let local = base.join("local-plugin");
        std::fs::create_dir_all(&managed).expect("the managed fixture");
        std::fs::create_dir_all(&local).expect("the local fixture");

        let mut state =
            super::AppState::with_store(crate::store::Store::open_at(base.join("store")));
        let record = |id: &str, path: &std::path::Path, managed: bool| {
            crate::settings::global::InstalledExtension {
                id: id.to_string(),
                path: Some(path.to_string_lossy().to_string()),
                managed,
                ..Default::default()
            }
        };
        state.settings.global.extensions = vec![
            record("plugin-a", &managed, true),
            record("local-plugin", &local, false),
        ];
        state.settings.workspace.disabled.extensions =
            vec!["plugin-a".to_string(), "local-plugin".to_string()];

        assert!(state.uninstall_extension_in(&managed_root, "local-plugin"));
        assert!(
            local.is_dir(),
            "a local package is never deleted by an uninstall"
        );
        assert!(
            !state
                .draft()
                .workspace
                .disabled
                .extensions
                .contains(&"local-plugin".to_string()),
            "its disabled entry goes with the record"
        );

        assert!(state.uninstall_extension_in(&managed_root, "plugin-a"));
        assert!(
            !managed_root.join("plugin-a").exists(),
            "the managed directory is removed"
        );
        assert!(
            state.draft().global.extensions.is_empty(),
            "both records are gone"
        );
        assert!(
            !state
                .draft()
                .workspace
                .disabled
                .extensions
                .iter()
                .any(|id| id == "plugin-a"),
            "and both disabled entries with them"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The egress allowlist is a workspace file: add and remove round-trip it,
    /// and a non-address is refused before anything is written.
    #[test]
    fn the_egress_allowlist_round_trips_and_rejects_bad_entries() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-state-policy-allowlist");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch");
        let mut state =
            super::AppState::with_store(crate::store::Store::open_at(base.join("store")));
        let id = state
            .settings
            .note_workspace("scratch", &base.to_string_lossy());
        state.settings.global.general.active_workspace = id;
        state.refresh_egress_allowlist();
        assert!(state.egress_allowlist.is_empty());

        state
            .add_egress_allowlist("127.0.0.1")
            .expect("an address is accepted");
        state
            .add_egress_allowlist("10.0.0.0/8")
            .expect("a CIDR range is accepted");
        assert!(
            state.add_egress_allowlist("not-an-address").is_err(),
            "a bad entry is refused"
        );
        state
            .remove_egress_allowlist("127.0.0.1")
            .expect("the entry is removed");

        let text = std::fs::read_to_string(base.join(".bsh").join("egress-allowlist"))
            .expect("the allowlist file");
        assert!(text.contains("10.0.0.0/8"), "{text}");
        assert!(!text.contains("127.0.0.1"), "{text}");
        state.refresh_egress_allowlist();
        assert_eq!(state.egress_allowlist, vec!["10.0.0.0/8".to_string()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Trust is a draft edit, so Cancel leaves it alone and Save persists it.
    #[test]
    fn preset_trust_lands_on_the_draft() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-state-policy-trust");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch");
        let mut state =
            super::AppState::with_store(crate::store::Store::open_at(base.join("store")));
        state.set_preset_trusted("strict", true);
        assert!(state
            .draft()
            .global
            .presets
            .get("strict")
            .is_some_and(|preset| preset.trusted));
        state.set_preset_trusted("strict", false);
        assert!(!state
            .draft()
            .global
            .presets
            .get("strict")
            .is_some_and(|preset| preset.trusted));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A branch cuts the transcript at the chosen message and opens as a
    /// sibling with an incremented title.
    #[test]
    fn a_branch_cuts_the_transcript_at_its_message() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-state-branch-at");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch");
        let mut state =
            super::AppState::with_store(crate::store::Store::open_at(base.join("store")));
        let id = state.active_id.clone().expect("a session");
        for turn in 0..3 {
            state
                .begin_turn_in(&id, &format!("turn {turn}"))
                .expect("a turn opens");
            state.apply_event(&id, super::AgentEvent::Text(format!("reply {turn}")));
            state.apply_event(&id, super::AgentEvent::Done);
        }
        let (title, len) = {
            let session = state.store.session(&id).expect("the session");
            (session.title.clone(), session.messages.len())
        };
        assert_eq!(len, 6, "three exchanges");
        let last = len - 1;
        assert!(
            state.can_branch_at(&id, last),
            "the newest settled reply can be branched"
        );
        assert!(
            !state.can_branch_at(&id, 1),
            "an earlier reply has later chat nodes, so it cannot"
        );

        let forked = state.branch_session_at(&id, last).expect("a branch opens");
        assert_ne!(forked, id, "the branch is its own session");
        let branched = state.store.session(&forked).expect("the branch");
        assert_eq!(
            branched.messages.len(),
            len,
            "branching at the last message keeps the whole transcript"
        );
        assert_eq!(
            branched.title,
            format!("{title} (2)"),
            "the title counts up"
        );
        // The original is untouched: a branch is a copy, never a move.
        assert_eq!(
            state
                .store
                .session(&id)
                .expect("the original")
                .messages
                .len(),
            len
        );
        // A branch of the branch keeps counting from the same base.
        let third = state
            .branch_session_at(&forked, last)
            .expect("a second branch");
        assert_eq!(
            state.store.session(&third).expect("the third").title,
            format!("{title} (3)")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A branch point that is not the last message of a settled turn is refused,
    /// and a turn still streaming has no point at all.
    #[test]
    fn a_branch_is_offered_only_on_the_last_message_of_a_settled_turn() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-state-branch-rule");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch");
        let mut state =
            super::AppState::with_store(crate::store::Store::open_at(base.join("store")));
        let id = state.active_id.clone().expect("a session");
        for turn in 0..2 {
            state
                .begin_turn_in(&id, &format!("turn {turn}"))
                .expect("a turn opens");
            state.apply_event(&id, super::AgentEvent::Text("hi".to_string()));
            state.apply_event(&id, super::AgentEvent::Done);
        }
        assert_eq!(
            state
                .store
                .session(&id)
                .expect("the session")
                .messages
                .len(),
            4
        );
        assert!(state.can_branch_at(&id, 3), "the newest settled reply");
        assert!(
            !state.can_branch_at(&id, 1),
            "an earlier reply has a successor"
        );
        assert!(
            !state.can_branch_at(&id, 0),
            "and a user row is not a branch point"
        );
        assert!(
            state.branch_session_at(&id, 1).is_none(),
            "the refusal is enforced by the same call, not only by the control"
        );
        // While a turn streams there is no settled reply to branch from.
        state
            .begin_turn_in(&id, "still going")
            .expect("a turn opens");
        let last = state
            .store
            .session(&id)
            .expect("the session")
            .messages
            .len()
            - 1;
        assert!(
            !state.can_branch_at(&id, last),
            "a streaming turn is not a branch point"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The footer clock reads local wall time, and dates a message that is not
    /// from today.
    #[test]
    fn the_message_clock_reads_local_time_and_dates_other_days() {
        use super::{clock_at, local_parts, now};
        let lang = crate::i18n::Lang::En;
        let today = clock_at(now(), lang);
        assert_eq!(today.len(), 5, "today is the bare HH:MM clock: {today}");
        assert!(today.contains(':'), "and it is a clock: {today}");

        // Yesterday is dated when its local day differs from today's; on the
        // rare run whose 24 hours land on the same local day the assertion has
        // nothing to prove, so it only checks the dated branch when it applies.
        let earlier = now().saturating_sub(86_400);
        let (ty, tm, td, ..) = local_parts(now());
        let (ey, em, ed, ..) = local_parts(earlier);
        let dated = clock_at(earlier, lang);
        if (ty, tm, td) != (ey, em, ed) {
            assert!(
                dated.contains('/') || dated.contains('-'),
                "another day carries its date: {dated}"
            );
            assert!(dated.contains(':'), "and still a clock: {dated}");
            assert!(
                dated.contains(&em.to_string()) && dated.contains(&ed.to_string()),
                "the date names the local day: {dated}"
            );
        }
    }
}
