//! The Trajectory tab.
//!
//! The reference builds this tab from an event log, not from the transcript it
//! shares with Chat: a filter toolbar (Duration / Turns / Calls plus a search
//! field), a three-lane timeline strip, a dense list of typed events, and a
//! detail panel for the selected one. bsh has the same raw material - the
//! session's messages and their tool calls - so the view is built from that and
//! nothing is invented: the lanes are step densities rather than a time axis
//! because no event carries a timestamp, and the SUBTOOL / CONTEXT chips the
//! reference also draws have no producer in this model (a nested call and an
//! injected time-context message are both their harness's, not ours).

use gpui_kit::*;

use super::shell::Shell;
use super::todo_panel::spinner;
use gpui_kit::component::input::Input;

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
        });
    }
    for (index, message) in session.messages.iter().enumerate() {
        if message.role == crate::model::Role::User {
            turn += 1;
        }
        let turn_now = turn.max(1);
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

    /// The tag itself: measured .kindTag, 19px tall at radius-xs, 0 5px padding,
    /// 10px at weight 650.
    fn traj_tag(&self, event: &Event) -> Div {
        let ink = event.kind.ink();
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
                txt(self.state.lang.tr(event.kind.key()), 10.0, ink)
                    .font_weight(FontWeight::BOLD)
                    .line_height(px(16.0)),
            )
    }

    /// The filter toolbar: the segmented measure and the search field.
    pub(crate) fn traj_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = [
            self.state.lang.tr("traj.duration"),
            self.state.lang.tr("traj.turns"),
            self.state.lang.tr("traj.calls"),
        ];
        // Measured (TrajectoryToolbar.module.css): a 32px bar over a .5px
        // border-l2, inner padding 0 6px at gap 8, and the measures are plain
        // 20px TOGGLES - 12px type, padding 0 7px, gap 4 - that read pressed as
        // label-primary over interactive-bg-hover, with no pill and no border.
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
                    .children(modes.into_iter().enumerate().map(|(index, label)| {
                        let on = self.traj_mode == index;
                        let other = label.clone();
                        div()
                            .debug_selector(|| format!("traj-mode-{index}"))
                            .id(SharedString::from(format!("traj-mode-{index}")))
                            .h(px(20.0))
                            .px(px(7.0))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .rounded(px(6.0))
                            .when(on, |cell| {
                                cell.bg(rgb(theme::hover()))
                                    .child(txt(label, 12.0, theme::text()))
                            })
                            .when(!on, |cell| cell.child(txt(other, 12.0, theme::faint())))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.traj_mode = index;
                                cx.notify();
                            }))
                    })),
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
    /// It IS a time axis now: a call records when it started, so a lane places
    /// each event at its own instant and sizes it by how long it took, with the
    /// gaps between calls left empty. A run recorded before that timestamp
    /// existed has no positions to place, so the lanes fall back to step order -
    /// one segment per turn - rather than inventing an axis.
    pub(crate) fn traj_lanes(&self, events: &[Event], cx: &mut Context<Self>) -> impl IntoElement {
        // The shared domain: the earliest start to the latest end. Lanes align
        // because every one of them divides this same span.
        let starts: Vec<u64> = events.iter().filter_map(|e| e.at).collect();
        let domain = match (starts.iter().min(), starts.iter().max()) {
            (Some(first), Some(last)) if last > first => {
                let end = events
                    .iter()
                    .filter_map(|e| e.at.map(|at| at + e.ms.unwrap_or(0)))
                    .max()
                    .unwrap_or(*last);
                Some((*first, end))
            }
            _ => None,
        };
        let lanes = [
            ("traj.laneInput", Kind::User),
            ("traj.laneModel", Kind::Assistant),
            ("traj.laneTools", Kind::Tool),
        ];
        // Measured (TrajectoryTimeline.module.css): a 50px plot on bg-layer-2
        // over a .5px border-l2, a 44px label column split off by a .5px
        // border-l1, and 10px labels pinned at y 7 / 21 / 35 (a 14px pitch) and
        // right-aligned 3px in.
        div()
            .flex()
            .flex_col()
            .h(px(50.0))
            .relative()
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
            .children(lanes.into_iter().map(|(key, kind)| {
                // One segment per turn, weighted by the toolbar's measure: the
                // turn's slowest tool while Duration is chosen, one equal share
                // per turn while Turns is, and the turn's call count while Calls
                // is. That is what makes the segmented control live - the strip
                // is the measure, so switching it redraws the strip.
                let mut cells: Vec<(usize, f32)> = Vec::new();
                for event in events.iter().filter(|e| e.kind == kind) {
                    let weight = match self.traj_mode {
                        0 => event.ms.unwrap_or(0) as f32,
                        2 => 1.0,
                        _ => 1.0,
                    };
                    match cells.iter_mut().find(|(turn, _)| *turn == event.turn) {
                        Some((_, total)) => *total += weight,
                        None => cells.push((event.turn, weight)),
                    }
                }
                cells.sort_by_key(|(turn, _)| *turn);
                let tallest = cells.iter().map(|(_, w)| *w).fold(0.0f32, f32::max);
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(6.0))
                            .ml(px(3.0))
                            .flex()
                            .flex_row()
                            .gap(px(1.0))
                            .when_some(domain, |track, (first, last)| {
                                // Real time: a spacer for the wait, then a bar
                                // for the call. Both are the same unit (ms), so
                                // the lane reads as a gantt of the run.
                                let span = (last - first).max(1) as f32;
                                let mut cursor = first;
                                let mut children: Vec<Div> = Vec::new();
                                let mut placed: Vec<&Event> = events
                                    .iter()
                                    .filter(|e| e.kind == kind && e.at.is_some())
                                    .collect();
                                placed.sort_by_key(|e| e.at.unwrap_or(first));
                                for event in placed {
                                    let at = event.at.unwrap_or(cursor);
                                    if at > cursor {
                                        children.push(
                                            div().flex_grow((at - cursor) as f32 / span).h(px(1.0)),
                                        );
                                    }
                                    let span_ms = event.ms.unwrap_or(0).max(60) as f32;
                                    children.push(
                                        div()
                                            .debug_selector(|| {
                                                format!("traj-cell-{}-{}", kind.key(), event.turn)
                                            })
                                            .flex_grow(span_ms / span)
                                            .h(px(6.0))
                                            .rounded(px(1.0))
                                            .bg(rgb(kind.ink())),
                                    );
                                    cursor = cursor.max(at + event.ms.unwrap_or(0).max(60));
                                }
                                track.children(children)
                            })
                            .when(domain.is_none(), |track| {
                                track.children(cells.into_iter().map(|(turn, weight)| {
                                    let share = if tallest > 0.0 { weight / tallest } else { 1.0 };
                                    div()
                                        .debug_selector(|| {
                                            format!("traj-cell-{}-{turn}", kind.key())
                                        })
                                        .flex_grow(share.max(0.15))
                                        .h(px(if weight >= tallest && tallest > 0.0 {
                                            6.0
                                        } else {
                                            4.0
                                        }))
                                        .rounded(px(1.0))
                                        .bg(rgb(kind.ink()))
                                }))
                            }),
                    )
            }))
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
        ];
        let body: String = match self.traj_tab {
            1 => event.outcome.clone(),
            2 => event.raw.clone(),
            3 => event.source.clone(),
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
                    .id("traj-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(txt(body, 12.0, theme::text()).line_height(px(20.0))),
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
        let rows: Vec<AnyElement> = visible
            .iter()
            .zip(opens)
            .map(|((index, event), opens_turn)| self.traj_row(*index, event, opens_turn, cx))
            .collect();
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
    use super::{duration_label, events_for, Kind};
    use crate::i18n::Lang;
    use crate::model::{ChatMessage, Session};

    fn session_with(messages: Vec<ChatMessage>) -> Session {
        let mut session = Session::new("w", "timing");
        session.messages = messages;
        session
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
