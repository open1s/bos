//! The Trajectory tab.
//!
//! The reference builds this tab from an event log, not from the transcript it
//! shares with Chat: a filter toolbar (Duration / Turns / Calls plus a search
//! field), a three-lane timeline strip, a dense list of typed events, and a
//! detail panel for the selected one. bsh has the same raw material - the
//! session's messages and their tool calls - so the view is built from that and
//! nothing is invented: the lanes are drawn against one shared time axis built
//! from each record's own start (`ToolEvent.at`, or the message timing), so a
//! block's x is when its event started and its width is how long it took. The
//! SUBTOOL / CONTEXT chips the reference also draws have no producer in this
//! model (a nested call and an injected time-context message are both their
//! harness's, not ours).

use gpui_kit::*;

use super::shell::Shell;
use super::todo_panel::spinner;
use gpui_kit::component::input::Input;

use super::lane::{block_box, equal_lane, fixed_box, time_lane, LaneRecord, TimeAxis};
use super::{icon, txt, DivExt, IconName};
use crate::i18n::Lang;
use crate::model::{ChatMessage, MessageTiming, Session};
use crate::theme;

/// What an event's row says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    User,
    Assistant,
    Tool,
    /// Context the harness puts in front of a turn: the goal the agent is
    /// working to, which bsh stores per session.
    Context,
    /// A turn that failed. The reference marks a failed row and paints its rail
    /// red; bsh records the failure on the message, so it is a real line.
    Error,
    /// A call that started while another call was still running. The reference
    /// nests those under their parent as SUBTOOL. The nesting is derived from
    /// the recorded times rather than stored, so no schema carries a parent.
    Subtool,
    /// A compaction: the turn an older span was condensed into a checkpoint.
    Compaction,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::User => "traj.kindUser",
            Kind::Assistant => "traj.kindAssistant",
            Kind::Tool => "traj.kindTool",
            Kind::Context => "traj.kindContext",
            Kind::Error => "traj.kindError",
            Kind::Subtool => "traj.kindSubtool",
            Kind::Compaction => "traj.kindCompaction",
        }
    }

    fn ink(self) -> u32 {
        match self {
            Kind::User => theme::muted(),
            Kind::Assistant => theme::accent(),
            Kind::Tool => theme::warn(),
            Kind::Context => theme::success(),
            Kind::Error => theme::error(),
            Kind::Subtool => theme::warn(),
            Kind::Compaction => theme::muted(),
        }
    }
}

/// One line of the log.
pub(crate) struct Event {
    pub(crate) kind: Kind,
    /// The turn the event belongs to, 1-based, as the reference numbers it.
    pub(crate) turn: usize,
    /// The primary line: a tool's heading, or the text's first line.
    pub(crate) title: String,
    /// The outcome line: what the call returned.
    pub(crate) outcome: String,
    /// What a tool was given, and what it gave back, for the Raw tab.
    pub(crate) raw: String,
    /// Where the line came from: a tool row names the tool.
    pub(crate) source: String,
    /// How long the record took, when a clock measured it.
    ///
    /// A tool carries its own milliseconds; a message carries the turn it
    /// opened (the user row its wait for the first output, the assistant row
    /// its settled span). `None` is a record written before timing existed,
    /// which the view reports as unavailable rather than as a zero.
    pub(crate) ms: Option<u64>,
    /// True while the row is the newest one and its turn still streams.
    pub(crate) live: bool,
    /// The message this line came from, and - for a tool line - its index among
    /// that message's tools. `usize::MAX` for anything that is not a tool line.
    pub(crate) message: usize,
    pub(crate) tool: usize,
    /// When the event started, epoch milliseconds, when the record carries it.
    pub(crate) at: Option<u64>,
    /// The path, host or command the policy was asked about.
    ///
    /// A tool row's `title` is a clipped DISPLAY string (`# read_file {"path":…}`),
    /// so the policy tab cannot read the target back out of it. This field
    /// carries the call's own argument, undipped, which is what the audit ring
    /// records and therefore what a verdict can be matched against.
    pub(crate) target: String,
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// A duration as the detail panel shows it.
///
/// The reference's own rule: whole milliseconds below a second, seconds with a
/// varying precision above it. `None` is a record that carries no clock, and
/// the em dash says so rather than claiming a zero-length turn.
pub(crate) fn duration_label(ms: Option<u64>) -> String {
    match ms {
        None => "\u{2014}".to_string(),
        Some(ms) if ms < 1_000 => format!("{ms} ms"),
        Some(ms) => {
            let seconds = ms as f64 / 1_000.0;
            if ms < 10_000 {
                format!("{seconds:.2} s")
            } else {
                format!("{seconds:.1} s")
            }
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{}...", cut.trim_end())
}

/// The width an equal-width block draws at, in CSS pixels.
///
/// The reference's `[data-equal-duration=true]` is a fixed `width: 8px`,
/// and the toggle's whole point is that every block is the same size
/// regardless of how long the work took.
pub(crate) const EQUAL_BLOCK_W: f32 = 8.0;

/// The narrowest a time-proportional block may be drawn, in CSS pixels.
///
/// The time axis carries the block's real width, but an event can be shorter
/// than the lane can resolve (and an untimed one has no duration at all). The
/// floor keeps every block a hit target without moving its left edge, so two
/// events that started together still line up.
pub(crate) const MIN_BLOCK_W: f32 = 2.0;

/// The shortest a block may be drawn, in milliseconds.
///
/// A block is also a control, so it needs a hit area even when the call it
/// stands for returned instantly.
pub(crate) const MINIMUM_BLOCK_MS: u64 = 60;

/// How wide one timeline block is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BlockWidth {
    /// A fixed pixel width, which is what equal-width mode draws.
    Fixed(f32),
    /// A flex share of the track, used by the equal-width timed reading.
    Share(f32),
    /// A time-proportional block: `x` and `width` are fractions of the lane.
    ///
    /// Relative geometry rather than flex weights is what makes the position
    /// float-accurate: a flex weight competes with the 1px gaps, while a
    /// fraction is the event's own instant on the shared axis.
    Placed { x: f32, width: f32 },
    /// A block placed by time but drawn at a FIXED pixel width.
    ///
    /// This is the reference's equal-width reading exactly: `left` is still the
    /// event's instant on the axis (`--trajectory-span-left`), and only the CSS
    /// `width` is overridden to 8px by `[data-equal-duration=true]`. Separating
    /// the two is what keeps the idle gaps visible while every block is the
    /// same size - packing them instead would draw a solid bar.
    PlacedFixed { x: f32, width: f32 },
}
/// The timeline projection a reader has chosen.
///
/// The reference derives this from two independent flags
/// (`timelineMode = actualDuration ? (actualTime ? "actual" : "duration")
/// : (actualTime ? "time" : "sequence")`), so the four readings below are its
/// own vocabulary rather than a rotation of one control.
///
/// This product's `Duration` mode is the reference's name with the user's
/// meaning: the toolbar's Duration control selects a REAL-TIME strip, where a
/// block starts at its event's recorded start and is as wide as the work took.
/// `Actual` reads the same way, because the toolbar sets both switches
/// together; the distinction is kept only so the stored pair keeps parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Projection {
    /// No recorded clock: one unit slot per record, end to end. Order without
    /// time - the reference's `sequence`.
    Sequence,
    /// Real timestamps, every block a fixed equal width. Gaps are real.
    Time,
    /// Real timestamps and real widths, with the idle stretches left empty:
    /// the gantt this strip draws in Duration mode.
    Duration,
    /// Real timestamps and real widths: the same gantt as `Duration`, named by
    /// the reference's second switch.
    Actual,
}

impl Projection {
    /// The projection a pair of switches selects, exactly as the reference
    /// composes its `timelineMode`.
    ///
    /// bsh exposes ONE control, so the pair a reader can reach is `(false, true)`
    /// and `(true, true)`: equal width on a real clock, then the gantt. The two
    /// clock-less readings stay reachable from a stored value and are what the
    /// projection tests pin.
    pub(crate) fn of(actual_duration: bool, actual_time: bool) -> Self {
        match (actual_duration, actual_time) {
            (true, true) => Self::Actual,
            (true, false) => Self::Duration,
            (false, true) => Self::Time,
            (false, false) => Self::Sequence,
        }
    }

    /// Whether a block is drawn at a fixed equal width rather than by duration.
    ///
    /// Only the two clock-less readings collapse the width; `Duration` and
    /// `Actual` keep a real `end - start`.
    pub(crate) fn equal_width(self) -> bool {
        matches!(self, Self::Sequence | Self::Time)
    }

    /// Whether the axis is a real clock rather than record order.
    pub(crate) fn is_timed(self) -> bool {
        !matches!(self, Self::Sequence)
    }
}

/// One block of a projected timeline, in the projection's own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectedSpan {
    /// The event this block stands for, as an index into the log.
    pub(crate) index: usize,
    /// Where the block starts, in projection units.
    pub(crate) start: f32,
    /// How wide the block is, in projection units.
    pub(crate) width: f32,
    /// The turn the record belongs to, for the boundary marks.
    pub(crate) turn: usize,
}

