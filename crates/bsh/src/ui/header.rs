//! The main column's title row and tab strip.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

use super::shell::{HandleSide, Shell};
use super::todo_panel::spinner;
use super::{
    hairline_all, hairline_bottom, hairline_left, hairline_top, icon, txt, DivExt, IconName,
};
use crate::state::Tab;
use crate::subagent::SubagentStatus;
use crate::theme;

/// The measured jobs panel: `.menu{z-index:100;border-radius:radius-lg;
/// background:specific-menu;width:500px;max-width:min(560px,100vw - 32px);
/// max-height:min(480px,100vh - 140px);gap:1px;padding:3px;position:absolute;
/// top:calc(100% + 5px);left:0;overflow:auto}`. The kit popup brings its own
/// radius, padding and surface; the width and the cap are the measured ones.
const JOBS_MENU_W: f32 = 500.0;
#[allow(dead_code)]
const JOBS_MENU_MAX_W: f32 = 560.0;
const JOBS_MENU_MAX_H: f32 = 480.0;

/// Static debug selectors for the first rows of the popover.
///
/// debug_bounds looks a selector up by a &'static str, so an id-derived name
/// can be drawn but never read back. These cover the rows a test can see inside
/// the measured 480px panel; the last entry names every row past the third so a
/// selector is always drawn.
const JOB_ROW_DEBUG: [&str; 4] = ["job-row-0", "job-row-1", "job-row-2", "job-row-many"];
const JOB_DOT_DEBUG: [&str; 4] = ["job-dot-0", "job-dot-1", "job-dot-2", "job-dot-many"];
const JOB_TOOL_DEBUG: [&str; 4] = ["job-tool-0", "job-tool-1", "job-tool-2", "job-tool-many"];
const JOB_COMMAND_DEBUG: [&str; 4] = [
    "job-command-0",
    "job-command-1",
    "job-command-2",
    "job-command-many",
];
const JOB_STATUS_DEBUG: [&str; 4] = [
    "job-status-0",
    "job-status-1",
    "job-status-2",
    "job-status-many",
];
const JOB_TIME_DEBUG: [&str; 4] = ["job-time-0", "job-time-1", "job-time-2", "job-time-many"];
const JOB_STOP_DEBUG: [&str; 4] = ["job-stop-0", "job-stop-1", "job-stop-2", "job-stop-many"];
const JOB_CHEVRON_DEBUG: [&str; 4] = [
    "job-chevron-0",
    "job-chevron-1",
    "job-chevron-2",
    "job-chevron-many",
];
const JOB_OUTPUT_DEBUG: [&str; 4] = [
    "job-output-0",
    "job-output-1",
    "job-output-2",
    "job-output-many",
];

/// Static debug selectors for the first subagent rows. The popover is a kit
/// layer, but an ElementItem is an ordinary element, so its wrapper is
/// readable; the names are indexed because every row used to share one.
/// The popover item content width: the measured 336px menu plus its own
/// padding stays inside the 400px ceiling, and a nowrap summary cannot widen it.
const SUBAGENT_ROW_W: f32 = 360.0;

const SUBAGENT_ROW_DEBUG: [&str; 3] = ["subagent-row-0", "subagent-row-1", "subagent-row-many"];
const SUBAGENT_SUMMARY_DEBUG: [&str; 3] = [
    "subagent-summary-0",
    "subagent-summary-1",
    "subagent-summary-many",
];

/// The debug selector for one row, clamped to the measured set.
fn job_debug(selectors: [&'static str; 4], index: usize) -> &'static str {
    selectors[index.min(selectors.len() - 1)]
}

/// The measured live row line: `.rowLine{align-items:stretch;gap:6px}` and
/// `.rowLineLive{border-radius:radius-md;background:fill-l2;
/// padding:4px 8px 4px 2px}`, whose own hover is a 9% label-primary mix - and
/// `.rowLineLive .row:hover{background:0 0}`, so the row inside it does not
/// hover on its own.
#[allow(dead_code)]
const JOB_LINE_GAP: f32 = 6.0;
#[allow(dead_code)]
const JOB_LINE_PAD_T: f32 = 4.0;
#[allow(dead_code)]
const JOB_LINE_PAD_R: f32 = 8.0;
#[allow(dead_code)]
const JOB_LINE_PAD_B: f32 = 4.0;
#[allow(dead_code)]
const JOB_LINE_PAD_L: f32 = 2.0;
/// `.row{min-height:28px;border-radius:radius-sm;padding:4px 7px;font-size:12px;
/// line-height:17px}` in label-primary.
const JOB_ROW_MIN_H: f32 = 28.0;
const JOB_ROW_PAD_X: f32 = 7.0;
const JOB_ROW_PAD_Y: f32 = 4.0;
/// `.stop{width:20px;height:20px;border:.5px solid border-l2;
/// border-radius:radius-sm;color:label-secondary}` holding a 10px stop mark,
/// and `.stopArmed{width:auto;gap:4px;padding:0 7px;color:state-error-primary}`
/// with `.stopLabel{font-size:11px;line-height:18px}`.
const JOB_STOP_SIDE: f32 = 20.0;
const JOB_STOP_ARMED_PAD: f32 = 7.0;
const JOB_STOP_ARMED_GAP: f32 = 4.0;
const JOB_STOP_MARK: f32 = 10.0;
const JOB_STOP_MARK_RADIUS: f32 = 3.0;
const JOB_STOP_LABEL: f32 = 11.0;
/// `.duration{font-size:11px;line-height:18px;
/// font-variant-numeric:tabular-nums}` in label-tertiary.
const JOB_DURATION: f32 = 11.0;
/// `KILL_ARM_MS = 3e3` and `KILL_FAILED_MS = 4e3`: an armed button waits this
/// long for its confirming press before disarming, and a failed stop keeps its
/// hint this long before the button resets.
pub(crate) const JOB_KILL_ARM: std::time::Duration = std::time::Duration::from_millis(3000);
pub(crate) const JOB_KILL_FAILED: std::time::Duration = std::time::Duration::from_millis(4000);

/// Which phase a job's stop button is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum JobPhase {
    /// The first press: the button now waits for a confirming press.
    Armed,
    /// A rejected stop: the button keeps its hint until the window closes.
    Failed,
}

/// The reference's kill arming state: one phase at a time, keyed by job id.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct JobArm {
    /// The job the press belongs to.
    pub(crate) id: String,
    /// The phase the button is in.
    pub(crate) phase: JobPhase,
    /// When the phase began, for the measured window.
    pub(crate) at: std::time::Instant,
}

