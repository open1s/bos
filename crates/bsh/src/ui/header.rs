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
use crate::theme;

/// The measured jobs panel: `.menu{z-index:100;border-radius:radius-lg;
/// background:specific-menu;width:500px;max-width:min(560px,100vw - 32px);
/// max-height:min(480px,100vh - 140px);gap:1px;padding:3px;position:absolute;
/// top:calc(100% + 5px);left:0;overflow:auto}`. The kit popup brings its own
/// radius, padding and surface; the width and the cap are the measured ones.
const JOBS_MENU_W: f32 = 500.0;
const JOBS_MENU_MAX_W: f32 = 560.0;
const JOBS_MENU_MAX_H: f32 = 480.0;

/// The measured live row line: `.rowLine{align-items:stretch;gap:6px}` and
/// `.rowLineLive{border-radius:radius-md;background:fill-l2;
/// padding:4px 8px 4px 2px}`, whose own hover is a 9% label-primary mix - and
/// `.rowLineLive .row:hover{background:0 0}`, so the row inside it does not
/// hover on its own.
const JOB_LINE_GAP: f32 = 6.0;
const JOB_LINE_PAD_T: f32 = 4.0;
const JOB_LINE_PAD_R: f32 = 8.0;
const JOB_LINE_PAD_B: f32 = 4.0;
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
pub(crate) fn job_count_label(lang: crate::i18n::Lang, count: usize) -> String {
    let key = if count == 1 {
        "jobs.countLiveOne"
    } else {
        "jobs.countLiveOther"
    };
    lang.tr(key).replace("{count}", &count.to_string())
}

impl Shell {
    /// Header, active tab body, composer and status line.
    pub(crate) fn render_main_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Measured: the reference draws its hero inside the composer stack
        // while the session is blank, centres that stack in the scroll body,
        // and leaves the transcript itself empty.
        let hero = self.state.hero_phase();
        let body = match self.state.tab {
            Tab::Chat => self.render_transcript(cx).into_any_element(),
            Tab::Trajectory => self.render_trajectory(cx).into_any_element(),
        };
        let goal = match self.state.tab {
            Tab::Chat => self.goal_row(),
            // The trajectory pane already shows the goal as a card.
            Tab::Trajectory => None,
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
                    middle
                        .child(
                            div()
                                .w(px(self.chat_w()))
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
                                .w(px(self.chat_w() + theme::COMPOSER_WIDEN))
                                .flex_none()
                                .flex()
                                .flex_col()
                                // Measured: the to-do panel is a child of the
                                // composer stack, 64px narrower than the card
                                // and centred over it, with the stack's own gap.
                                .gap(px(theme::COMPOSER_STACK_GAP))
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
                    .child(div().w(px(self.chat_w())).child(self.render_statusbar(cx))),
            )
    }