/// Project one lane's events into the reference's own projection units.
///
/// This is the reference's `deriveTrajectoryTimeline`: `sequence` lays one unit
/// slot per record end to end, and every timed projection places a block at its
/// recorded start, sized by its duration unless the projection is equal-width.
/// No projection removes idle time: a pause is visible on the axis, which is
/// the whole point of reading a run against a clock.
///
/// Duration / Actual draw through the shared pixel geometry in `ui::lane`
/// instead; this unit-space reading remains the model of the equal-width modes
/// and the projection tests.
///
/// Returns `None` when no event in the lane can be placed, which is what tells
/// the caller to draw the lane empty rather than invent an axis.
pub(crate) fn project_lane(
    lane: &[(usize, usize, Option<u64>, u64)],
    projection: Projection,
    minimum_ms: u64,
) -> Option<(f32, Vec<ProjectedSpan>)> {
    if lane.is_empty() {
        return None;
    }
    if !projection.is_timed() {
        // One slot per record, back to back; there is no clock to read.
        let spans: Vec<ProjectedSpan> = lane
            .iter()
            .enumerate()
            .map(|(slot, (index, turn, _, _))| ProjectedSpan {
                index: *index,
                start: slot as f32,
                width: 1.0,
                turn: *turn,
            })
            .collect();
        return Some((lane.len() as f32, spans));
    }
    // The timed projections need a recorded start on every placed record.
    let timed: Vec<(usize, usize, u64, u64)> = lane
        .iter()
        .filter_map(|(index, turn, at, ms)| at.map(|at| (*index, *turn, at, (*ms).max(minimum_ms))))
        .collect();
    if timed.is_empty() {
        return None;
    }
    let mut sorted = timed;
    sorted.sort_by_key(|(_, _, at, _)| *at);
    let first = sorted.first().map(|(_, _, at, _)| *at).unwrap_or(0);
    let mut ends: Vec<u64> = Vec::with_capacity(sorted.len());
    let mut spans: Vec<ProjectedSpan> = Vec::with_capacity(sorted.len());
    for (index, turn, at, ms) in &sorted {
        let start = at.saturating_sub(first);
        spans.push(ProjectedSpan {
            index: *index,
            start: start as f32,
            width: if projection.equal_width() {
                0.0
            } else {
                *ms as f32
            },
            turn: *turn,
        });
        ends.push(start + ms);
    }
    let domain = ends.iter().copied().max().unwrap_or(1).max(1) as f32;
    Some((domain, spans))
}
/// The policy verdict behind one tool call.
///
/// bsh-only: the reference has no equivalent tab, because in DSH a policy
/// question and the tool that asked it are separate records. Here every call
/// is made THROUGH the policy seam, so a call has a decision attached even
/// when it succeeded - which is the thing a reader actually wants to see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PolicyVerdict {
    /// The effective permission in force for this session's tier.
    pub(crate) permission: String,
    /// The agent preset in effect, as the policy recorded it.
    ///
    /// A refusal carries the preset recorded at its decision; a call with no
    /// refusal takes the fallback the caller passes (the policy the turn was
    /// built for), because a `ToolEvent` itself stores no preset.
    pub(crate) preset: String,
    /// The action the policy was asked about, or an empty string when the call
    /// never reached the seam (a purely local read is still a `read` here).
    pub(crate) action: String,
    /// The path, host or command the action was about.
    pub(crate) target: String,
    /// Whether the policy allowed it.
    pub(crate) allowed: bool,
    /// The rule that refused it, when it was refused and the policy named one.
    pub(crate) rule: Option<String>,
    /// The policy's reason, when it refused and gave one.
    pub(crate) reason: Option<String>,
}

/// What action a tool name asks the policy about.
///
/// This is the same classification the audit page uses to bucket a call, and
/// it is deliberately a NAME rule rather than a per-tool table: a tool bsh has
/// never heard of still gets a sensible action instead of a blank row.
pub(crate) fn action_of(tool: &str) -> &'static str {
    let name = tool.to_ascii_lowercase();
    if name.contains("write") || name.contains("edit") || name.contains("patch") {
        "write"
    } else if name.contains("bash") || name.contains("shell") || name.contains("exec") {
        "spawn"
    } else if name.contains("fetch") || name.contains("http") || name.contains("curl") {
        "egress"
    } else {
        "read"
    }
}

/// The verdict for one call, given the ring of recorded refusals.
///
/// A call with no matching refusal was ALLOWED - the ring only stores denials,
/// because a turn makes thousands of allows and none of them is individually
/// interesting until one is missing. Matching is by tool AND target, so two
/// calls to the same tool against different paths are not confused.
pub(crate) fn verdict_for(
    tool: &str,
    target: &str,
    permission: &str,
    preset: &str,
    refusals: &[PolicyVerdict],
) -> PolicyVerdict {
    // A refusal is recorded against the path or host verbatim, while the call
    // carries its own arguments; a prefix match either way is what makes the
    // join work without requiring both sides to be byte-identical.
    let refusal = refusals.iter().find(|refusal| {
        refusal.action == action_of(tool)
            && (refusal.target == target
                || target.starts_with(&refusal.target)
                || refusal.target.starts_with(target))
    });
    match refusal {
        Some(refusal) => PolicyVerdict {
            permission: refusal.permission.clone(),
            preset: refusal.preset.clone(),
            action: refusal.action.clone(),
            target: refusal.target.clone(),
            allowed: false,
            rule: refusal.rule.clone(),
            reason: refusal.reason.clone(),
        },
        None => PolicyVerdict {
            permission: permission.to_string(),
            preset: preset.to_string(),
            action: action_of(tool).to_string(),
            target: target.to_string(),
            allowed: true,
            rule: None,
            reason: None,
        },
    }
}
/// Fold each collapsed turn to its first content row plus a summary row.
///
/// The reference's `collapseTurnRecords`: within a collapsed turn the
/// FIRST content record survives, every other content record is dropped,
/// and one synthetic row follows carrying `…` and a summary of what was
/// hidden. A request-only or system record always survives, because it is
/// boundary scaffolding rather than content, and a turn with nothing but
/// its first row is left alone - there would be nothing to summarize.
///
/// Returns the surviving (index, opens_turn, collapsed_summary) rows in log
/// order, where `collapsed_summary` is `Some` on the synthetic row.
pub(crate) fn collapse_turns(
    visible: &[(usize, &Event)],
    opens: &[bool],
    lang: Lang,
) -> Vec<(usize, bool, Option<String>)> {
    // Group first, so a turn's size is known before any of it is dropped.
    let mut per_turn: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for ((index, event), _) in visible.iter().zip(opens) {
        per_turn.entry(event.turn).or_default().push(*index);
    }
    let mut out = Vec::with_capacity(visible.len());
    for ((index, event), opens_turn) in visible.iter().zip(opens) {
        let turn_rows = per_turn.get(&event.turn).map(Vec::as_slice).unwrap_or(&[]);
        // A turn that holds only its own opening row has nothing to fold.
        if turn_rows.len() <= 1 {
            out.push((*index, *opens_turn, None));
            continue;
        }
        if *opens_turn {
            out.push((*index, true, None));
            // The summary counts what the fold hides, which is every row
            // after this one, and names the turn so the row is readable.
            let hidden = turn_rows.len().saturating_sub(1);
            out.push((
                *index,
                false,
                Some(crate::i18n::tr_args(
                    lang,
                    "traj.turnCollapsed",
                    &[("count", hidden.to_string())],
                )),
            ));
        }
        // Every other row of the turn is dropped: the first content row and
        // the summary are the whole collapsed turn.
    }
    out
}
/// The event log for a session, oldest first.
pub(crate) fn events(shell: &Shell) -> Vec<Event> {
    let Some(session) = shell.state.active_session() else {
        return Vec::new();
    };
    events_for(
        session,
        shell.state.is_streaming(&session.id),
        shell.state.lang,
    )
}

/// One message's recorded turn timing, when the record carries it.
fn timing_of(message: &ChatMessage) -> Option<&MessageTiming> {
    message.timing()
}

/// The span a user row owns: from its submission to the reply's first output.
///
/// The reply is the next message in the log. A turn that settled without ever
/// producing output (a refusal or an immediate stop) reports its whole span; a
/// record written before timing existed reports none.
fn user_wait(session: &Session, index: usize, message: &ChatMessage) -> Option<u64> {
    let start = timing_of(message)?.start?;
    // Only the assistant turn that answers this submission is its reply.
    let reply = session
        .messages
        .get(index + 1)
        .filter(|reply| reply.role == crate::model::Role::Assistant)
        .and_then(timing_of)?;
    match reply.first_output {
        Some(first) => Some(first.saturating_sub(start)),
        None => reply.ms,
    }
}