impl JobArm {
    /// Whether the phase has outlived its measured window.
    pub(crate) fn expired(&self, at: std::time::Instant) -> bool {
        let window = match self.phase {
            JobPhase::Armed => JOB_KILL_ARM,
            JobPhase::Failed => JOB_KILL_FAILED,
        };
        at.saturating_duration_since(self.at) >= window
    }
}

/// The reference's `formatDuration`: elapsed time in at most two adjacent units,
/// with hours as the widest unit - past an hour the figure keeps counting hours
/// rather than growing a day vocabulary no producer reaches.
pub(crate) fn job_duration(ms: u64, lang: crate::i18n::Lang) -> String {
    let total = ms / 1000;
    let seconds = total % 60;
    let minutes = (total / 60) % 60;
    let hours = total / 3600;
    if hours > 0 {
        lang.tr("jobs.durationHours")
            .replace("{hours}", &hours.to_string())
            .replace("{minutes}", &minutes.to_string())
    } else if minutes > 0 {
        lang.tr("jobs.durationMinutes")
            .replace("{minutes}", &minutes.to_string())
            .replace("{seconds}", &seconds.to_string())
    } else {
        lang.tr("jobs.durationSeconds")
            .replace("{seconds}", &seconds.to_string())
    }
}

/// The reference's own count wording and its plural rule:
/// `t(live ? (count === 1 ? "count.live.one" : "count.live.other") : ...)`,
/// whose English is "1 background job running" / "{count} background jobs
/// running" and whose Chinese is "{count} 个后台任务运行中" for both forms. bsh
/// keeps no settled jobs, so the idle forms have nothing to count yet.
fn prompt_snippet(prompt: &str) -> String {
    prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .chars()
        .take(96)
        .collect()
}

/// The reference's own count wording and plural rule, for both the live and
/// the settled roster: "1 background job running" / "{count} background jobs
/// running" while something runs, and "1 background job" / "{count} background
/// jobs" once everything has settled.
pub(crate) fn job_count_label(lang: crate::i18n::Lang, count: usize, live: bool) -> String {
    let key = match (live, count == 1) {
        (true, true) => "jobs.countLiveOne",
        (true, false) => "jobs.countLiveOther",
        (false, true) => "jobs.countSettledOne",
        (false, false) => "jobs.countSettledOther",
    };
    lang.tr(key).replace("{count}", &count.to_string())
}

/// The row's right-hand status word: the exit code for a settled command, the
/// live word while it runs, and the killed word after a stop.
pub(crate) fn job_status_label(status: agent::jobs::JobStatus, lang: crate::i18n::Lang) -> String {
    match status {
        agent::jobs::JobStatus::Running => lang.tr("jobs.statusRunning"),
        agent::jobs::JobStatus::Succeeded { code } | agent::jobs::JobStatus::Failed { code } => {
            lang.tr("jobs.exitCode")
                .replace("{code}", &code.to_string())
        }
        agent::jobs::JobStatus::Killed => lang.tr("jobs.statusKilled"),
    }
}

/// The GoalBar label key for one durable phase and activation.
///
/// The reference draws nothing for a completed goal; an active goal reads
/// differently when its process-local continuation is armed or not, and a
/// blocked goal is still drawn (with Edit and Clear; its bar offers no Resume,
/// although the /goal command can resume it).
pub(crate) fn goal_bar_label_key(goal: &crate::model::Goal) -> Option<&'static str> {
    use agent::goal::GoalState;
    match goal.state {
        GoalState::Completed => None,
        GoalState::Paused => Some("goal.barPaused"),
        GoalState::Blocked => Some("goal.barBlocked"),
        GoalState::Active if goal.activation.is_armed() => Some("goal.barActive"),
        GoalState::Active => Some("goal.barDisarmed"),
    }
}

