//! The reference's to-do panel: the agent's working plan over the composer.
//!
//! Measured from TodoPanel.module.css and TodoPanel.js in
//! dsh-client-ui-conversation, plus the shared StateDot primitive: a radius-lg
//! menu surface 64px narrower than its container, holding a 13/24 weight-500
//! "To-dos" title, a 13/20 per-status progress summary, a 14px chevron and one
//! 16px glyph per item beside its 13/20 label. The reference starts the list
//! collapsed and renders nothing at all with no plan.
//!
//! The data is the agent's own plan store, which its update_plan tool writes
//! through: a running turn reads live and the finished copy is saved onto the
//! session, which is also what the trajectory pane reads.

use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, icon, txt, DivExt, IconName};
use crate::i18n::{tr_args, Lang};
use crate::model::{PlanItem, PlanStatus};
use crate::theme;

/// The header summary: one segment per non-zero status, joined with a spaced
/// middle dot, which is the reference's own rule - zero-count segments are
/// dropped as noise, and a non-empty list always keeps at least one.
pub(crate) fn progress(items: &[PlanItem], lang: Lang) -> String {
    let done = items
        .iter()
        .filter(|item| item.status == PlanStatus::Completed)
        .count();
    let active = items
        .iter()
        .filter(|item| item.status == PlanStatus::InProgress)
        .count();
    let pending = items.len() - done - active;
    let mut parts: Vec<String> = Vec::new();
    for (count, name, key) in [
        (done, "done", "todo.progress.done"),
        (active, "active", "todo.progress.active"),
        (pending, "pending", "todo.progress.pending"),
    ] {
        if count > 0 {
            parts.push(tr_args(lang, key, &[(name, count.to_string())]));
        }
    }
    parts.join(" · ")
}

/// One item's glyph, in the reference's shared StateDot language.
///
/// Measured: a 10px slot painting a 6px core - the static green-500 when the
/// step is done, state-idle-primary while it waits - and a 14px ring with a 2px
/// viewBox stroke in label-tertiary while the step runs. The running ring is
/// [`spinner`], the shared ongoing mark, so the composer's busy chip, the job
/// rows and the trajectory all turn and breathe on the same measured cycle.
fn glyph(status: PlanStatus) -> Div {
    let slot = div()
        .w(px(theme::TODO_GLYPH))
        .h(px(theme::TODO_GLYPH))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match status {
        PlanStatus::InProgress => slot.child(spinner()),
        PlanStatus::Completed | PlanStatus::Pending => {
            let core = if status == PlanStatus::Completed {
                theme::TODO_DONE
            } else {
                theme::todo_idle()
            };
            slot.child(
                div()
                    .w(px(theme::TODO_DOT))
                    .h(px(theme::TODO_DOT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(theme::TODO_DOT_CORE))
                            .h(px(theme::TODO_DOT_CORE))
                            .rounded_full()
                            .bg(rgb(core)),
                    ),
            )
        }
    }
}

/// The reference's ongoing cycle, measured from StateDot.module.css.
///
/// `dsh-state-dot-spin` is `1.5s linear infinite` on a `rotate(360deg)`
/// transform, and `dsh-state-dot-dash` is `1.5s ease-in-out infinite` on the
/// arc's dash pair. Both carry that one period so pinning their start times
/// aligns them, which is why a single phase drives both here.
pub(crate) const SPIN_PERIOD: std::time::Duration = std::time::Duration::from_millis(1500);

/// The dash pattern's own length space: one circumference of the reference's
/// 24-unit viewBox ring (r = 9.5), the units `stroke-dasharray` is written in.
/// bsh draws that ring at 14px, so a dash length converts to turns by this.
pub(crate) const SPIN_CIRCUMFERENCE: f32 = 59.690_26;

/// The running arc's start and sweep in turns of the ring, for a phase in
/// `0..1` of [`SPIN_PERIOD`].
///
/// Measured keyframes: the dash runs `12 150` at offset 0, `24 150` at -6 and
/// back to `12 150` at 0, so the arc both grows and walks forward. A negative
/// `stroke-dashoffset` starts the dash that many units into the path. The
/// sheet's `ease-in-out` is the kit's quadratic S-curve, which departs from
/// CSS's `cubic-bezier(.42, 0, .58, 1)` by under 0.5% of the sweep.
pub(crate) fn spin_sweep(phase: f32) -> (f32, f32) {
    let phase = if phase.is_finite() {
        phase.rem_euclid(1.0)
    } else {
        0.0
    };
    let rising = phase < 0.5;
    let eased = ease_in_out(if rising {
        phase * 2.0
    } else {
        (phase - 0.5) * 2.0
    });
    let (dash, offset) = if rising {
        (12.0 + 12.0 * eased, -6.0 * eased)
    } else {
        (24.0 - 12.0 * eased, -6.0 + 6.0 * eased)
    };
    (-offset / SPIN_CIRCUMFERENCE, dash / SPIN_CIRCUMFERENCE)
}

/// The `prefers-reduced-motion` shape: `stroke-dasharray: 18 150` at
/// `stroke-dashoffset: -3`, which the sheet substitutes for both animations
/// and which bsh paints as the still ring.
pub(crate) fn spin_rest() -> (f32, f32) {
    (3.0 / SPIN_CIRCUMFERENCE, 18.0 / SPIN_CIRCUMFERENCE)
}