/// The event log for one session, oldest first.
///
/// `streaming` says whether this session's turn is in flight: a live row never
/// reports an invented elapsed time, which is the reference's own rule.
pub(crate) fn events_for(session: &Session, streaming: bool, lang: Lang) -> Vec<Event> {
    let mut out = Vec::new();
    let mut turn = 0;
    let mut carry: Option<u64> = None;
    let last = session.messages.len().saturating_sub(1);
    // The goal is context the agent is handed every turn; the reference draws
    // its injected context as CONTEXT lines, and this is the one such item bsh
    // actually keeps.
    if let Some(goal) = &session.goal {
        out.push(Event {
            kind: Kind::Context,
            turn: 1,
            title: clip(&goal.objective, 120),
            outcome: String::new(),
            raw: goal.objective.clone(),
            source: "Goal".to_string(),
            ms: None,
            live: false,
            message: usize::MAX,
            tool: usize::MAX,
            at: session
                .messages
                .iter()
                .find_map(|m| m.tools.iter().find_map(|t| t.at)),
            target: String::new(),
        });
    }
    // A compaction happens between turns, so its row sits where the boundary it
    // installed is: above the checkpoint the model was given in place of the
    // folded history. The marker names how much moved and what replaced it.
    let mut markers = session.compactions.iter().peekable();
    for (index, message) in session.messages.iter().enumerate() {
        if message.role == crate::model::Role::User {
            turn += 1;
        }
        let turn_now = turn.max(1);
        // `compacted` is a GLOBAL transcript index; the trajectory walks the
        // loaded window, so compare through `loaded_from`.
        if session.loaded_from + index == session.compacted {
            if let Some(record) = markers.next() {
                out.push(Event {
                    kind: Kind::Compaction,
                    turn: turn_now,
                    title: crate::i18n::tr_args(
                        lang,
                        "msg.compaction.completed",
                        &[
                            ("items", record.items.to_string()),
                            ("tokens", record.tokens.to_string()),
                        ],
                    ),
                    outcome: String::new(),
                    raw: record.summary.clone(),
                    source: "Compaction".to_string(),
                    ms: None,
                    live: false,
                    message: usize::MAX,
                    tool: usize::MAX,
                    at: None,
                    target: String::new(),
                });
            }
        }
        let live = streaming && index == last;
        // The message's own clock is the turn's recorded start. A record with
        // no timing falls back to its first call, then to the call before it,
        // so an old log still places its rows.
        let timing = timing_of(message);
        let anchor = timing
            .and_then(|timing| timing.start)
            .or_else(|| message.tools.iter().find_map(|tool| tool.at))
            .or(carry);
        let message_ms = timing.and_then(|timing| timing.ms);
        if message.role == crate::model::Role::User {
            out.push(Event {
                kind: Kind::User,
                turn: turn_now,
                title: clip(&message.text, 120),
                outcome: String::new(),
                raw: message.text.clone(),
                source: "Message".to_string(),
                ms: user_wait(session, index, message),
                live,
                message: index,
                tool: usize::MAX,
                at: anchor,
                target: String::new(),
            });
            continue;
        }
        if let Some(error) = &message.error {
            out.push(Event {
                kind: Kind::Error,
                turn: turn_now,
                title: clip(error, 120),
                outcome: String::new(),
                raw: error.clone(),
                source: "Turn".to_string(),
                ms: message_ms,
                live,
                message: index,
                tool: usize::MAX,
                at: anchor,
                target: String::new(),
            });
        }
        if !message.text.trim().is_empty() {
            out.push(Event {
                kind: Kind::Assistant,
                turn: turn_now,
                title: clip(&message.text, 120),
                outcome: String::new(),
                raw: message.text.clone(),
                source: "Message".to_string(),
                ms: message_ms,
                live,
                message: index,
                tool: usize::MAX,
                at: anchor,
                target: String::new(),
            });
        } else if !message.tools.is_empty() {
            out.push(Event {
                kind: Kind::Assistant,
                turn: turn_now,
                title: lang.tr("traj.toolCallOnly"),
                outcome: String::new(),
                raw: String::new(),
                source: "Message".to_string(),
                ms: message_ms,
                live,
                message: index,
                tool: usize::MAX,
                at: anchor,
                target: String::new(),
            });
        }
        for (tool_index, tool) in message.tools.iter().enumerate() {
            let args = clip(&tool.args, 48);
            // Nested when its start falls inside a call already in flight. `at`
            // and `ms` are the two facts every record carries, so the depth is
            // derived on read and old logs need no migration.
            let nested = tool.at.is_some_and(|at| {
                out.iter().any(|event| {
                    event.kind == Kind::Tool
                        && event.at.is_some_and(|start| {
                            start <= at && at < start + event.ms.unwrap_or(1).max(1)
                        })
                })
            });
            let outcome = match tool.output.as_deref().map(str::trim) {
                Some(output) if !output.is_empty() => clip(first_line(output).as_str(), 72),
                _ => lang.tr("traj.noOutput"),
            };
            out.push(Event {
                kind: if nested { Kind::Subtool } else { Kind::Tool },
                turn: turn_now,
                title: clip(&format!("# {} {}", tool.name, args), 120),
                outcome,
                raw: format!(
                    "{}\n\n{}",
                    tool.args,
                    tool.output.clone().unwrap_or_default()
                ),
                source: tool.name.clone(),
                ms: tool.ms,
                live,
                message: index,
                tool: tool_index,
                at: tool.at,
                target: tool.args.clone(),
            });
            if tool.at.is_some() {
                carry = tool.at;
            }
        }
    }
    out
}

/// Where a chat row's tool call sits in the log, so its Inspect link can open
/// the matching line.
pub(crate) fn tool_event(shell: &Shell, message: usize, tool: usize) -> Option<usize> {
    events(shell)
        .iter()
        .position(|event| event.message == message && event.tool == tool)
}

/// Does an event answer the search field?
pub(crate) fn matches(event: &Event, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let needle = query.to_lowercase();
    event.title.to_lowercase().contains(&needle)
        || event.outcome.to_lowercase().contains(&needle)
        || event.source.to_lowercase().contains(&needle)
}

impl Shell {
    /// Open a chat row's file in the right panel's editor.
    ///
    /// The reference turns a file step's path into a link; pressing it opens the
    /// file in the panel rather than handing the reader a path to hunt down in
    /// the tree. The editor tab, its highlighting and its tab strip already
    /// exist - this only routes a click into them.
    pub(crate) fn open_in_editor(
        &mut self,
        message: usize,
        tool: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .state
            .active_session()
            .and_then(|session| session.messages.get(message))
            .and_then(|message| message.tools.get(tool))
            .and_then(|tool| crate::ui::transcript::tool_path(&tool.args))
        else {
            return;
        };
        // A tool is usually handed an ABSOLUTE path, and the panel's resolver
        // confines every path to the workspace root and refuses an absolute one
        // outright - so a file that IS inside the workspace reported "that path
        // leaves the workspace". Turn it into the relative path the resolver
        // wants; a path genuinely outside the root keeps its refusal, which is
        // the confinement policy rather than a bug.
        let mut rel = std::path::PathBuf::from(path.trim_matches(['"', '\'', '`']));
        if rel.is_absolute() {
            if let Some(root) = self.state.workspace_root() {
                match rel.strip_prefix(&root) {
                    Ok(inside) => rel = inside.to_path_buf(),
                    Err(_) => {
                        self.right.open = true;
                        self.right.tab = Some(crate::ui::right_panel::RightTab::Files);
                        self.files.error = Some(self.state.lang.tr("right.files.outside"));
                        cx.notify();
                        return;
                    }
                }
            }
        }
        self.right.open = true;
        self.right.tab = Some(crate::ui::right_panel::RightTab::Files);
        self.open_file(&rel, window, cx);
        cx.notify();
    }

    /// Pull the next window of older messages into the log, keeping the reader
    /// where they were: the trajectory's own earlierHistory control.
    ///
    /// The read is scheduled on the background executor, like the transcript's
    /// scroll path; the landing scrolls the trace back to its first row.
    pub(crate) fn load_older_in_trajectory(&mut self, cx: &mut Context<Self>) {
        self.request_older_page(cx);
    }

    /// Open a chat row's tool call in the Trajectory tab, on its own line.
    pub(crate) fn inspect_in_trajectory(&mut self, message: usize, tool: usize) {
        self.state.tab = crate::state::Tab::Trajectory;
        self.traj_select = tool_event(self, message, tool);
        self.traj_tab = 0;
        if let Some(index) = self.traj_select {
            self.traj_scroll.scroll_to_top_of_item(index);
        }
    }

    /// The chip the log puts in front of every line.
    fn traj_chip(&self, event: &Event) -> Div {
        // Measured (.kindSlot): 76px, right-aligned - 44px in zh, where the
        // words are two characters rather than nine.
        let slot = if self.state.lang == crate::i18n::Lang::Zh {
            44.0
        } else {
            76.0
        };
        div()
            .w(px(slot))
            .flex_none()
            .flex()
            .justify_end()
            .child(self.traj_tag(event))
    }

    /// The inline pill: measured .kindTag, 19px tall at radius-xs, 0 5px
    /// padding, 10px at weight 650.
    ///
    /// The log's kind tag and the policy tab's preset value are the same
    /// affordance, so they share one definition rather than drifting apart.
    fn traj_badge(&self, label: String, ink: u32) -> Div {
        div()
            .h(px(19.0))
            .px(px(5.0))
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .rounded(px(4.0))
            .bg(rgb(theme::hover()))
            .child(
                txt(label, 10.0, ink)
                    .font_weight(FontWeight::BOLD)
                    .line_height(px(16.0)),
            )
    }

    /// The kind tag a log row carries.
    fn traj_tag(&self, event: &Event) -> Div {
        self.traj_badge(self.state.lang.tr(event.kind.key()), event.kind.ink())
    }

