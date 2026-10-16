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
/// viewBox stroke in label-tertiary while the step runs. The reference's ring
/// rotates on two 1.5s keyframes; a GPUI canvas path carries no animation seam,
/// so bsh draws its reduced-motion shape: a 25% track with a static arc.
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

/// The running step's ring: a full track at 25% behind a static arc.
///
/// The arc is the reference's own reduced-motion dash pair - 18 of 150 units,
/// offset by -3 - on a 9.5-radius circle in a 24-unit viewBox scaled to 14px.
/// It is the shared ongoing `StateDot`, so the composer's model trigger borrows
/// it for its own busy mark.
pub(crate) fn spinner() -> impl IntoElement {
    let size = theme::TODO_SPINNER;
    let stroke = px(theme::TODO_SPINNER_STROKE);
    let radius = theme::TODO_SPINNER_RADIUS;
    let track = rgb(theme::faint()).alpha(0.25);
    let arc = rgb(theme::faint());
    canvas(
        |_bounds, _window, _cx| (),
        move |bounds, _state, window, _cx| {
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
            let (from, to) = (0.087, 0.609);
            let mut sweep = PathBuilder::stroke(stroke);
            sweep.move_to(point_at(from));
            for step in 1..=24 {
                sweep.line_to(point_at(from + (to - from) * step as f32 / 24.0));
            }
            if let Ok(path) = sweep.build() {
                window.paint_path(path, arc);
            }
        },
    )
    .w(px(size))
    .h(px(size))
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
            .w(px(
                self.chat_w() + theme::COMPOSER_WIDEN - 2.0 * theme::TODO_SIDE_INSET
            ))
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