/// The running step's ring at one phase of the cycle: a full track at 25%
/// behind the moving arc, on a 9.5-radius circle in a 24-unit viewBox.
fn spinner_at(phase: f32) -> Div {
    let size = theme::TODO_SPINNER;
    let stroke = px(theme::TODO_SPINNER_STROKE);
    let radius = theme::TODO_SPINNER_RADIUS;
    let track = rgb(theme::faint()).alpha(0.25);
    let arc = rgb(theme::faint());
    div()
        .debug_selector(|| "state-dot-spinner".to_string())
        .w(px(size))
        .h(px(size))
        .flex_none()
        .child(
            canvas(
                |_bounds, _window, _cx| (),
                move |bounds, _state, window, cx| {
                    let center = bounds.center();
                    let point_at = |turns: f32| {
                        let angle = turns * std::f32::consts::TAU;
                        point(
                            center.x + px(radius * angle.cos()),
                            center.y + px(radius * angle.sin()),
                        )
                    };
                    let mut ring = PathBuilder::stroke(stroke);
                    ring.move_to(point_at(0.0));
                    for step in 1..=32 {
                        ring.line_to(point_at(step as f32 / 32.0));
                    }
                    ring.close();
                    if let Ok(path) = ring.build() {
                        window.paint_path(path, track);
                    }
                    // The ring turns once per cycle under the arc's own
                    // breathing; reduced motion freezes both, as the sheet's
                    // media query does.
                    let (start, sweep) = if cx.reduce_motion() {
                        spin_rest()
                    } else {
                        let (start, sweep) = spin_sweep(phase);
                        (start + phase, sweep)
                    };
                    let steps = ((sweep * 32.0).ceil() as usize).max(2);
                    let mut dash = PathBuilder::stroke(stroke);
                    dash.move_to(point_at(start));
                    for step in 1..=steps {
                        dash.line_to(point_at(start + sweep * step as f32 / steps as f32));
                    }
                    if let Ok(path) = dash.build() {
                        window.paint_path(path, arc);
                    }
                },
            )
            .w(px(size))
            .h(px(size)),
        )
}

/// How often the ring redraws.
///
/// The reference's cycle is a composited CSS animation, which costs it no
/// layout; a GPUI animation frame rebuilds the view, so the mark samples at
/// half the display rate. The ring still turns 8 degrees a frame, which reads
/// as motion, and a window with no mark in it pays nothing at all.
pub(crate) const SPIN_FPS: f32 = 30.0;

/// The shared ongoing mark: the reference's turn-and-breathe ring.
///
/// `AnimationExt::with_animation` is the frame seam the canvas path lacked, and
/// it is where the reference's media query lands too: with reduce motion on it
/// renders the cycle's start once and schedules no frames, while the paint
/// closure sees `reduce_motion()` and draws the sheet's own still dash.
pub(crate) fn spinner() -> impl IntoElement {
    spinner_at(0.0).with_animation(
        "state-dot-spinner",
        Animation::new(SPIN_PERIOD).repeat().with_max_fps(SPIN_FPS),
        |_still: Div, phase| spinner_at(phase),
    )
}

impl Shell {
    /// The to-do panel above the composer card, absent while no plan exists.
    pub(crate) fn todo_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let id = self.state.active_id.clone()?;
        let items = self.state.plan_items(&id);
        if items.is_empty() {
            return None;
        }
        let lang = self.state.lang;
        let expanded = self.todo_open;
        let panel = div()
            .debug_selector(|| "todo-panel".to_string())
            .w(px(self.composer_w() - 2.0 * theme::TODO_SIDE_INSET))
            .rounded(px(theme::TODO_RADIUS))
            .bg(rgb(theme::page()))
            .shadow_sm()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("todo-panel-body")
                    .flex()
                    .flex_col()
                    .gap(px(theme::TODO_GAP))
                    .px(px(theme::TODO_PAD_X))
                    .py(px(theme::TODO_PAD_Y))
                    .child(
                        div()
                            .id("todo-panel-header")
                            .debug_selector(|| "todo-panel-header".to_string())
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(theme::TODO_HEAD_GAP))
                            .cursor_pointer()
                            .child(icon(
                                IconName::ListChecks,
                                theme::TODO_GLYPH,
                                theme::faint(),
                            ))
                            .child(
                                txt(lang.tr("todo.title"), theme::TODO_TITLE, theme::text())
                                    .flex_none()
                                    .font_weight(FontWeight::MEDIUM)
                                    .line_height(px(theme::TODO_TITLE_LINE)),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    txt(progress(&items, lang), theme::TODO_TITLE, theme::faint())
                                        .line_height(px(theme::TODO_LINE))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap(),
                                ),
                            )
                            .child(icon(
                                if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronUp
                                },
                                theme::TODO_CHEVRON,
                                theme::faint(),
                            ))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.todo_open = !this.todo_open;
                                cx.notify();
                            })),
                    )
                    .when(expanded, |body| {
                        body.child(
                            div()
                                .id("todo-panel-list")
                                .debug_selector(|| "todo-panel-list".to_string())
                                .flex()
                                .flex_col()
                                .gap(px(theme::TODO_GAP))
                                .max_h(px(theme::TODO_LIST_MAX_H))
                                .overflow_y_scroll()
                                .children(items.iter().map(|item| {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(theme::TODO_HEAD_GAP))
                                        .child(glyph(item.status))
                                        .child(
                                            div().flex_1().min_w_0().child(
                                                txt(
                                                    item.text.clone(),
                                                    theme::TODO_TITLE,
                                                    theme::muted(),
                                                )
                                                .line_height(px(theme::TODO_LINE))
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap(),
                                            ),
                                        )
                                })),
                        )
                    }),
            );
        // The reference lifts the panel with elevation-panel, whose ring is
        // elevation-stroke at border-l1; a GPUI shadow carries no ring, so the
        // stroke is drawn as a 0.5px hairline.
        Some(hairline_all(panel).border_color(rgb(theme::border_soft())))
    }
}