    /// The filter toolbar: the duration toggle, the collapse actions and the
    /// search field.
    ///
    /// Measured (TrajectoryToolbar + its CSS module): a 32px bar over a .5px
    /// border-l2, inner padding 0 6px at gap 8. `Duration` is one `aria-pressed`
    /// TOGGLE - 20px, 12px type, padding 0 7px, gap 4 - whose tooltip names the
    /// OTHER state: `Use actual duration` while the strip is equal-width,
    /// `Use equal-width operations` while it is real-time. `Turns` and
    /// `Calls` are NOT measures: each collapses or expands every turn or call
    /// in the table, and its glyph flips between the expand and collapse box.
    /// They are left exactly as measured; the strip's own measure is the
    /// Duration toggle.
    pub(crate) fn traj_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let actual = self.traj_actual_duration;
        let duration_tip = if actual {
            lang.tr("traj.useEqualWidth")
        } else {
            lang.tr("traj.useActualDuration")
        };
        let turns_tip = if self.traj_turns_collapsed {
            lang.tr("traj.expandTurns")
        } else {
            lang.tr("traj.collapseTurns")
        };
        let calls_tip = if self.traj_calls_collapsed {
            lang.tr("traj.expandCalls")
        } else {
            lang.tr("traj.collapseCalls")
        };
        let turns_glyph = if self.traj_turns_collapsed {
            "\u{229e}"
        } else {
            "\u{229f}"
        };
        let calls_glyph = if self.traj_calls_collapsed {
            "\u{229e}"
        } else {
            "\u{229f}"
        };
        let button = |id: &'static str, label: String, on: bool, glyph: &'static str| {
            let tip = label.clone();
            div()
                .debug_selector(move || id.to_string())
                .id(SharedString::from(id))
                .h(px(20.0))
                .px(px(7.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .flex_none()
                .cursor_pointer()
                .rounded(px(6.0))
                .when(on, |cell| cell.bg(rgb(theme::hover())))
                .child(if on {
                    txt(label, 12.0, theme::text()).into_any_element()
                } else {
                    txt(label, 12.0, theme::faint()).into_any_element()
                })
                .child(txt(glyph, 10.0, theme::faint()))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(32.0))
            .px(px(6.0))
            .gap(px(8.0))
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::page()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        button("traj-duration", duration_tip.clone(), actual, "\u{25f4}").on_click(
                            cx.listener(|this, _event, _window, cx| {
                                this.traj_actual_duration = !this.traj_actual_duration;
                                // The axis is the CLOCK in both readings. The
                                // toggle changes only how WIDE a block is drawn:
                                // OFF is the reference's equal-width bar (8px,
                                // still placed at the instant it happened), ON is
                                // the gantt where a block is as wide as its work.
                                //
                                // Both flags move together because bsh exposes ONE
                                // control, not the reference's two: leaving the
                                // time flag false would select `sequence`, which
                                // has no axis at all and packs a row into a solid
                                // bar - the exact defect this pair fixes.
                                this.traj_actual_time = true;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(
                            "traj-turns",
                            turns_tip.clone(),
                            self.traj_turns_collapsed,
                            turns_glyph,
                        )
                        .on_click(cx.listener(
                            |this, _event, _window, cx| {
                                this.traj_turns_collapsed = !this.traj_turns_collapsed;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button(
                            "traj-calls",
                            calls_tip.clone(),
                            self.traj_calls_collapsed,
                            calls_glyph,
                        )
                        .on_click(cx.listener(
                            |this, _event, _window, cx| {
                                this.traj_calls_collapsed = !this.traj_calls_collapsed;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(div().flex_1())
            // The field is flex:0 164px tall 22, radius-sm, a .5px border-l4 over
            // bg-layer-2, padded 0 6px with a 4px gap.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .h(px(22.0))
                    .w(px(164.0))
                    .flex_none()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .bg(rgb(theme::new_session_fill()))
                    .child(icon(IconName::Search, 11.0, theme::faint()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.traj_search).appearance(false)),
                    ),
            )
    }

    /// The three-lane strip.
    ///
    /// In Duration mode it is a real time axis: every lane is drawn against ONE
    /// domain, from the earliest event start to the latest event end, so a
    /// block's left edge is its event's instant and its width is the work it
    /// took. A pause is therefore empty space, and two events that started
    /// together line up vertically across lanes. A run recorded before the
    /// clock existed has no positions to place, so every lane falls back to
    /// record order rather than inventing an axis.
    pub(crate) fn traj_lanes(&self, events: &[Event], cx: &mut Context<Self>) -> impl IntoElement {
        // Build every lane before drawing any: the axis is the UNION of their
        // events, so a block's x depends on lanes other than its own.
        let lanes = [
            ("traj.laneInput", Kind::User, "input"),
            ("traj.laneModel", Kind::Assistant, "model"),
            ("traj.laneTools", Kind::Tool, "tools"),
        ];
        let records: Vec<Vec<LaneRecord>> = lanes
            .iter()
            .map(|(_, kind, _)| {
                events
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| event.kind == *kind)
                    .map(|(index, event)| LaneRecord {
                        index,
                        turn: event.turn,
                        start: event.at,
                        duration_ms: event.ms,
                    })
                    .collect()
            })
            .collect();
        let axis = TimeAxis::across(&records.iter().map(Vec::as_slice).collect::<Vec<_>>());
        let projection = self.traj_projection();
        // EVERY timed projection is placed on the shared clock, including the
        // equal-width ones. A block's LEFT EDGE always comes from when its
        // event happened; only its WIDTH changes with the equal-width switch.
        // Routing equal width away from the axis is what packed a row into a
        // solid bar and lost the idle gaps - the reference never does that, it
        // overrides `width` to 8px and keeps `left` from the start time.
        let real_time = projection.is_timed();
        // Measured (TrajectoryTimeline.module.css): a 50px plot on bg-layer-2
        // over a .5px border-l2, a 44px label column split off by a .5px
        // border-l1, and 10px labels pinned at y 7 / 21 / 35 (a 14px pitch) and
        // right-aligned 3px in.
        div()
            .debug_selector(|| "traj-strip".to_string())
            .flex()
            .flex_col()
            .h(px(50.0))
            .relative()
            // Nothing in the strip may paint outside its own bounds: the
            // lanes clamp their blocks, and this is the whole-strip backstop
            // that also covers the earlier-history affordance.
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::new_session_fill()))
            // Measured (.earlierHistory): a 28px affordance over the track's
            // left edge, label-secondary at .72, fading left to right. It is a
            // real control here too: it pulls the window of older messages the
            // log is drawn from, so a long session can be read backwards.
            .child(
                div()
                    .debug_selector(|| "traj-earlier".to_string())
                    .id("traj-earlier")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(44.0))
                    .w(px(28.0))
                    .flex()
                    .items_center()
                    .pl(px(3.0))
                    .cursor_pointer()
                    // gpui-pre exposes no gradient on this seam, so the wash
                    // is the plot's own fill rather than a fade.
                    .bg(rgb(theme::new_session_fill()))
                    // The affordance turns into the shared busy mark while a
                    // page is loading, so a slow read never looks like a dead
                    // control.
                    .child(if self.trace.is_busy() || self.older.is_busy() {
                        div()
                            .debug_selector(|| "trace-loading".to_string())
                            .child(spinner())
                            .into_any_element()
                    } else {
                        txt("\u{2039}", 13.0, theme::muted())
                            .opacity(0.72)
                            .into_any_element()
                    })
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.load_older_in_trajectory(cx);
                    })),
            )
            .children(
                lanes
                    .into_iter()
                    .enumerate()
                    .map(|(lane_index, (key, kind, lane_id))| {
                        let lane = &records[lane_index];
                        // The last prepaint's track width, shared by all three
                        // lanes. Each track sets it; they are the same width
                        // because they are the same row minus the same label.
                        let lane_width = self.traj_lane_w.clone();
                        let track = div()
                            .debug_selector(move || format!("traj-lane-{lane_id}"))
                            .flex_1()
                            .min_w_0()
                            .h(px(6.0))
                            .ml(px(3.0))
                            // The blocks are absolute children of this box, so
                            // relative(1.0) resolves to THIS box's right edge.
                            // No padding or border sits here on purpose: the
                            // sentinel below measures 100% of the same box the
                            // fractions resolve against, so a future inset would
                            // still make 1.0 the track's own right edge.
                            .relative()
                            // The backstop. The measured geometry already clamps
                            // every block, but the one frame that still carries a
                            // pre-resize width (and any arithmetic slip) must not
                            // paint past the lane either.
                            .overflow_hidden()
                            // The FIRST child is a full-size, out-of-flow
                            // sentinel: its prepainted bounds ARE the track's
                            // width, which is what the pixel floor and the
                            // right-edge clamp are computed against. It paints
                            // nothing and is not a hit target.
                            .child(div().absolute().top_0().left_0().w_full().h_full())
                            .on_children_prepainted(move |children, _window, _cx| {
                                if let Some(bounds) = children.first() {
                                    lane_width.set(Some(f32::from(bounds.size.width)));
                                }
                            });
                        let track = if real_time {
                            // One common axis, absolute fractions: x is the event's
                            // instant and width its duration, so a pause stays empty
                            // and lanes line up vertically.
                            let blocks = match axis {
                                Some(axis) => time_lane(lane, axis),
                                // No clock anywhere: record order is the only honest
                                // placement, and every event still draws.
                                None => equal_lane(lane),
                            };
                            // Equal width keeps the real POSITION and drops the
                            // real WIDTH; the other readings keep both.
                            let equal_width = projection.equal_width();
                            track.relative().children(blocks.into_iter().map(|block| {
                                let width = if equal_width {
                                    BlockWidth::PlacedFixed {
                                        x: block.x,
                                        width: block.width,
                                    }
                                } else {
                                    BlockWidth::Placed {
                                        x: block.x,
                                        width: block.width,
                                    }
                                };
                                self.timeline_cell(
                                    kind,
                                    block.index,
                                    self.traj_select == Some(block.index),
                                    width,
                                    cx,
                                )
                            }))
                        } else {
                            // Unreachable for a timed projection, which always takes
                            // the placed path above. This is the SEQUENCE reading: no
                            // clock, so record order is the axis and every block keeps
                            // the same share.
                            let legacy: Vec<(usize, usize, Option<u64>, u64)> = lane
                                .iter()
                                .map(|record| {
                                    (
                                        record.index,
                                        record.turn,
                                        record.start,
                                        record.duration_ms.unwrap_or(0),
                                    )
                                })
                                .collect();
                            let equal_width = projection.equal_width();
                            let projected = project_lane(&legacy, projection, MINIMUM_BLOCK_MS);
                            track.flex().flex_row().gap(px(1.0)).when_some(
                                projected,
                                |track, (domain, spans)| {
                                    let domain = domain.max(1.0);
                                    let mut cursor = 0.0f32;
                                    let mut children: Vec<AnyElement> = Vec::new();
                                    for span in spans {
                                        // A gap is real idle time, and the equal-width
                                        // timed reading keeps it as a spacer.
                                        if span.start > cursor {
                                            children.push(
                                                div()
                                                    .flex_grow((span.start - cursor) / domain)
                                                    .h(px(1.0))
                                                    .into_any_element(),
                                            );
                                        }
                                        let on = self.traj_select == Some(span.index);
                                        children.push(self.timeline_cell(
                                            kind,
                                            span.index,
                                            on,
                                            if equal_width {
                                                BlockWidth::Fixed(EQUAL_BLOCK_W)
                                            } else {
                                                BlockWidth::Share(span.width / domain)
                                            },
                                            cx,
                                        ));
                                        cursor = cursor.max(span.start + span.width);
                                    }
                                    track.children(children)
                                },
                            )
                        };
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(14.0))
                            .child(
                                // The label column: 44 wide, split by a hairline, text
                                // right-aligned 3px in at 10px.
                                div()
                                    .w(px(44.0))
                                    .h_full()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .pr(px(3.0))
                                    .border_r_1()
                                    .border_color(rgb(theme::border_soft()))
                                    .child(txt(self.state.lang.tr(key), 10.0, theme::faint())),
                            )
                            .child(track)
                    }),
            )
    }