    /// Title row plus tab strip.
    ///
    /// Measured: the header is 76 tall over a `10px 28px 0 20px` padding with a
    /// `.5px` `border-l3` rule under it; the tab strip adds its own 10px margin.
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .debug_selector(|| "hdr-header".to_string())
            .flex()
            .flex_col()
            .min_h(px(76.0))
            .pt(px(10.0))
            .pr(px(28.0))
            .pl(px(20.0))
            .child(self.title_row(cx))
            .child(self.tab_strip(cx));
        hairline_bottom(header).border_color(rgb(theme::border_soft()))
    }

    /// The session title and its status chips.
    fn title_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .state
            .active_session()
            .map(|session| session.title.clone())
            .unwrap_or_default();
        let streaming = self.state.any_streaming();
        let lang = self.state.lang;
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
            .child(
                // Measured: the current crumb is 14px/20px at weight 500, label-primary.
                txt(title, theme::BODY_SIZE, theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(20.0))
                    .truncate(),
            )
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
            .children(self.job_chip(cx))
            .child(div().flex_1())
            .when_some(self.state.active_id.clone(), |row, id| {
                let pinned = self
                    .state
                    .store
                    .session(&id)
                    .map(|session| session.pinned)
                    .unwrap_or(false);
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
            // The reference header ends with the panel toggle; language lives in
            // settings and the account menu, and New session lives in the sidebar.
            .child(self.header_button(
                "hdr-show-sidebar",
                icon(IconName::PanelLeft, 15.0, theme::muted()),
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
        let count = self.state.background_jobs().len();
        if count == 0 {
            return None;
        }
        Some(job_count_label(self.state.lang, count))
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
        // The measured bar: 2 wide, 72 tall, its solid 16px band between two
        // 28px fades, centred in the strip - which is the CSS default pointer-y
        // of 50% and puts it the measured 4px in from the inner edge.
        let colour = theme::scrollbar_bar();
        let faded = colour & 0xFFFFFF00;
        let fade = |from: u32, to: u32| {
            linear_gradient(
                180.0,
                linear_color_stop(rgba(from), 0.0),
                linear_color_stop(rgba(to), 1.0),
            )
        };
        let bar = div()
            .debug_selector(move || {
                match side {
                    HandleSide::Left => "chat-handle-bar-left",
                    HandleSide::Right => "chat-handle-bar-right",
                }
                .to_string()
            })
            .w(px(crate::theme::HANDLE_BAR_W))
            .h(px(crate::theme::HANDLE_BAR_H))
            .flex()
            .flex_col()
            .rounded(px(crate::theme::HANDLE_BAR_W))
            .opacity(
                if self.chat_drag.is_some() || self.chat_hover == Some(side) {
                    1.0
                } else {
                    0.0
                },
            )
            .child(
                div()
                    .w_full()
                    .h(px(crate::theme::HANDLE_BAR_FADE))
                    .bg(fade(faded, colour)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(crate::theme::HANDLE_BAR_BAND))
                    .bg(rgba(colour)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(crate::theme::HANDLE_BAR_FADE))
                    .bg(fade(colour, faded)),
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
            HandleSide::Left => handle.right(px(offset)),
            HandleSide::Right => handle.left(px(offset)),
        }
    }

    fn job_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let label = self.job_label()?;
        let shell = cx.entity();
        let button = Button::new("hdr-jobs")
            .ghost()
            .flex()
            .items_center()
            .gap(px(3.0))
            .px(px(2.0))
            .py(px(3.0))
            // The kit Button owns its own height, so the measured 28 has to be
            // set rather than floored; the composer chips take their height the
            // same way.
            .h(px(28.0))
            .rounded(px(8.0))
            .child(
                div()
                    .debug_selector(|| "hdr-jobs-dot".to_string())
                    .child(spinner()),
            )
            .child(
                div()
                    .debug_selector(|| "hdr-jobs-count".to_string())
                    .mx(px(5.0))
                    .child(txt(label, 12.0, theme::faint()).line_height(px(18.0))),
            )
            .child(
                icon(IconName::ChevronDown, 12.0, theme::faint())
                    .debug_selector(|| "hdr-jobs-chevron".to_string()),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, cx| {
                let menu = menu
                    .min_w(px(JOBS_MENU_W))
                    .max_w(px(JOBS_MENU_MAX_W))
                    .max_h(px(JOBS_MENU_MAX_H));
                let lang = shell.read(cx).state.lang;
                let jobs = shell.read(cx).state.background_jobs();
                let arm = shell.read(cx).jobs_arm.clone();
                let header = lang.tr("jobs.sectionLive");
                // Measured .sectionHeader{border-top:.5px solid border-l1;
                // color:label-tertiary;margin:4px 4px 1px;padding:4px 0 3px;
                // font-size:11px;line-height:16px}, drawn for the live section
                // only: bsh keeps no settled roster yet, so the reference's own
                // "Finished {count}" header and its Clear action have nothing to
                // count.
                let mut menu = menu.item(PopupMenuItem::element(move |_, _| {
                    let row = div()
                        .flex()
                        .items_center()
                        .w_full()
                        .mx(px(4.0))
                        .mt(px(4.0))
                        .mb(px(1.0))
                        .pt(px(4.0))
                        .pb(px(3.0))
                        .child(txt(header.clone(), 11.0, theme::faint()).line_height(px(16.0)));
                    hairline_top(row)
                        .border_color(rgb(theme::border_soft()))
                        .into_any_element()
                }));
                for (id, title) in jobs.iter() {
                    let id = id.clone();
                    let title = title.clone();
                    let elapsed = shell.read(cx).state.job_elapsed_ms(&id).unwrap_or(0);
                    let time = job_duration(elapsed, lang);
                    let phase = arm.as_ref().filter(|arm| arm.id == id).map(|arm| arm.phase);
                    let open_shell = shell.clone();
                    let kill_shell = shell.clone();
                    menu = menu.item(PopupMenuItem::element(move |_, _| {
                        // The measured live stop control: a 20x20 .5px border-l2
                        // box at radius-sm holding a 10px stop mark, which grows
                        // to width:auto;gap:4px;padding:0 7px with an 11px/18px
                        // label once it is armed.
                        let armed = phase == Some(JobPhase::Armed);
                        let failed = phase == Some(JobPhase::Failed);
                        let colour = if armed || failed {
                            theme::error()
                        } else {
                            theme::muted()
                        };
                        let stop_title = lang
                            .tr(if armed {
                                "jobs.killConfirm"
                            } else if failed {
                                "jobs.killFailed"
                            } else {
                                "jobs.killStop"
                            })
                            .replace("{label}", &title);
                        let mark = div()
                            .h(px(JOB_STOP_MARK))
                            .w(px(JOB_STOP_MARK))
                            .rounded(px(JOB_STOP_MARK_RADIUS))
                            .bg(rgb(colour));
                        let kill_shell = kill_shell.clone();
                        let kill_id = id.clone();
                        let mut stop = div()
                            .id(format!("job-stop-{kill_id}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_none()
                            .self_center()
                            .h(px(JOB_STOP_SIDE))
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .tooltip(move |window, cx| {
                                Tooltip::new(stop_title.clone()).build(window, cx)
                            })
                            .child(mark)
                            .on_click({
                                let kill_id = kill_id.clone();
                                move |_, _window, cx| {
                                    kill_shell.update(cx, |this, cx| {
                                        this.press_job_kill_at(
                                            &kill_id,
                                            std::time::Instant::now(),
                                            cx,
                                        );
                                    });
                                }
                            });
                        stop = if armed {
                            hairline_all(stop)
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
                                )
                        } else if failed {
                            hairline_all(stop)
                                .border_color(rgb(theme::error()).alpha(0.45))
                                .w(px(JOB_STOP_SIDE))
                        } else {
                            hairline_all(stop)
                                .border_color(rgb(theme::border()))
                                .w(px(JOB_STOP_SIDE))
                                .hover(|s| s.border_color(rgb(theme::error()).alpha(0.45)))
                        };
                        let open_shell = open_shell.clone();
                        let open_id = id.clone();
                        let label_id = id.clone();
                        let label_text = title.clone();
                        let label_tip = title.clone();
                        let row = div()
                            .id(format!("job-row-{open_id}"))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(JOB_ROW_MIN_H))
                            .px(px(JOB_ROW_PAD_X))
                            .py(px(JOB_ROW_PAD_Y))
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .on_click(move |_, _window, cx| {
                                open_shell.update(cx, |this, cx| this.open_job(&open_id, cx));
                            })
                            .child(spinner())
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .id(format!("job-label-{label_id}"))
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .text_size(px(12.0))
                                            .line_height(px(17.0))
                                            .text_color(rgb(theme::text()))
                                            .text_ellipsis()
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(label_tip.clone()).build(window, cx)
                                            })
                                            .child(label_text),
                                    )
                                    .child(
                                        div()
                                            .id(format!("job-time-{label_id}"))
                                            .text_size(px(JOB_DURATION))
                                            .line_height(px(18.0))
                                            .text_color(rgb(theme::faint()))
                                            .child(time.clone()),
                                    ),
                            );
                        div()
                            .flex()
                            .items_stretch()
                            .gap(px(JOB_LINE_GAP))
                            .rounded(px(12.0))
                            .bg(rgb(theme::new_session_fill()))
                            .pt(px(JOB_LINE_PAD_T))
                            .pr(px(JOB_LINE_PAD_R))
                            .pb(px(JOB_LINE_PAD_B))
                            .pl(px(JOB_LINE_PAD_L))
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(row)
                            .child(stop)
                            .into_any_element()
                    }));
                }
                menu
            });
        Some(
            div()
                .debug_selector(|| "hdr-jobs".to_string())
                .flex()
                .child(button)
                .into_any_element(),
        )
    }

    /// Bring a background job's session to the front, which is what a menu item
    /// does.
    pub(crate) fn open_job(&mut self, id: &str, cx: &mut Context<Self>) {
        self.state.select(id);
        cx.notify();
    }

    fn open_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let root = self.state.settings.active_workspace.clone();
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

    /// The goal being pursued, as one row above the composer.
    fn goal_row(&self) -> Option<AnyElement> {
        let goal = self.state.active_session()?.goal.as_ref()?;
        Some(
            div()
                .mx(px(30.0))
                .mb(px(6.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(theme::border()))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(txt(
                    self.state.lang.tr("pane.goal"),
                    theme::CHIP_SIZE,
                    theme::muted(),
                ))
                .child(txt(goal.objective.clone(), theme::CHIP_SIZE, theme::text()).flex_1())
                .into_any_element(),
        )
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
                    this.state.tab = this_tab;
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
            .child(entry(
                "tab-trajectory",
                "tab.trajectory",
                Tab::Trajectory,
                cx,
            ))
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
                _ => {
                    this.open_settings(window, cx);
                }
            }))
    }
}