impl Shell {
    /// Header, active tab body, composer and status line.
    pub(crate) fn render_main_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Measured: the reference draws its hero inside the composer stack
        // while the session is blank, centres that stack in the scroll body,
        // and leaves the transcript itself empty.
        let subagent = self.state.subagent_view_run();
        let hero = self.state.hero_phase();
        // The selected child is a transient Session, so Chat and Trajectory use
        // exactly the renderers the main conversation uses.
        let body = match self.state.tab {
            Tab::Chat => self.render_transcript(cx).into_any_element(),
            Tab::Trajectory => self.render_trajectory(cx).into_any_element(),
            Tab::Audit => self.render_audit(cx).into_any_element(),
        };
        let goal = match self.state.tab {
            Tab::Chat => self.goal_row(cx),
            // The trajectory pane already shows the goal as a card, and the
            // audit is about tool calls rather than the plan.
            Tab::Trajectory | Tab::Audit => None,
        };
        let column = div()
            .debug_selector(|| String::from("main-column"))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        // The reference hangs the shell's only structural rule on the centre
        // column - `.5px` at border-l3 - and drops it when the sidebar folds away.
        let column = if self.sidebar_folded() {
            column
        } else {
            hairline_left(column).border_color(rgb(theme::border_soft()))
        };
        column
            .child(self.render_header(cx))
            // The reference centres the chat in a fixed-width column; the header and
            // the tab strip stay full width, the way its own layout does.
            .child({
                let middle = div()
                    .debug_selector(|| "chat-body".to_string())
                    // The reference parks its width handles in a positioned
                    // .body, and reports the pointer there while one is dragged.
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        this.clear_chat_hover(cx);
                        this.drag_chat_width(event.position.x.as_f32(), cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.commit_chat_width(cx);
                        }),
                    )
                    .child(self.chat_handle(HandleSide::Left, cx))
                    .child(self.chat_handle(HandleSide::Right, cx));
                if hero {
                    middle.justify_center().child(self.hero_group(cx))
                } else {
                    // The transcript is not centred: its right edge keeps the
                    // band the turn rail and the composer were laid out with,
                    // and its left edge takes only CHAT_LEFT_SHARE of the band,
                    // so the text starts where the eye expects it instead of
                    // behind a wide empty margin.
                    let band = ((self.conversation_w.as_f32() - self.chat_w()) / 2.0).max(0.0);
                    middle
                        .child(
                            div()
                                .w_full()
                                .pl(px(self.chat_left_inset()))
                                .pr(px(band))
                                .min_h_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .child(body)
                                .when_some(goal, |column, goal| column.child(goal)),
                        )
                        // Measured: the composer card is the chat content width + 32.
                        .child(
                            div()
                                .debug_selector(|| "composer-stack".to_string())
                                .w(px(self.composer_w()))
                                .flex_none()
                                .flex()
                                .flex_col()
                                // Measured: the to-do panel is a child of the
                                // composer stack, 64px narrower than the card
                                // and centred over it, with the stack's own gap.
                                .gap(px(theme::COMPOSER_STACK_GAP))
                                .when(subagent.is_none(), |stack| {
                                    stack
                                        .when_some(self.todo_panel(cx), |stack, panel| {
                                            stack.child(
                                                div()
                                                    .w_full()
                                                    .flex()
                                                    .flex_row()
                                                    .justify_center()
                                                    .child(panel),
                                            )
                                        })
                                        // Measured: the queue dock is the stack's next
                                        // child, and its own negative bottom margin
                                        // tucks it under the card's top edge.
                                        .when_some(self.queue_dock(cx), |stack, dock| {
                                            stack.child(
                                                div()
                                                    .w_full()
                                                    .flex()
                                                    .flex_row()
                                                    .justify_center()
                                                    .mb(px(theme::QUEUE_BOTTOM_MARGIN))
                                                    .child(dock),
                                            )
                                        })
                                })
                                .child(self.render_composer(cx)),
                        )
                }
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    // No leftward shift: it existed to align a SEPARATE meter row
                    // with the pills. The meter now leads the row, so the shift
                    // pushed it under the sidebar's edge and clipped it.
                    .child(div().w(px(self.chat_w())).child(self.render_statusbar(cx))),
            )
    }

    /// Title row plus tab strip.
    ///
    /// Measured: the header is 76 tall over a `10px 28px 0 20px` padding with a
    /// `.5px` `border-l3` rule under it; the tab strip adds its own 10px margin.
    ///
    /// Folded, the row grows its left pad to the traffic-light zone's right
    /// edge: the window's lights are over the corner the sidebar gave up, so the
    /// two controls the folded state adds (Show left panel and Add Session) and
    /// the tabs start past them rather than underneath them. Open, the lights
    /// are over the sidebar and 20 is right.
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pad_left = if self.sidebar_folded() {
            theme::traffic_light_inset().right
        } else {
            20.0
        };
        let header = div()
            .debug_selector(|| "hdr-header".to_string())
            .flex()
            .flex_col()
            .min_h(px(76.0))
            .pt(px(10.0))
            .pr(px(28.0))
            .pl(px(pad_left))
            .child(self.title_row(cx))
            .child(self.tab_strip(cx));
        hairline_bottom(header).border_color(rgb(theme::border_soft()))
    }

    /// The session title and its status chips.
    fn title_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .state
            .subagent_view_run()
            .map(|run| run.title)
            .or_else(|| {
                self.state
                    .active_session()
                    .map(|session| session.title.clone())
            })
            .unwrap_or_default();
        let streaming = self.state.any_streaming();
        let lang = self.state.lang;
        let parent_title = self.state.subagent_parent_title().unwrap_or_default();
        div()
            .debug_selector(|| "hdr-title".to_string())
            // Measured `min-height: 30`, drawn as a fixed height: gpui's own
            // `min_h` rendered 2px above the CSS floor on this row (32 with a
            // 20px text child), while the reference renders 30.
            .h(px(30.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            // The reference's folded state: with the sidebar away, the header's
            // left edge carries the two controls the sidebar used to hold - the
            // panel toggle that brings it back, and New Session - ahead of the
            // title. When the sidebar is up, both live in it and the row starts
            // at the title.
            .when(self.sidebar_folded(), |row| {
                row.child(
                    div()
                        .id("hdr-show-sidebar")
                        .debug_selector(|| String::from("hdr-show-sidebar"))
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::PanelLeft, 15.0, theme::muted()))
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.toggle_sidebar_at(window.viewport_size().width.as_f32(), cx);
                        })),
                )
                .child(
                    div()
                        .id("hdr-new-session")
                        .debug_selector(|| String::from("hdr-new-session"))
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::Plus, 15.0, theme::muted()))
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.new_chat(window, cx);
                        })),
                )
            })
            .when(self.state.subagent.is_some(), |row| {
                row.child(
                    div()
                        .id("subagent-back")
                        .debug_selector(|| "subagent-back".to_string())
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w(px(0.0))
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.8))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.state.leave_subagent();
                            cx.notify();
                        }))
                        .child(
                            txt(parent_title, 13.0, theme::muted())
                                .truncate()
                                .max_w(px(220.0)),
                        )
                        .child(txt("/", 13.0, theme::faint())),
                )
            })
            .child(
                // Measured: the current crumb is 14px/20px at weight 500, label-primary.
                txt(title, theme::BODY_SIZE, theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(20.0))
                    .truncate(),
            )
            // DSH's header names the agent preset after the title, before the
            // background-jobs chip. The hero phase draws the same chip in its
            // own workspace row, so this one is absent there rather than
            // duplicating it.
            .when(!self.state.hero_phase(), |row| {
                row.child(self.agent_preset_chip(lang, cx))
            })
            .when(streaming, |row| {
                row.child(
                    div()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::accent()))
                        .child(txt(
                            lang.tr("status.running"),
                            theme::CHIP_SIZE,
                            theme::page(),
                        )),
                )
            })
            .children(self.subagent_chip(cx))
            .child(div().flex_1())
            // The reference's jobs chip lives in the session header's action
            // cluster, between the title and the right-hand controls, which is
            // why its popover hangs right-aligned under the chip.
            .children(self.job_chip(cx))
            .when_some(self.state.active_id.clone(), |row, id| {
                let pinned = self
                    .state
                    .store
                    .session(&id)
                    .map(|session| session.pinned)
                    .unwrap_or(false);
                let archived = self.state.is_archived(&id);
                let streaming = self.state.is_streaming(&id);
                let lang = self.state.lang;
                let more_shell = cx.entity();
                // The reference names this control More actions and drives it from the
                // same session actions the sidebar row offers - one dispatcher, two doors.
                row.child(
                    Button::new("hdr-more")
                        .ghost()
                        .compact()
                        .icon(IconName::Ellipsis)
                        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                            let mut menu = menu;
                            for item in crate::ui::session_menu::session_items(
                                &id,
                                pinned,
                                archived,
                                streaming,
                                lang,
                                &more_shell,
                            ) {
                                menu = menu.item(item);
                            }
                            menu
                        }),
                )
            })
            // The reference header carries Open in <editor> and More ways to open
            // ahead of the panel toggle; the editor is whichever known one is
            // actually installed.
            .children(self.open_controls(cx))
            // The reference header ends with the right panel's toggle. The left
            // column's controls live elsewhere: the sidebar's own strip carries
            // its fold control while it is up, and the folded state carries
            // Show-left-panel and New Session at the title row's left edge.
            .child(self.header_button(
                "hdr-show-right-panel",
                icon(IconName::PanelRight, 15.0, theme::muted()),
                cx,
            ))
    }
    /// Open the active workspace in an installed editor, or reveal it.
    ///
    /// The reference header carries Open in Zed and More ways to open; which
    /// editors exist is a property of the machine, so the primary control names the
    /// first installed one and the menu offers the rest plus the file browser.
    /// The background-jobs chip label, or None when nothing runs in the
    /// background. It is split out from the chip because a kit Button records no
    /// bounds the harness could look up - not even through a wrapper - so the
    /// wording and the count have to be assertable on their own.
    pub(crate) fn job_label(&self) -> Option<String> {
        let (running, settled) = self.jobs_counts;
        if running > 0 {
            return Some(job_count_label(self.state.lang, running, true));
        }
        if settled > 0 {
            return Some(job_count_label(self.state.lang, settled, false));
        }
        None
    }

    /// The reference's background-jobs trigger, which is not a pill: measured
    /// `.trigger{border-radius:radius-sm;min-height:28px;color:label-tertiary;
    /// cursor:pointer;background:0 0;border:0;align-items:center;gap:3px;
    /// padding:3px 2px;font-size:12px;line-height:18px;display:inline-flex}`,
    /// drawing `[ongoing StateDot?] [span.count{margin:0 5px}]
    /// [IconChevronDownOutlineRegular 12]` and opening a 500-wide panel whose
    /// first row is the measured `.sectionHeader` "Running". Absent while
    /// nothing runs in the background, because a trigger reading zero would be
    /// noise - and the reference draws the ongoing dot only while a row lives.
    /// The measured width handle (round 115): a 10px col-resize strip parked
    /// 24px outside its edge of the content column, carrying the measured 2px
    /// bar 4px in from that edge, and setting the column's stored width when
    /// dragged. The reference fades that bar in around the pointer and hides
    /// both handles behind a composer overlay; bsh shows it while dragging.
    fn chat_handle(&self, side: HandleSide, cx: &mut Context<Self>) -> impl IntoElement {
        let offset =
            self.conversation_w.as_f32() / 2.0 + self.chat_w() / 2.0 + crate::theme::HANDLE_OFFSET;
        // The measured bar, from the one shared definition every seam uses.
        let bar = super::handle_bar(
            match side {
                HandleSide::Left => "chat-handle-bar-left",
                HandleSide::Right => "chat-handle-bar-right",
            },
            if self.chat_drag.is_some() || self.chat_hover == Some(side) {
                1.0
            } else {
                0.0
            },
        );
        let id = match side {
            HandleSide::Left => "chat-handle-left",
            HandleSide::Right => "chat-handle-right",
        };
        let handle = div()
            .id(id)
            .debug_selector(move || id.to_string())
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .w(px(self.handle_w()))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.begin_chat_drag(side, event.position.x.as_f32(), cx);
                }),
            )
            // The reference reveals the bar on :hover; stopping the move here
            // is what keeps the body's own move from clearing it instantly. A
            // drag still updates, so a pointer that stays on the strip works.
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    this.drag_chat_width(event.position.x.as_f32(), cx);
                    this.hover_chat_handle(side, cx);
                    cx.stop_propagation();
                }),
            )
            .child(bar);
        match side {
            // The transcript is not centred, so each handle parks outside ITS
            // own edge: the left one follows the narrowed inset rather than the
            // symmetric offset the right one still uses.
            HandleSide::Left => handle.left(px((self.chat_left_inset()
                - crate::theme::HANDLE_OFFSET
                - self.handle_w())
            .max(0.0))),
            HandleSide::Right => handle.left(px(offset)),
        }
    }

    /// The selector wording: the selected child's title, or the collection
    /// count while the parent conversation is showing.
    pub(crate) fn subagent_chip_label(&self) -> Option<String> {
        let runs = self.state.subagent_runs();
        if runs.is_empty() {
            return None;
        }
        if let Some(current) = self.state.subagent_view_run() {
            return Some(current.title);
        }
        let count = runs.len();
        Some(if count == 1 {
            self.state.lang.tr("subagents.countOne")
        } else {
            self.state
                .lang
                .tr("subagents.countOther")
                .replace("{count}", &count.to_string())
        })
    }

    /// DSH's SubagentHeaderLineage: a lineage switcher whose trigger names the
    /// current run and whose 336px menu lists the child runs with a
    /// token/duration metrics column.
    fn subagent_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let runs = self.state.subagent_runs();
        if runs.is_empty() {
            return None;
        }
        let lang = self.state.lang;
        let label = self.subagent_chip_label()?;
        let running = runs.iter().filter(|run| run.status.is_running()).count();
        let shell = cx.entity();
        let button = Button::new("hdr-subagents")
            .ghost()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(2.0))
            .py(px(3.0))
            .h(px(28.0))
            .rounded(px(8.0))
            .child(
                div()
                    .debug_selector(|| "hdr-subagents-dot".to_string())
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if running > 0 {
                        spinner().into_any_element()
                    } else {
                        div()
                            .h(px(6.0))
                            .w(px(6.0))
                            .rounded_full()
                            .bg(rgb(theme::success()))
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .debug_selector(|| "hdr-subagents-count".to_string())
                    .max_w(px(244.0))
                    .min_w(px(0.0))
                    .child(
                        txt(label, 12.0, theme::text())
                            .font_weight(FontWeight::MEDIUM)
                            .truncate(),
                    ),
            )
            .child(
                icon(IconName::ChevronDown, 12.0, theme::faint())
                    .debug_selector(|| "hdr-subagents-chevron".to_string()),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, cx| {
                let mut menu = menu.min_w(px(336.0)).max_w(px(400.0)).max_h(px(560.0));
                let shell = shell.clone();
                let current_id = shell
                    .read(cx)
                    .state
                    .subagent
                    .as_ref()
                    .map(|subagent| subagent.id.clone());
                let runs = shell.read(cx).state.subagent_runs();
                for (index, run) in runs.into_iter().enumerate() {
                    let id = run.id.clone();
                    let title = run.title.clone();
                    let prompt = run.prompt.clone();
                    let parent_id = run.parent.clone();
                    let status = run.status;
                    let tokens = run.total_tokens();
                    let estimated_tokens = run.estimated_tokens;
                    let elapsed = run.elapsed_ms();
                    let is_current = current_id.as_deref() == Some(id.as_str());
                    let row_debug = SUBAGENT_ROW_DEBUG[index.min(SUBAGENT_ROW_DEBUG.len() - 1)];
                    let summary_debug =
                        SUBAGENT_SUMMARY_DEBUG[index.min(SUBAGENT_SUMMARY_DEBUG.len() - 1)];
                    let open_shell = shell.clone();
                    let open_id = id.clone();
                    menu = menu.item(PopupMenuItem::element(move |_, _| {
                        let open_shell = open_shell.clone();
                        let open_id = open_id.clone();
                        let row_title = title.clone();
                        let summary = prompt_snippet(&prompt);
                        let time = job_duration(elapsed, lang);
                        let token_text = if tokens == 0 {
                            format!("~{} tok", crate::state::human_tokens(estimated_tokens))
                        } else {
                            format!("{} tok", crate::state::human_tokens(tokens))
                        };
                        let activity = match status {
                            SubagentStatus::Running => spinner().into_any_element(),
                            SubagentStatus::Done => div()
                                .h(px(6.0))
                                .w(px(6.0))
                                .rounded_full()
                                .bg(rgb(theme::success()))
                                .into_any_element(),
                            SubagentStatus::Failed => div()
                                .h(px(6.0))
                                .w(px(6.0))
                                .rounded_full()
                                .bg(rgb(theme::error()))
                                .into_any_element(),
                        };
                        let row = div()
                            .id(format!("subagent-row-{id}"))
                            .flex()
                            .items_start()
                            .gap(px(6.0))
                            .min_h(px(44.0))
                            .w_full()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .px(px(9.0))
                            .py(px(6.0))
                            .rounded(px(12.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(theme::hover())))
                            .tooltip({
                                let tip = format!("{prompt}\nparent: {parent_id}");
                                move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
                            })
                            .on_click(move |_, _window, cx| {
                                open_shell.update(cx, |this, cx| {
                                    this.state.show_subagent(&open_id);
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .w(px(14.0))
                                    .h(px(17.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(activity),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .gap(px(1.0))
                                    .child({
                                        let row_label =
                                            txt(row_title, 12.0, theme::text()).truncate();
                                        if is_current {
                                            row_label.font_weight(FontWeight::SEMIBOLD)
                                        } else {
                                            row_label
                                        }
                                    })
                                    .child(
                                        txt(summary, 10.0, theme::faint())
                                            .truncate()
                                            .debug_selector(move || summary_debug.to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_end()
                                    .flex_none()
                                    .max_w(px(96.0))
                                    .gap(px(1.0))
                                    .child(if token_text.is_empty() {
                                        div().into_any_element()
                                    } else {
                                        txt(token_text, 10.0, theme::faint())
                                            .truncate()
                                            .into_any_element()
                                    })
                                    .child(txt(time, 10.0, theme::faint()).truncate()),
                            )
                            .into_any_element();
                        // A fixed content width is what keeps the kit item's
                        // min-content from widening the popover: the summary is
                        // nowrap, and a wrapper that only says w_full inherits
                        // its full min-content width inside the popup's
                        // shrink-to-fit item row.
                        div()
                            .debug_selector(move || row_debug.to_string())
                            .w(px(SUBAGENT_ROW_W))
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .child(row)
                            .into_any_element()
                    }));
                }
                menu
            });
        Some(
            div()
                .debug_selector(|| "hdr-subagents".to_string())
                .flex()
                .items_center()
                .child(button)
                .into_any_element(),
        )
    }

    /// The reference's background-jobs trigger, which is not a pill: measured
    /// .trigger{border-radius:radius-sm;min-height:28px;color:label-tertiary;
    /// cursor:pointer;background:0 0;border:0;align-items:center;gap:3px;
    /// padding:3px 2px;font-size:12px;line-height:18px;display:inline-flex},
    /// drawing [ongoing StateDot?] [span.count{margin:0 5px}]
    /// [IconChevronDownOutlineRegular 12]. Absent while the session has no job
    /// at all, because a trigger reading zero would be noise; the ongoing dot is
    /// drawn only while a row is running. The count takes the reference's own
    /// plural rule and switches to the settled wording once nothing runs.
    fn job_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let label = self.job_label()?;
        let running = self.state.has_running_jobs();
        let open = self.jobs_open;
        let mut chip = div()
            .id("hdr-jobs")
            .debug_selector(|| "hdr-jobs".to_string())
            .flex()
            .items_center()
            .gap(px(3.0))
            .px(px(2.0))
            .py(px(3.0))
            // The kit Button owns its own height, so the measured 28 is set
            // rather than floored.
            .h(px(28.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::hover())))
            // The frame's press handler closes the popover; the trigger owns its
            // own press so its click can toggle instead.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.jobs_open = !this.jobs_open;
                cx.notify();
            }));
        if running {
            chip = chip.child(
                div()
                    .debug_selector(|| "hdr-jobs-dot".to_string())
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(spinner()),
            );
        }
        chip = chip
            .child(
                div()
                    .debug_selector(|| "hdr-jobs-count".to_string())
                    .mx(px(5.0))
                    .child(txt(label, 12.0, theme::faint()).line_height(px(18.0))),
            )
            .child(
                icon(
                    if open {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    },
                    12.0,
                    theme::faint(),
                )
                .debug_selector(|| "hdr-jobs-chevron".to_string()),
            );
        Some(chip.into_any_element())
    }

    /// The reference's jobs popover: a 500-wide panel .menu{width:500px;
    /// max-width:min(560px,100vw - 32px);max-height:min(480px,100vh - 140px);
    /// border-radius:radius-lg;background:specific-menu;gap:1px;padding:3px;
    /// top:calc(100% + 5px)}, right-aligned under the chip. It is painted by
    /// Shell::render after every column rather than as a kit popup layer, so its
    /// expanded rows are ordinary elements and a press can toggle one without
    /// rebuilding the menu.
    ///
    /// Sections: Running (only while something runs) above Finished, each with
    /// the measured 11px/16px header; Clear is the Finished header's right-hand
    /// action and drops only settled jobs.
    pub(crate) fn render_jobs_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.jobs_open {
            return None;
        }
        let jobs = self.state.background_jobs();
        let lang = self.state.lang;
        let running: Vec<agent::jobs::JobSummary> = jobs
            .iter()
            .filter(|job| job.is_running())
            .cloned()
            .collect();
        let settled: Vec<agent::jobs::JobSummary> = jobs
            .iter()
            .filter(|job| !job.is_running())
            .cloned()
            .collect();
        let mut body = div()
            .id("jobs-panel-body")
            .flex()
            .flex_col()
            .gap(px(1.0))
            .max_h(px(JOBS_MENU_MAX_H - 6.0))
            .overflow_y_scroll();
        if jobs.is_empty() {
            body = body.child(
                div()
                    .debug_selector(|| "jobs-empty".to_string())
                    .px(px(8.0))
                    .py(px(12.0))
                    .child(txt(lang.tr("jobs.empty"), 12.0, theme::faint())),
            );
        }
        let mut index = 0usize;
        if !running.is_empty() {
            body = body.child(self.job_section_header(
                lang,
                "jobs-running",
                lang.tr("jobs.sectionLive"),
                running.len(),
                false,
                true,
                cx,
            ));
            for job in &running {
                body = body.child(self.job_row(job, index, lang, cx));
                index += 1;
            }
        }
        if !settled.is_empty() {
            body = body.child(self.job_section_header(
                lang,
                "jobs-settled",
                lang.tr("jobs.sectionSettled"),
                settled.len(),
                true,
                running.is_empty(),
                cx,
            ));
            for job in &settled {
                body = body.child(self.job_row(job, index, lang, cx));
                index += 1;
            }
        }
        // The main column's right edge: the header's own 28px pad, plus the
        // right panel when it is open. The chip sits in the right-hand action
        // cluster, so this is the reference's right-aligned panel.
        let right = 28.0
            + if self.right.open {
                self.right_panel_w()
            } else {
                0.0
            };
        // The measured max-width is min(560px, 100vw - 32px); the 500 default
        // gives way on a window narrower than that, so the panel never runs off
        // the left edge.
        let width = JOBS_MENU_W.min((self.viewport_w - 32.0).max(240.0));
        let panel = div()
            .id("jobs-panel")
            .debug_selector(|| "jobs-panel".to_string())
            .absolute()
            .top(px(45.0))
            .right(px(right))
            .w(px(width))
            .max_h(px(JOBS_MENU_MAX_H))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .p(px(3.0))
            .rounded(px(16.0))
            .bg(rgb(theme::page()))
            .shadow_lg()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(body);
        Some(
            hairline_all(panel)
                .border_color(rgb(theme::border_soft()))
                .into_any_element(),
        )
    }

    /// One measured .sectionHeader: a 11px/16px label and its count on the
    /// left, the Clear text action on the right, a .5px border-l1 rule above
    /// every section but the first (margin 4px 4px 1px, padding 4px 0 3px).
    #[allow(clippy::too_many_arguments)]
    fn job_section_header(
        &self,
        lang: crate::i18n::Lang,
        id: &'static str,
        label: String,
        count: usize,
        clear: bool,
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count_id = if id == "jobs-running" {
            "jobs-running-count"
        } else {
            "jobs-settled-count"
        };
        let mut header =
            div()
                .debug_selector(move || format!("{id}-header"))
                .flex()
                .items_center()
                .justify_between()
                .mx(px(4.0))
                .mt(px(4.0))
                .mb(px(1.0))
                .pt(px(4.0))
                .pb(px(3.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(txt(label, 11.0, theme::faint()).line_height(px(16.0)))
                        .child(div().debug_selector(move || count_id.to_string()).child(
                            txt(count.to_string(), 11.0, theme::faint()).line_height(px(16.0)),
                        )),
                );
        if clear {
            header = header.child(
                div()
                    .id("jobs-clear")
                    .debug_selector(|| "jobs-clear".to_string())
                    .rounded(px(8.0))
                    .px(px(4.0))
                    .py(px(2.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .child(txt(lang.tr("jobs.clear"), 11.0, theme::faint()).line_height(px(16.0)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if let Some(id) = this.state.active_id.clone() {
                            this.state.jobs.clear_settled(&id);
                        }
                        this.jobs_expanded.clear();
                        this.jobs_output_cache.clear();
                        cx.notify();
                    })),
            );
        }
        if first {
            header.into_any_element()
        } else {
            hairline_top(header)
                .border_color(rgb(theme::border_soft()))
                .into_any_element()
        }
    }

    /// One job row: a 6px status dot, the tool name in mono, the one-line
    /// ellipsised command, the right-aligned status word and duration, an
    /// optional stop control while it runs, and the chevron that expands the
    /// captured output inline.
    fn job_row(
        &self,
        job: &agent::jobs::JobSummary,
        index: usize,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = job.id.clone();
        let expanded = self.jobs_expanded.contains(&id);
        let armed = self
            .jobs_arm
            .as_ref()
            .filter(|arm| arm.id == id)
            .map(|arm| arm.phase);
        let dot = match job.status {
            agent::jobs::JobStatus::Running => theme::accent(),
            agent::jobs::JobStatus::Succeeded { .. } => theme::success(),
            agent::jobs::JobStatus::Failed { .. } => theme::error(),
            agent::jobs::JobStatus::Killed => theme::warn(),
        };
        let status = job_status_label(job.status, lang);
        let duration = job_duration(job.duration_ms, lang);
        let row_debug = job_debug(JOB_ROW_DEBUG, index);
        let dot_debug = job_debug(JOB_DOT_DEBUG, index);
        let tool_debug = job_debug(JOB_TOOL_DEBUG, index);
        let command_debug = job_debug(JOB_COMMAND_DEBUG, index);
        let status_debug = job_debug(JOB_STATUS_DEBUG, index);
        let time_debug = job_debug(JOB_TIME_DEBUG, index);
        let chevron_debug = job_debug(JOB_CHEVRON_DEBUG, index);
        let mut line = div()
            .id(format!("job-row-{id}"))
            .debug_selector(move || row_debug.to_string())
            .flex()
            .items_center()
            .gap(px(6.0))
            .w_full()
            .min_h(px(JOB_ROW_MIN_H))
            .px(px(JOB_ROW_PAD_X))
            .py(px(JOB_ROW_PAD_Y))
            .rounded(px(8.0))
            .hover(|style| style.bg(rgb(theme::hover())))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .debug_selector(move || dot_debug.to_string())
                    .flex_none()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded_full()
                    .bg(rgb(dot)),
            )
            .child(
                div()
                    .debug_selector(move || tool_debug.to_string())
                    .flex_none()
                    .font_family(crate::theme::CODE_FONT)
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(rgb(theme::faint()))
                    .child(job.tool.clone()),
            )
            .child(
                div()
                    .debug_selector(move || command_debug.to_string())
                    .flex_1()
                    .min_w(px(0.0))
                    .font_family(crate::theme::CODE_FONT)
                    .text_size(px(12.0))
                    .line_height(px(17.0))
                    .text_color(rgb(if job.is_running() {
                        theme::text()
                    } else {
                        theme::faint()
                    }))
                    .text_ellipsis()
                    .child(job.command.clone()),
            )
            .child(
                div()
                    .debug_selector(move || status_debug.to_string())
                    .flex_none()
                    .text_size(px(10.0))
                    .line_height(px(16.0))
                    .text_color(rgb(theme::faint()))
                    .child(status),
            )
            .child(
                div()
                    .debug_selector(move || time_debug.to_string())
                    .flex_none()
                    .text_size(px(JOB_DURATION))
                    .line_height(px(18.0))
                    .text_color(rgb(theme::faint()))
                    .child(duration),
            );
        if job.is_running() {
            line = line.child(self.job_stop(&id, index, &job.command, armed, lang, cx));
        }
        let toggle_shell = id.clone();
        line = line.child(
            div()
                .id(format!("job-chevron-{id}"))
                .debug_selector(move || chevron_debug.to_string())
                .flex_none()
                .w(px(20.0))
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::hover())))
                .child(icon(
                    if expanded {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    },
                    12.0,
                    theme::faint(),
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    if !this.jobs_expanded.remove(&toggle_shell) {
                        this.jobs_expanded.insert(toggle_shell.clone());
                    }
                    cx.notify();
                })),
        );
        let mut item = div().flex().flex_col().w_full().child(line);
        if expanded {
            let (text, truncated) = self
                .job_output_cached(&id)
                .map(|(text, truncated)| (text.to_string(), truncated))
                .unwrap_or_default();
            let shown = if text.trim().is_empty() {
                lang.tr("jobs.outputEmpty")
            } else {
                text
            };
            let output_debug = job_debug(JOB_OUTPUT_DEBUG, index);
            let mut output = div()
                .id(format!("job-output-{id}"))
                .debug_selector(move || output_debug.to_string())
                .mx(px(8.0))
                .mb(px(8.0))
                .p(px(6.0))
                .max_h(px(288.0))
                .rounded(px(8.0))
                .bg(rgb(theme::new_session_fill()))
                .overflow_y_scroll()
                .font_family(crate::theme::CODE_FONT)
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(rgb(theme::muted()))
                .child(shown);
            if truncated {
                output = output.child(div().mt(px(4.0)).child(txt(
                    lang.tr("jobs.outputTruncated"),
                    10.0,
                    theme::faint(),
                )));
            }
            item = item.child(output);
        }
        item.into_any_element()
    }

    /// The measured live stop control: a 20x20 .5px border-l2 box at radius-sm
    /// holding a 10px mark, which arms on the first press and grows to
    /// width:auto;gap:4px;padding:0 7px with an 11px/18px label. The second
    /// press is the kill; a job that already settled shows the measured failed
    /// phase for its window.
    fn job_stop(
        &self,
        id: &str,
        index: usize,
        command: &str,
        phase: Option<JobPhase>,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let armed = phase == Some(JobPhase::Armed);
        let failed = phase == Some(JobPhase::Failed);
        let colour = if armed || failed {
            theme::error()
        } else {
            theme::muted()
        };
        let tip = lang
            .tr(if armed {
                "jobs.killConfirm"
            } else if failed {
                "jobs.killFailed"
            } else {
                "jobs.killStop"
            })
            .replace("{label}", command);
        let stop_id = id.to_string();
        let stop_debug = job_debug(JOB_STOP_DEBUG, index);
        let mut stop = div()
            .id(format!("job-stop-{id}"))
            .debug_selector(move || stop_debug.to_string())
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .h(px(JOB_STOP_SIDE))
            .rounded(px(8.0))
            .cursor_pointer()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .h(px(JOB_STOP_MARK))
                    .w(px(JOB_STOP_MARK))
                    .rounded(px(JOB_STOP_MARK_RADIUS))
                    .bg(rgb(colour)),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.press_job_kill_at(&stop_id, std::time::Instant::now(), cx);
            }));
        if armed {
            stop = hairline_all(stop)
                .border_color(rgb(theme::error()).alpha(0.45))
                .bg(rgb(theme::error()).alpha(0.12))
                .gap(px(JOB_STOP_ARMED_GAP))
                .px(px(JOB_STOP_ARMED_PAD))
                .child(
                    txt(
                        lang.tr("jobs.killConfirmAction"),
                        JOB_STOP_LABEL,
                        theme::error(),
                    )
                    .line_height(px(18.0)),
                );
        } else if failed {
            stop = hairline_all(stop)
                .border_color(rgb(theme::error()).alpha(0.45))
                .w(px(JOB_STOP_SIDE));
        } else {
            stop = hairline_all(stop)
                .border_color(rgb(theme::border()))
                .w(px(JOB_STOP_SIDE))
                .hover(|style| style.border_color(rgb(theme::error()).alpha(0.45)));
        }
        stop.into_any_element()
    }

    fn open_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let root = self
            .state
            .settings
            .active_workspace()
            .map(|workspace| workspace.root.clone())
            .unwrap_or_default();
        if root.trim().is_empty() {
            return None;
        }
        let lang = self.state.lang;
        let editors = crate::editors::installed_editors();
        let primary = editors.first().copied();
        let label = match primary {
            Some(name) => lang.tr("hdr.openIn").replace("{editor}", name),
            None => lang.tr("hdr.revealFinder"),
        };
        let rest: Vec<&str> = editors.into_iter().skip(1).collect();
        let menu_shell = cx.entity();
        let menu_root = root.clone();
        let reveal = lang.tr("hdr.revealFinder");
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .debug_selector(|| "hdr-open".to_string())
                        .id("hdr-open")
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(rgb(theme::border()))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(theme::hover())))
                        .child(txt(label, theme::CHIP_SIZE, theme::muted()))
                        .on_click(cx.listener({
                            let root = root.clone();
                            move |this, _event, _window, cx| {
                                this.open_workspace(primary, &root, cx);
                            }
                        })),
                )
                .child(
                    Button::new("hdr-open-more")
                        .ghost()
                        .compact()
                        .label(lang.tr("hdr.moreWays"))
                        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                            let mut menu = menu;
                            for name in rest.iter().copied() {
                                let shell = menu_shell.clone();
                                let path = menu_root.clone();
                                menu = menu.item(PopupMenuItem::new(name).on_click(
                                    move |_, _, cx| {
                                        shell.update(cx, |this, cx| {
                                            this.open_workspace(Some(name), &path, cx);
                                        });
                                    },
                                ));
                            }
                            let shell = menu_shell.clone();
                            let path = menu_root.clone();
                            menu.item(PopupMenuItem::new(reveal.clone()).on_click(
                                move |_, _, cx| {
                                    shell.update(cx, |this, cx| {
                                        this.open_workspace(None, &path, cx);
                                    });
                                },
                            ))
                        }),
                )
                .into_any_element(),
        )
    }

    /// Open the active workspace, reporting a failure rather than doing nothing
    /// when no handler accepts the path.
    fn open_workspace(&mut self, editor: Option<&str>, path: &str, cx: &mut Context<Self>) {
        let lang = self.state.lang;
        if crate::editors::open_in(editor, path).is_err() {
            self.state.notice(lang.tr("hdr.openFailed"));
            cx.notify();
        }
    }

    /// The goal strip above the composer: the reference's GoalBar.
    ///
    /// Measured from dsh-client-ui-goal's GoalBar.module.css and JSX: a 36-tall
    /// row at radius 12, padded 4/5/4/12 at gap 10, holding a 14px goal glyph,
    /// a 13/24 weight-500 phase label, the ellipsised objective at 13/20, and
    /// 28px round icon actions. The label AND its actions depend on the durable
    /// phase and the process-local activation: active+armed is Ongoing with
    /// Pause; active+disarmed is Inactive with Resume; paused is Paused with
    /// Resume; blocked is Blocked with Edit and Clear only. A completed goal
    /// draws nothing.
    /// The blocked reason is the bar's tooltip (the reference's title attribute).
    fn goal_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use agent::goal::GoalState;
        let goal = self.state.active_session()?.goal.as_ref()?;
        let label_key = goal_bar_label_key(goal)?;
        let lang = self.state.lang;
        let armed = goal.activation.is_armed();
        let label = lang.tr(label_key);
        let action = |id: &'static str, glyph: IconName, tip: String| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(|style| style.bg(rgb(theme::hover())))
                .tooltip({
                    let tip = tip.clone();
                    move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
                })
                .child(icon(glyph, 14.0, theme::faint()))
        };
        let mut actions: Vec<AnyElement> = Vec::new();
        if goal.state == GoalState::Active && armed {
            actions.push(
                action("goal-pause", IconName::Pause, lang.tr("goal.pause"))
                    .on_click(cx.listener(|this, _event, _window, cx| this.pause_active_goal(cx)))
                    .into_any_element(),
            );
        }
        if goal.state == GoalState::Paused || (goal.state == GoalState::Active && !armed) {
            actions.push(
                action("goal-resume", IconName::Play, lang.tr("goal.resume"))
                    .on_click(cx.listener(|this, _event, _window, cx| this.resume_active_goal(cx)))
                    .into_any_element(),
            );
        }
        actions.push(
            action("goal-edit", IconName::Pencil, lang.tr("goal.edit"))
                .on_click(cx.listener(|this, _event, window, cx| {
                    this.open_note(crate::ui::slash_palette::NoteKind::Goal, window, cx);
                }))
                .into_any_element(),
        );
        actions.push(
            action("goal-clear", IconName::Trash, lang.tr("goal.clear"))
                .on_click(cx.listener(|this, _event, _window, cx| this.clear_active_goal(cx)))
                .into_any_element(),
        );
        let mut bar = div()
            .id("composer-goal")
            .debug_selector(|| String::from("composer-goal"))
            .mx(px(30.0))
            .mb(px(6.0))
            .h(px(36.0))
            .pl(px(12.0))
            .pr(px(5.0))
            .py(px(4.0))
            .rounded(px(12.0))
            .bg(rgb(theme::page()))
            .shadow_sm()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(icon(IconName::Target, 14.0, theme::faint()).flex_none())
            .child(
                txt(label, 13.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(24.0))
                    .flex_none(),
            )
            .child(
                txt(goal.objective.clone(), 13.0, theme::muted())
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .line_height(px(20.0)),
            );
        for element in actions {
            bar = bar.child(element);
        }
        if let Some(reason) = &goal.blocked_reason {
            let reason = reason.message.clone();
            bar = bar.tooltip(move |window, cx| Tooltip::new(reason.clone()).build(window, cx));
        }
        Some(bar.into_any_element())
    }

    /// Chat / Trajectory.
    fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let tab = self.state.tab;
        let entry = |id: &'static str, key: &'static str, this_tab: Tab, cx: &mut Context<Self>| {
            let selected = tab == this_tab;
            div()
                .debug_selector(|| id.to_string())
                .id(id)
                .pb(px(9.0))
                .border_b_2()
                .border_color(if selected {
                    rgb(theme::accent())
                } else {
                    rgb(0x00000000)
                })
                .cursor_pointer()
                .child(
                    txt(
                        lang.tr(key),
                        13.0,
                        if selected {
                            theme::accent()
                        } else {
                            theme::faint()
                        },
                    )
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(16.0)),
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    // The trace tab reads its rows from the Parquet log, so the
                    // first page is scheduled when the tab is opened - not on
                    // every frame, and not for a tab nobody is looking at.
                    this.state.tab = this_tab;
                    if this_tab == Tab::Trajectory && this.trace_pages == 0 {
                        this.request_trace_page(cx);
                    }
                    cx.notify();
                }))
        };
        div()
            .h(px(27.0))
            .mt(px(10.0))
            .pl(px(8.0))
            .flex()
            .flex_row()
            .items_end()
            .gap(px(36.0))
            .child(entry("tab-chat", "tab.chat", Tab::Chat, cx))
            // The reference renders the trajectory tab only while its own
            // `developerTools` preference (Show coding view) is on; measured
            // in `dsh-client-ui-conversation`, whose header does
            // `if (!developerTools) return null` for this entry.
            .when(self.state.draft().global.general.developer_tools, |strip| {
                strip.child(entry(
                    "tab-trajectory",
                    "tab.trajectory",
                    Tab::Trajectory,
                    cx,
                ))
            })
            // The audit is not behind Show coding view: a refusal log is a
            // safety surface, and hiding it behind a developer preference would
            // make it unreachable exactly when a reader needs it.
            .child(entry("tab-audit", "tab.audit", Tab::Audit, cx))
    }

    /// A small square icon button in the title row.
    fn header_button(
        &self,
        id: &'static str,
        mark: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            // Recorded only with debug assertions; it is how the interaction test
            // finds this control to click it.
            .debug_selector(|| id.to_string())
            .id(id)
            // Measured iconButton: 28x28 at radius 8.
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(mark)
            .on_click(cx.listener(move |this, _event, window, cx| match id {
                "hdr-show-sidebar" => {
                    this.toggle_sidebar_at(window.viewport_size().width.as_f32(), cx);
                }
                "hdr-show-right-panel" => {
                    this.toggle_right_panel(cx);
                }
                _ => {
                    this.open_settings(window, cx);
                }
            }))
    }
}