    /// One clickable block in a timeline lane.
    ///
    /// Measured (TrajectoryTimeline.module.css): 8px tall at radius 1, the lane
    /// kind's own ink at full opacity, `data-selected=false` dimming to .2 when a
    /// range excludes it, and the block under the pointer (or the selected one)
    /// lifting to a 2px business-primary ring. The reference also carries
    /// `data-timeline-record-index`, which is what makes a block a way BACK to
    /// the record it stands for rather than a picture of it.
    /// The projection the reader's two switches select.
    ///
    /// The Duration control sets BOTH switches, so the mode the reader names
    /// Duration is a real-time gantt; either timed reading is laid out by the
    /// shared `ui::lane` geometry.
    pub(crate) fn traj_projection(&self) -> Projection {
        Projection::of(self.traj_actual_duration, self.traj_actual_time)
    }

    /// One clickable block in a timeline lane.
    ///
    /// Measured (TrajectoryTimeline.module.css): 8px tall at radius 1, the lane
    /// kind's own ink at full opacity, `data-selected=false` dimming to .2 when a
    /// range excludes it, and the block under the pointer (or the selected one)
    /// lifting to a 2px business-primary ring. The reference also carries
    /// `data-timeline-record-index`, which is what makes a block a way BACK to
    /// the record it stands for rather than a picture of it.
    fn timeline_cell(
        &self,
        kind: Kind,
        index: usize,
        selected: bool,
        width: BlockWidth,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ink = kind.ink();
        let ring = theme::accent();
        // A fixed width is a real pixel box; a share is a flex weight. Keeping
        // the two apart is what makes equal width mean the SAME 8px on every
        // lane regardless of how much of the track a lane owns.
        let base = div()
            .debug_selector(move || format!("traj-cell-{}-{}", kind.key(), index))
            .h(px(6.0))
            .flex_none()
            .rounded(px(1.0))
            .bg(rgb(ink))
            .when(selected, |block| block.border_1().border_color(rgb(ring)));
        // The track's measured width, from the last layout pass. Without it a
        // block can only be a fraction: it is still clipped by the track, and
        // the next frame places it in pixels. With it, the fraction is fitted
        // to the real track so the pixel floor and the right-edge clamp are
        // exact rather than guessed.
        let lane_w = self.traj_lane_w.get();
        let cell = match width {
            BlockWidth::Fixed(w) => base.w(px(w)),
            BlockWidth::Share(share) => base.flex_grow(share.max(0.0001)),
            // Time-proportional: the left edge is the event's instant and the
            // width its span, both fractions of the common axis. The 2px floor
            // keeps an instant (or untimed) event pressable; when that floor
            // would cross the axis end, block_box shifts the LEFT edge left by
            // just enough to keep the block fully visible and fully inside.
            BlockWidth::Placed { x, width } => match lane_w {
                Some(track_w) => {
                    let block = block_box(x, width, track_w, MIN_BLOCK_W);
                    base.absolute().top_0().left(px(block.x)).w(px(block.width))
                }
                // First frame after the strip mounts: no measurement yet. The
                // width is clamped to the room left on the axis (there is no
                // track width to express the floor in) and the track clips it.
                None => base
                    .absolute()
                    .top_0()
                    .left(relative(x.clamp(0.0, 1.0)))
                    .w(relative(width.clamp(0.0, 1.0).min(1.0 - x.clamp(0.0, 1.0)))),
            },
            // Placed by time, sized by the equal-width rule. The width argument
            // is ignored on purpose: 8px is the whole point of the mode, and
            // taking it from the caller is how the two readings drift apart.
            // The fixed width still fits the track: at the axis end the block
            // slides left by its own width instead of hanging over the edge.
            BlockWidth::PlacedFixed { x, .. } => match lane_w {
                Some(track_w) => {
                    let block = fixed_box(x, EQUAL_BLOCK_W, track_w);
                    base.absolute().top_0().left(px(block.x)).w(px(block.width))
                }
                None => base
                    .absolute()
                    .top_0()
                    .left(relative(x.clamp(0.0, 1.0)))
                    .w(px(EQUAL_BLOCK_W)),
            },
        };
        cell.debug_selector(move || format!("traj-cell-hit-{}", index))
            .id(SharedString::from(format!("traj-cell-{}", index)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // The block IS the record: pressing it selects that row and
                // opens its detail panel, which is the reference's own
                // `onRecordSelect`.
                this.traj_select = Some(index);
                this.traj_tab = 0;
                this.traj_scroll.scroll_to_top_of_item(index);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The synthetic row that stands in for a collapsed turn's hidden steps.
    ///
    /// Measured (TrajectoryTable.module.css + its renderer): the row is 20px
    /// tall rather than the usual 30, it is a pointer with the standard hover
    /// fill, and its content is `…` in label-tertiary at weight 600 followed
    /// by an ellipsised summary in label-secondary at 12/16. Pressing it
    /// expands that turn again, which is what makes the row a control and not
    /// a label.
    fn traj_summary_row(
        &self,
        index: usize,
        opens_turn: bool,
        summary: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = theme::border_soft();
        let row = div()
            .debug_selector(move || format!("traj-collapsed-{index}"))
            .id(SharedString::from(format!("traj-collapsed-{index}")));
        let row = row
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .when(opens_turn, |row| row.pt(px(4.0)))
            .h(px(20.0))
            .pl(px(36.0))
            .pr(px(4.0))
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                // The reference's collapsed row takes the standard hover fill.
                this.traj_summary_hover = *hovered;
                cx.notify();
            }))
            .when(self.traj_summary_hover, |row| row.bg(rgb(hover)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // Pressing the summary expands the turn it hides.
                this.traj_turns_collapsed = false;
                this.traj_select = None;
                this.traj_scroll.scroll_to_top_of_item(index);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w_0()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .child(
                        txt("\u{2026}", 12.0, theme::faint())
                            .mr(px(6.0))
                            .font_weight(FontWeight::BOLD),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(txt(summary.to_string(), 12.0, theme::muted())),
                    ),
            );
        super::hairline_bottom(row)
            .border_color(rgb(theme::border_soft()))
            .into_any_element()
    }
    /// One log line.
    fn traj_row(
        &self,
        index: usize,
        event: &Event,
        opens_turn: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = self.traj_select == Some(index);
        // A sub-call sits one rail in from the call that was running when it
        // started, the way the reference indents its nested step rows.
        let is_call = matches!(event.kind, Kind::Tool | Kind::Subtool);
        let indent = if event.kind == Kind::Subtool {
            18.0
        } else {
            0.0
        };
        let row = div()
            .debug_selector(|| format!("traj-row-{index}"))
            .id(SharedString::from(format!("traj-row-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            // A new turn is parted by four pixels of air, not by a rule: the
            // reference's 2px border made a heavy grid with the row hairlines.
            .when(opens_turn, |row| row.pt(px(4.0)))
            // Measured (table td): a 30px line, 0 8px cells, a .5px border-l1
            // hairline under each, hover on interactive-bg-hover and
            // interactive-bg-active for the selected line.
            .h(px(30.0))
            // A margin, not padding: padding moves the content inside the row's
            // box, so the row would still start on the same edge.
            .ml(px(indent))
            .pl(px(8.0))
            .pr(px(8.0))
            // A hairline, not a pixel border: a 30px row with a 1px rule under it
            // and a 2px rule at each turn read as a grid of heavy lines. The turn
            // rule is gone entirely - the turn still reads from its rail and its
            // first line - so the log is rows on the page, not a table.
            .cursor_pointer()
            .hover(|row| row.bg(rgb(theme::hover())))
            .when(on, |row| row.bg(rgb(theme::selected_row())))
            .child(
                div().w(px(6.0)).flex_none().child(
                    div()
                        .w(px(4.0))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(rgb(event.kind.ink())),
                ),
            )
            .child(self.traj_chip(event))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    // Measured: a tool or subtool row's text and its result run in
                    // the CODE face at 12/18 (`--ds-font-family-code`,
                    // `toolCallNameTypeface`), because a call is written as code.
                    .when(is_call, |cell| cell.font_family(theme::CODE_FONT))
                    .child(txt(event.title.clone(), 12.0, theme::text()).line_height(px(18.0))),
            )
            .when(!event.outcome.is_empty(), |row| {
                row.child(
                    div()
                        .flex_none()
                        .max_w(px(320.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .when(is_call, |cell| cell.font_family(theme::CODE_FONT))
                        .child(
                            txt(format!("\u{2192} {}", event.outcome), 12.0, theme::faint())
                                .line_height(px(18.0)),
                        ),
                )
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.traj_select = Some(index);
                this.traj_tab = 0;
                cx.notify();
            }));
        super::hairline_bottom(row)
            .border_color(rgb(theme::border_soft()))
            .into_any_element()
    }

    /// The detail panel, from the reference's own header, tabs and fields.
    pub(crate) fn traj_detail(&self, event: &Event, cx: &mut Context<Self>) -> AnyElement {
        let tabs = [
            self.state.lang.tr("traj.summary"),
            self.state.lang.tr("traj.preview"),
            self.state.lang.tr("traj.raw"),
            self.state.lang.tr("traj.sourceTab"),
            self.state.lang.tr("traj.policyTab"),
        ];
        // The policy tab is bsh's own: the reference has no equivalent, because
        // in DSH a policy question and the tool that asked it are separate
        // records. Here every CALL is made through the policy seam, so the
        // decision is a property of the call and can be shown beside it.
        // The call's own argument, not its display title: the audit ring records
        // the real path, so matching against a clipped `# tool {…}` string would
        // never join.
        let verdict = self.state.policy_verdict_for(&event.source, &event.target);
        let body: String = match self.traj_tab {
            1 => event.outcome.clone(),
            2 => event.raw.clone(),
            3 => event.source.clone(),
            4 => String::new(),
            _ => {
                if event.outcome.is_empty() {
                    event.raw.clone()
                } else {
                    format!("{}\n\n{}", event.title, event.outcome)
                }
            }
        };
        let field = |label: String, value: String| {
            div()
                .flex()
                .flex_row()
                .gap(px(12.0))
                .child(txt(label, 12.0, theme::muted()).w(px(72.0)).flex_none())
                .child(txt(value, 12.0, theme::text()))
        };
        div()
            .debug_selector(|| "traj-detail".to_string())
            .id("traj-detail")
            .w(px(470.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(theme::border()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(14.0))
                    .h(px(40.0))
                    .border_b_1()
                    .border_color(rgb(theme::border()))
                    .child(self.traj_chip(event))
                    .child(txt(
                        format!(
                            "{} {} \u{b7} {}",
                            self.state.lang.tr("traj.turn"),
                            event.turn,
                            self.state.lang.tr("traj.message"),
                        ),
                        13.0,
                        theme::muted(),
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .debug_selector(|| "traj-detail-close".to_string())
                            .id("traj-detail-close")
                            .w(px(22.0))
                            .h(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Close, 12.0, theme::muted()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.traj_select = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(16.0))
                    .px(px(14.0))
                    .h(px(36.0))
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(theme::border()))
                    .children(tabs.into_iter().enumerate().map(|(index, label)| {
                        let on = self.traj_tab == index;
                        div()
                            .debug_selector(|| format!("traj-tab-{index}"))
                            .id(SharedString::from(format!("traj-tab-{index}")))
                            .h_full()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .border_b_2()
                            .border_color(rgb(if on { theme::accent() } else { theme::page() }))
                            // Measured (.detailTab): the panel's tabs are 13px,
                            // one step above the table's 12.
                            .child(txt(
                                label,
                                13.0,
                                if on { theme::accent() } else { theme::muted() },
                            ))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.traj_tab = index;
                                cx.notify();
                            }))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .px(px(14.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(theme::border()))
                    .child(field(
                        self.state.lang.tr("traj.source"),
                        event.source.clone(),
                    ))
                    .child(field(
                        self.state.lang.tr("traj.status"),
                        self.state.lang.tr(if event.live {
                            "traj.running"
                        } else {
                            "traj.completed"
                        }),
                    ))
                    .child(field(
                        self.state.lang.tr("traj.durationLabel"),
                        duration_label(event.ms),
                    )),
            )
            .child(
                div()
                    .debug_selector(|| "traj-detail-body".to_string())
                    .id("traj-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .when(self.traj_tab == 4, |panel| {
                        panel.child(self.policy_body(&verdict, cx))
                    })
                    .when(self.traj_tab != 4, |panel| {
                        panel.child(txt(body, 12.0, theme::text()).line_height(px(20.0)))
                    }),
            )
            .into_any_element()
    }

    /// The Permission/Policy tab: the decision behind one call.
    ///
    /// bsh's own surface. It answers the three questions a reader actually has
    /// about a call: WHICH permission was in force, WHAT the policy was asked,
    /// and HOW it answered. A refusal adds the rule and the reason, because a
    /// refusal without them is an unexplained wall.
    fn policy_body(&self, verdict: &PolicyVerdict, _cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        // The preset comes from the state's one policy fact, not from the tool
        // name: `applied_preset` reads the policy the turn was built for (a
        // denial keeps the preset recorded at its decision), falling back to
        // the session-level effective preset. The row resolves the id through
        // the same catalogue the composer chip and the settings pane use, so a
        // named preset reads by its display name.
        let preset = self.state.preset_label(&verdict.preset);
        let row = |label: String, value: String| {
            div()
                .debug_selector(|| format!("policy-row-{label}"))
                .flex()
                .flex_row()
                .gap(px(12.0))
                .child(txt(label, 12.0, theme::muted()).w(px(96.0)).flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(txt(value, 12.0, theme::text()).line_height(px(20.0))),
                )
        };
        let verdict_ink = if verdict.allowed {
            theme::success()
        } else {
            theme::error()
        };
        let verdict_label = if verdict.allowed {
            lang.tr("traj.policyAllowed")
        } else {
            lang.tr("traj.policyDenied")
        };
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(row(
                lang.tr("traj.policyPermission"),
                verdict.permission.clone(),
            ))
            .child(row(lang.tr("traj.policyAction"), verdict.action.clone()))
            .child(row(lang.tr("traj.policyTarget"), verdict.target.clone()))
            .child(
                div()
                    .debug_selector(|| "policy-verdict".to_string())
                    .flex()
                    .flex_row()
                    .gap(px(12.0))
                    .child(
                        txt(lang.tr("traj.policyVerdict"), 12.0, theme::muted())
                            .w(px(96.0))
                            .flex_none(),
                    )
                    .child(txt(verdict_label, 12.0, verdict_ink).font_weight(FontWeight::BOLD)),
            );
        if let Some(rule) = &verdict.rule {
            column = column.child(row(lang.tr("traj.policyRule"), rule.clone()));
        }
        if let Some(reason) = &verdict.reason {
            column = column.child(row(lang.tr("traj.policyReason"), reason.clone()));
        }
        // The preset the call ran under, drawn as the drawer's own pill. It is
        // the last fact row because it names the declaration behind every row
        // above it, and the note below is explanation rather than data.
        column = column.child(
            div()
                .debug_selector(|| "policy-preset".to_string())
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(
                    txt(lang.tr("traj.policyPreset"), 12.0, theme::muted())
                        .w(px(96.0))
                        .flex_none(),
                )
                .child(
                    div()
                        .debug_selector(|| format!("policy-preset-{preset}"))
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .child(self.traj_badge(preset.clone(), theme::text())),
                ),
        );
        column
            .child(
                div()
                    .mt(px(8.0))
                    .child(txt(lang.tr("traj.policyNote"), 11.0, theme::faint())),
            )
            .into_any_element()
    }

    /// The whole tab.
    pub(crate) fn render_trajectory_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let events = events(self);
        let query = self.traj_query.clone();
        let visible: Vec<(usize, &Event)> = events
            .iter()
            .enumerate()
            .filter(|(_, event)| matches(event, &query))
            .collect();
        let detail = self
            .traj_select
            .and_then(|index| events.get(index))
            .map(|event| self.traj_detail(event, cx));
        // Which lines open a turn: the first one whose turn differs from the
        // line above, computed before the rows so no closure has to carry it.
        let mut opens: Vec<bool> = Vec::with_capacity(visible.len());
        let mut previous: Option<usize> = None;
        for (_, event) in &visible {
            opens.push(previous != Some(event.turn));
            previous = Some(event.turn);
        }
        // The collapse actions fold ROWS, not the strip: the picture of the run
        // never lies about what happened, which is the reference's own rule.
        // `Calls` folds a turn to its prompt plus one summary row; `Turns`
        // removes the steps inside a turn without adding a summary line.
        let rows: Vec<AnyElement> = if self.traj_turns_collapsed {
            collapse_turns(&visible, &opens, self.state.lang)
                .into_iter()
                .filter_map(|(index, opens_turn, summary)| {
                    let event = events.get(index)?;
                    Some(match summary {
                        Some(text) => self.traj_summary_row(index, opens_turn, &text, cx),
                        None => self.traj_row(index, event, opens_turn, cx),
                    })
                })
                .collect()
        } else if self.traj_calls_collapsed {
            visible
                .iter()
                .zip(opens)
                .filter(|((_, event), opens_turn)| *opens_turn || event.kind == Kind::Compaction)
                .map(|((index, event), opens_turn)| self.traj_row(*index, event, opens_turn, cx))
                .collect()
        } else {
            visible
                .iter()
                .zip(opens)
                .map(|((index, event), opens_turn)| self.traj_row(*index, event, opens_turn, cx))
                .collect()
        };
        div()
            .debug_selector(|| "trajectory".to_string())
            .id("trajectory")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.traj_toolbar(cx))
            .child(self.traj_lanes(&events, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .debug_selector(|| "traj-list".to_string())
                            .id("traj-list")
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.traj_scroll)
                            // The same eased wheel the chat uses, so the log
                            // scrolls with the trackpad the way the reference
                            // does instead of stepping a notch at a time.
                            .on_scroll_wheel(cx.listener(super::transcript::traj_wheel))
                            .py(px(6.0))
                            .flex()
                            .flex_col()
                            .children(rows),
                    )
                    .children(detail),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        action_of, collapse_turns, duration_label, events_for, project_lane, verdict_for,
        BlockWidth, Kind, PolicyVerdict, Projection, MINIMUM_BLOCK_MS,
    };
    use crate::i18n::Lang;
    use crate::model::{ChatMessage, Session};

    fn session_with(messages: Vec<ChatMessage>) -> Session {
        let mut session = Session::new("w", "timing");
        session.messages = messages;
        session
    }

    /// The two switches compose into four projections, exactly as the
    /// reference's own ternary does.
    #[test]
    fn the_two_switches_compose_four_projections() {
        assert_eq!(Projection::of(false, false), Projection::Sequence);
        assert_eq!(Projection::of(false, true), Projection::Time);
        assert_eq!(Projection::of(true, false), Projection::Duration);
        assert_eq!(Projection::of(true, true), Projection::Actual);
        // Both timed readings keep a real width; no reading removes idle time.
        assert!(Projection::Sequence.equal_width());
        assert!(Projection::Time.equal_width());
        assert!(!Projection::Duration.equal_width());
        assert!(!Projection::Actual.equal_width());
        assert!(!Projection::Sequence.is_timed());
        assert!(Projection::Time.is_timed());
        assert!(Projection::Duration.is_timed());
        assert!(Projection::Actual.is_timed());
    }

    /// The action a tool name asks the policy about, by NAME rule so an
    /// unknown tool still gets a sensible answer.
    #[test]
    fn a_tool_name_resolves_to_the_action_the_policy_sees() {
        assert_eq!(action_of("read_file"), "read");
        assert_eq!(action_of("write_file"), "write");
        assert_eq!(action_of("edit"), "write");
        assert_eq!(action_of("bash"), "spawn");
        assert_eq!(action_of("web_fetch"), "egress");
        assert_eq!(
            action_of("some_future_tool"),
            "read",
            "an unknown tool is read, not blank"
        );
    }

    /// A call with no matching refusal was allowed, and carries the
    /// permission that was in force when it ran.
    #[test]
    fn a_call_with_no_refusal_reads_as_allowed() {
        let verdict = verdict_for("read_file", "/w/src/main.rs", "workspace-write", "ptc", &[]);
        assert!(verdict.allowed);
        assert_eq!(verdict.permission, "workspace-write");
        assert_eq!(verdict.preset, "ptc");
        assert_eq!(verdict.action, "read");
        assert_eq!(verdict.target, "/w/src/main.rs");
        assert!(verdict.rule.is_none() && verdict.reason.is_none());
    }

    /// A refusal joined to its call carries the rule and the reason, which is
    /// what turns a wall into an explanation.
    #[test]
    fn a_refusal_joins_its_call_and_names_the_rule() {
        let refusals = vec![PolicyVerdict {
            permission: "read-only".to_string(),
            preset: "strict".to_string(),
            action: "write".to_string(),
            target: "/w/src/main.rs".to_string(),
            allowed: false,
            rule: Some("deny-writes".to_string()),
            reason: Some("the session is read-only".to_string()),
        }];
        let verdict = verdict_for(
            "write_file",
            "/w/src/main.rs",
            "read-only",
            "ptc",
            &refusals,
        );
        assert!(!verdict.allowed);
        assert_eq!(verdict.rule.as_deref(), Some("deny-writes"));
        assert_eq!(verdict.reason.as_deref(), Some("the session is read-only"));
        // The refusal's own permission wins: the ring records what was in
        // force at the DECISION, which may differ from the current setting.
        assert_eq!(verdict.permission, "read-only");
        // The preset follows the same rule: the decision's own record wins.
        assert_eq!(verdict.preset, "strict");
    }

    /// Two calls to the same tool against different paths are not confused,
    /// because the join is by tool AND target.
    #[test]
    fn a_refusal_does_not_leak_to_a_different_target() {
        let refusals = vec![PolicyVerdict {
            permission: "read-only".to_string(),
            preset: "strict".to_string(),
            action: "write".to_string(),
            target: "/w/secret.txt".to_string(),
            allowed: false,
            rule: Some("deny-path".to_string()),
            reason: None,
        }];
        let other = verdict_for(
            "write_file",
            "/w/public.txt",
            "workspace-write",
            "ptc",
            &refusals,
        );
        assert!(other.allowed, "another path is not this refusal");
        assert_eq!(
            other.preset, "ptc",
            "an allowed call takes the caller's fallback"
        );
        let same = verdict_for("write_file", "/w/secret.txt", "read-only", "ptc", &refusals);
        assert!(!same.allowed);
    }

    /// An action mismatch is not a match either: a refused spawn says nothing
    /// about an allowed read of the same target.
    #[test]
    fn a_refusal_does_not_leak_across_actions() {
        let refusals = vec![PolicyVerdict {
            permission: "read-only".to_string(),
            preset: "strict".to_string(),
            action: "spawn".to_string(),
            target: "/w".to_string(),
            allowed: false,
            rule: Some("deny-spawn".to_string()),
            reason: None,
        }];
        let read = verdict_for("read_file", "/w/src/main.rs", "read-only", "ptc", &refusals);
        assert!(read.allowed, "a spawn refusal is not a read refusal");
    }
    /// `Turns` folds a turn to its first content row plus one summary row
    /// that counts what it hides, and leaves a one-row turn alone.
    #[test]
    fn collapsing_a_turn_emits_one_summary_row() {
        let mut session = session_with(vec![
            ChatMessage::user("one"),
            ChatMessage::assistant(),
            ChatMessage::user("two"),
            ChatMessage::assistant(),
        ]);
        if let Some(reply) = session.messages.get_mut(1) {
            reply.text = "a one".to_string();
        }
        if let Some(reply) = session.messages.get_mut(3) {
            reply.text = "a two".to_string();
        }
        let log = events_for(&session, false, Lang::En);
        let visible: Vec<(usize, &super::Event)> = log.iter().enumerate().collect();
        // Every row opens its turn here, because the two turns alternate.
        let opens: Vec<bool> = visible.iter().map(|(_, event)| event.turn != 1).collect();
        let opens: Vec<bool> = std::iter::once(true)
            .chain(opens.into_iter().skip(1))
            .collect();
        let rows = collapse_turns(&visible, &opens, Lang::En);
        // Turn 1 keeps its prompt and gains a summary; turn 2 does the same.
        let summaries: Vec<&String> = rows
            .iter()
            .filter_map(|(_, _, summary)| summary.as_ref())
            .collect();
        assert!(!summaries.is_empty(), "a collapsed turn carries a summary");
        assert!(
            summaries.iter().all(|text| text.contains('1')),
            "the summary counts the hidden step: {summaries:?}"
        );
    }

    /// A turn holding only its own opening row is left as it is: there is
    /// nothing to hide, and a summary row would be a lie.
    #[test]
    fn a_one_row_turn_gets_no_summary() {
        let session = session_with(vec![ChatMessage::user("only")]);
        let log = events_for(&session, false, Lang::En);
        let visible: Vec<(usize, &super::Event)> = log.iter().enumerate().collect();
        let opens = vec![true; visible.len()];
        let rows = collapse_turns(&visible, &opens, Lang::En);
        assert_eq!(rows.len(), visible.len(), "nothing was folded");
        assert!(rows.iter().all(|(_, _, summary)| summary.is_none()));
    }
    /// A lane with a recorded clock, as (index, turn, started-at, ms).
    fn timed_lane() -> Vec<(usize, usize, Option<u64>, u64)> {
        vec![
            (0, 1, Some(0), 1_000),
            (1, 1, Some(1_000), 1_000),
            // A 60s pause, then work again.
            (2, 2, Some(62_000), 1_000),
        ]
    }

    /// `sequence` places one unit slot per record, back to back, and ignores
    /// the clock entirely - that is how a run with no timestamps still reads.
    #[test]
    fn sequence_places_one_slot_per_record() {
        let (domain, spans) = project_lane(&timed_lane(), Projection::Sequence, MINIMUM_BLOCK_MS)
            .expect("a projection");
        assert_eq!(domain, 3.0, "three records, three slots");
        assert_eq!(spans.len(), 3);
        for (slot, span) in spans.iter().enumerate() {
            assert_eq!(span.start, slot as f32, "slots run end to end");
            assert_eq!(span.width, 1.0);
        }
        // A lane with no clock at all still projects, because sequence needs
        // no timestamps.
        let untimed = vec![(0, 1, None, 0), (1, 1, None, 0)];
        assert!(project_lane(&untimed, Projection::Sequence, MINIMUM_BLOCK_MS).is_some());
        assert!(
            project_lane(&untimed, Projection::Actual, MINIMUM_BLOCK_MS).is_none(),
            "a timed projection has no axis without a clock"
        );
    }

    /// `time` keeps the real axis and draws every block at zero width, which
    /// is what the renderer turns into its fixed 8px box.
    #[test]
    fn time_keeps_real_gaps_but_equal_width() {
        let (domain, spans) =
            project_lane(&timed_lane(), Projection::Time, MINIMUM_BLOCK_MS).expect("a projection");
        assert_eq!(domain, 63_000.0, "the whole span, pause included");
        assert!(spans.iter().all(|span| span.width == 0.0));
        // The pause is visible: the third block starts 62s in, not at 2s.
        assert_eq!(spans[2].start, 62_000.0);
    }

    /// `duration` is the real-time reading: the pause stays on the axis and a
    /// block is as wide as the work it stands for.
    #[test]
    fn duration_keeps_the_idle_gap_and_the_real_width() {
        let (domain, spans) = project_lane(&timed_lane(), Projection::Duration, MINIMUM_BLOCK_MS)
            .expect("a projection");
        assert_eq!(domain, 63_000.0, "the whole span, the 60s pause included");
        assert_eq!(
            spans[2].start, 62_000.0,
            "work resumes where it really started"
        );
        assert_eq!(
            spans[0].width, 1_000.0,
            "a one-second call is one second wide"
        );
        // The equal-width timed reading keeps the same axis but collapses the
        // width to its fixed box, which is the mode the toggle switches away
        // from rather than a difference in the axis.
        let (_, equal) =
            project_lane(&timed_lane(), Projection::Time, MINIMUM_BLOCK_MS).expect("a");
        assert_eq!(equal[2].start, spans[2].start, "same axis, same instant");
        assert!(equal.iter().all(|span| span.width == 0.0), "equal width");
    }

    /// `actual` is the gantt: real axis, and a block as wide as the work took.
    #[test]
    fn actual_keeps_real_widths_and_real_gaps() {
        let (domain, spans) = project_lane(&timed_lane(), Projection::Actual, MINIMUM_BLOCK_MS)
            .expect("a projection");
        assert_eq!(domain, 63_000.0);
        assert_eq!(
            spans[0].width, 1_000.0,
            "a one-second call is one second wide"
        );
        assert_eq!(spans[2].start, 62_000.0);
    }

    /// A block never collapses below the minimum, so an instant call is still
    /// a control the reader can press.
    #[test]
    fn an_instant_call_still_gets_a_hit_area() {
        let lane = vec![(0, 1, Some(0), 0)];
        let (_, spans) = project_lane(&lane, Projection::Actual, MINIMUM_BLOCK_MS).expect("a");
        assert_eq!(spans[0].width, MINIMUM_BLOCK_MS as f32);
    }

    /// The two block widths stay distinguishable at the type level, which is
    /// what keeps equal-width a fixed pixel box rather than a track share.
    #[test]
    fn a_fixed_width_is_not_a_track_share() {
        assert_ne!(BlockWidth::Fixed(8.0), BlockWidth::Share(0.8));
        assert_ne!(
            BlockWidth::Share(0.8),
            BlockWidth::Placed { x: 0.0, width: 0.8 }
        );
    }
    /// The detail panel's formatter: milliseconds below a second, seconds
    /// above it, and an em dash when no clock recorded the row.
    #[test]
    fn durations_read_the_way_the_reference_writes_them() {
        assert_eq!(duration_label(None), "\u{2014}");
        assert_eq!(duration_label(Some(0)), "0 ms");
        assert_eq!(duration_label(Some(999)), "999 ms");
        assert_eq!(duration_label(Some(1_000)), "1.00 s");
        assert_eq!(duration_label(Some(9_999)), "10.00 s");
        assert_eq!(duration_label(Some(12_345)), "12.3 s");
    }

    /// A settled turn gives BOTH its rows a measured span: the user row the
    /// wait for the reply, the assistant row the turn's own duration.
    #[test]
    fn a_finished_turn_reports_measured_spans() {
        let mut user = ChatMessage::user("hello");
        user.timing_mut().start = Some(1_000);
        let mut assistant = ChatMessage::assistant();
        assistant.text = "hi".to_string();
        let timing = assistant.timing_mut();
        timing.start = Some(1_000);
        timing.first_output = Some(1_300);
        timing.ms = Some(2_500);
        let log = events_for(&session_with(vec![user, assistant]), false, Lang::En);
        assert_eq!(log.len(), 2, "a user row and an assistant row");
        assert_eq!(log[0].kind, Kind::User);
        assert_eq!(log[0].at, Some(1_000));
        assert_eq!(
            log[0].ms,
            Some(300),
            "the input row owns the wait for the reply"
        );
        assert_eq!(log[1].kind, Kind::Assistant);
        assert_eq!(log[1].at, Some(1_000));
        assert_eq!(
            log[1].ms,
            Some(2_500),
            "the model row owns the turn's settled span"
        );
    }

    /// A compaction is a row of its own, placed where the boundary it installed
    /// is, and it carries the same count the transcript marker reports.
    #[test]
    fn a_compaction_is_a_trajectory_row() {
        let mut session = session_with(vec![
            ChatMessage::user("one"),
            ChatMessage::assistant(),
            ChatMessage::user("two"),
            ChatMessage::assistant(),
        ]);
        // The boundary folds the first three rows, leaving the last.
        session.compacted = 2;
        session.compactions.push(crate::model::CompactionRecord {
            at: 0,
            items: 3,
            tokens: 512,
            summary: "## Current Work".to_string(),
            manual: true,
            after: 4,
        });
        let log = events_for(&session, false, Lang::En);
        let row = log
            .iter()
            .find(|event| event.kind == Kind::Compaction)
            .expect("the compaction is a row");
        assert!(
            row.title.contains("3"),
            "it names the items folded: {}",
            row.title
        );
        assert!(
            row.title.contains("512"),
            "and the tokens freed: {}",
            row.title
        );
        assert_eq!(row.raw, "## Current Work", "the raw tab is the checkpoint");
        // The row sits at the boundary: before the message it replaced the
        // history of, and after the rows that were folded.
        let marker_at = log
            .iter()
            .position(|event| event.kind == Kind::Compaction)
            .expect("a position");
        assert!(marker_at > 0, "rows precede it");
        assert!(
            log[marker_at + 1..]
                .iter()
                .any(|event| event.kind == Kind::User),
            "and the retained tail follows it"
        );
    }

    /// A session that was never compacted has no such row.
    #[test]
    fn an_uncompacted_session_has_no_compaction_row() {
        let session = session_with(vec![ChatMessage::user("hello"), ChatMessage::assistant()]);
        let log = events_for(&session, false, Lang::En);
        assert!(log.iter().all(|event| event.kind != Kind::Compaction));
    }

    /// A record written before the clock existed reports no duration; the view
    /// shows the unavailable mark rather than a zero.
    #[test]
    fn a_record_without_a_clock_reports_no_duration() {
        let mut assistant = ChatMessage::assistant();
        assistant.text = "hi".to_string();
        let log = events_for(
            &session_with(vec![ChatMessage::user("hello"), assistant]),
            false,
            Lang::En,
        );
        assert_eq!(log[0].ms, None, "no submission time was recorded");
        assert_eq!(log[1].ms, None, "and no settled span either");
    }

    /// A turn still streaming keeps its end open: the assistant row reports no
    /// invented elapsed time, while the user row already owns the wait it saw.
    #[test]
    fn a_live_turn_does_not_invent_an_elapsed_time() {
        let mut user = ChatMessage::user("hello");
        user.timing_mut().start = Some(1_000);
        let mut assistant = ChatMessage::assistant();
        assistant.text = "hi".to_string();
        let timing = assistant.timing_mut();
        timing.start = Some(1_000);
        timing.first_output = Some(1_200);
        let log = events_for(&session_with(vec![user, assistant]), true, Lang::En);
        assert_eq!(log[0].ms, Some(200), "the wait is already measured");
        assert_eq!(log[1].ms, None, "the model span is not closed yet");
        assert!(log[1].live, "and the row says it is running");
    }
}
