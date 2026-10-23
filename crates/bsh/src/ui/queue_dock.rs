//! The reference's queue dock: the messages waiting for a running turn.
//!
//! Measured from QueueDock.module.css and QueueDock.js in
//! dsh-client-ui-conversation: a panel 64px narrower than the composer card,
//! rounded on its top corners only and tucked under the card's edge, holding a
//! 36-tall count header - drawn only when more than one message waits - over
//! rows that preview the message beside the actions that rewrite it.
//!
//! The reference's rows are its session's durable inbox, echoed by a
//! "Sending…" row only for a local submission the Host has not admitted yet.
//! bsh has no admission round-trip, so that phase does not exist here. A
//! STEERED message does have a pending span - it leaves this queue when the
//! reader presses Send now and the running turn takes it at its next step - and
//! the reference shows that span in the CONVERSATION (its pending steering
//! bubble), not here. The transcript draws it, so one message never appears in
//! two places; when the turn ends before taking it, the row comes back here.

use gpui_kit::component::input::Textarea;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, hairline_no_bottom, hairline_top, icon, txt, DivExt, IconName};
use crate::i18n::tr_args;
use crate::state::Queued;
use crate::theme;

/// The reference's own preview rule, measured from `previewOf`.
///
/// Text blocks join with a space, runs of whitespace collapse to one, and the
/// first 200 characters survive; anything longer gets an ellipsis. The budget
/// counts characters rather than bytes, which is what its `Array.from` is for.
pub(crate) fn preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() > theme::QUEUE_PREVIEW_CHARS {
        format!(
            "{}…",
            chars[..theme::QUEUE_PREVIEW_CHARS]
                .iter()
                .collect::<String>()
        )
    } else {
        flat
    }
}

/// One 28px round action hit box, in the reference's own shape.
///
/// The kit hands out `on_click` on a stateful element only, so every action
/// carries its own identity: the row's seq keeps two rows apart and the name
/// keeps the two or three actions inside one row apart.
///
/// The label is both the accessible name and the hover tip, which is the
/// reference's own pairing (`aria-label` = the tooltip label on every queue
/// path button). GPUI needs a role as well as a label for a div to reach
/// AccessKit, so these are buttons, not anonymous click targets.
fn action_box(name: &'static str, seq: u64, label: SharedString) -> Stateful<Div> {
    div()
        .debug_selector(move || format!("{name}-{seq}"))
        .id((name, seq as usize))
        .role(Role::Button)
        .aria_label(label.clone())
        .tooltip_show_delay(std::time::Duration::from_millis(500))
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        .w(px(theme::QUEUE_ACTION))
        .h(px(theme::QUEUE_ACTION))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(theme::hover())))
}

/// An action that cannot run: the reference's own disabled face.
fn action_disabled(
    glyph: IconName,
    name: &'static str,
    seq: u64,
    label: SharedString,
) -> Stateful<Div> {
    action_box(name, seq, label).opacity(0.45).child(icon(
        glyph,
        theme::QUEUE_ACTION_GLYPH,
        theme::faint(),
    ))
}

impl Shell {
    /// The queue dock above the composer, or nothing while no message waits.
    ///
    /// One waiting message draws its row directly; a longer queue collapses
    /// behind the count header, which is the reference's own default and its
    /// `hidden` list.
    pub(crate) fn queue_dock(&self, cx: &mut Context<Self>) -> Option<Div> {
        let id = self.state.active_id.clone()?;
        // Only the DURABLE queue is listed. A steered message has left it: the
        // reference moves the row to the next-step inbox and shows the pending
        // steering bubble in the conversation instead, so echoing it here too
        // would show one message in two places.
        let items = self.state.inbox(&id);
        if items.is_empty() {
            return None;
        }
        let count = items.len();
        let expanded = self.queue_open || self.queue_edit_for.is_some();
        let list_visible = count == 1 || expanded;
        let header_label = tr_args(self.state.lang, "queue.count", &[("n", count.to_string())]);
        let mut rows: Vec<Div> = Vec::with_capacity(count);
        for (index, item) in items.iter().enumerate() {
            rows.push(self.queue_row(&id, index, item, count, cx));
        }
        let panel = div()
            .debug_selector(|| "queue-dock".to_string())
            .w(px(self.composer_w() - 2.0 * theme::QUEUE_SIDE_INSET))
            .rounded_t(px(theme::QUEUE_RADIUS))
            .bg(rgb(theme::page()))
            .py(px(theme::QUEUE_PAD_Y))
            .overflow_hidden()
            .flex()
            .flex_col()
            .when(count > 1, |panel| {
                panel.child(
                    div()
                        .id("queue-dock-header")
                        .debug_selector(|| "queue-dock-header".to_string())
                        .role(Role::Button)
                        .aria_label(header_label.clone())
                        .aria_expanded(expanded)
                        .h(px(theme::QUEUE_HEAD_H))
                        .px(px(theme::QUEUE_HEAD_PAD_X))
                        .py(px(theme::QUEUE_HEAD_PAD_Y))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::QUEUE_HEAD_GAP))
                        .cursor_pointer()
                        .child(icon(IconName::Inbox, theme::QUEUE_GLYPH, theme::faint()))
                        .child(
                            div().flex_1().min_w_0().child(
                                txt(header_label, theme::QUEUE_COUNT, theme::text())
                                    .font_weight(FontWeight::MEDIUM)
                                    .line_height(px(theme::QUEUE_COUNT_LINE))
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
                            theme::QUEUE_CHEVRON,
                            theme::faint(),
                        ))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.queue_open = !this.queue_open;
                            cx.notify();
                        })),
                )
            })
            .when(list_visible, |panel| {
                panel.child(
                    div()
                        .id("queue-dock-list")
                        .debug_selector(|| "queue-dock-list".to_string())
                        .flex()
                        .flex_col()
                        .max_h(px(theme::QUEUE_LIST_MAX_H))
                        .overflow_y_scroll()
                        .children(rows),
                )
            });
        // The reference lifts the panel with the menu fill and rings it at
        // border-l1 with the bottom edge left open, because the card covers it.
        // A GPUI shadow carries no ring, so the stroke is a 0.5px hairline.
        Some(hairline_no_bottom(panel).border_color(rgb(theme::border_soft())))
    }

    /// One waiting message: its preview, or its editor, beside its actions.
    fn queue_row(
        &self,
        id: &str,
        index: usize,
        item: &Queued,
        count: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let session = id.to_string();
        let seq = item.seq;
        let editing = self.queue_edit_for == Some(seq);
        // The reference refuses to edit a row that carries anything but text.
        let editable = item.attachments.is_empty();
        let mut row = div()
            .debug_selector(move || format!("queue-row-{seq}"))
            .min_h(px(theme::QUEUE_ROW_MIN_H))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::QUEUE_ROW_GAP))
            .rounded(px(theme::QUEUE_ROW_RADIUS))
            .pl(px(theme::QUEUE_ROW_PAD_L))
            .pr(px(theme::QUEUE_ROW_PAD_R))
            .py(px(theme::QUEUE_ROW_PAD_Y));
        // The reference rules adjacent rows apart with an inset top shadow,
        // which GPUI does not carry; the measured hairline sits on the row.
        if index > 0 {
            row = hairline_top(row).border_color(rgb(theme::border_soft()));
        }
        if count == 1 {
            row = row.child(icon(IconName::Inbox, theme::QUEUE_GLYPH, theme::faint()));
        }
        if editing {
            row = row.child(
                hairline_all(
                    div()
                        .id(("queue-editor", seq as usize))
                        .flex_1()
                        .min_w_0()
                        .min_h(px(theme::QUEUE_EDITOR_MIN_H))
                        .max_h(px(theme::QUEUE_EDITOR_MAX_H))
                        .px(px(theme::QUEUE_EDITOR_PAD_X - theme::FIELD_INTERIOR_PAD))
                        .rounded(px(theme::QUEUE_EDITOR_RADIUS))
                        .bg(rgb(theme::page()))
                        .overflow_y_scroll()
                        .flex()
                        .child(Textarea::new(&self.queue_edit).appearance(false)),
                )
                .border_color(rgb(theme::border())),
            );
        } else {
            row = row.child(
                div().flex_1().min_w_0().child(
                    txt(
                        preview(&crate::state::display_text(&item.text, &item.attachments)),
                        theme::QUEUE_COUNT,
                        theme::muted(),
                    )
                    .line_height(px(theme::QUEUE_COUNT_LINE))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap(),
                ),
            );
        }
        // The reference disables Save on an empty draft, so the draft's own
        // emptiness is read before the button is built.
        let draft = self.queue_edit.read(cx).value().trim().to_string();
        let lang = self.state.lang;
        let mut actions = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::QUEUE_ACTION_GAP));
        if editing {
            let save_session = session.clone();
            let save_label: SharedString = lang.tr("queue.save").into();
            let save = if draft.is_empty() {
                action_disabled(IconName::Check, "queue-save", seq, save_label)
            } else {
                action_box("queue-save", seq, save_label)
                    .child(icon(
                        IconName::Check,
                        theme::QUEUE_ACTION_GLYPH,
                        theme::faint(),
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        let text = this.queue_edit.read(cx).value().trim().to_string();
                        if !text.is_empty() {
                            this.state.edit_queued(&save_session, seq, text);
                        }
                        this.queue_edit_for = None;
                        cx.notify();
                    }))
            };
            actions = actions.child(save).child(
                action_box("queue-cancel", seq, lang.tr("queue.cancelEdit").into())
                    .child(icon(IconName::X, theme::QUEUE_ACTION_GLYPH, theme::faint()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.queue_edit_for = None;
                        cx.notify();
                    })),
            );
        } else {
            let remove_session = session.clone();
            let text = item.text.clone();
            let edit_label: SharedString = lang.tr("queue.edit").into();
            let edit = if editable {
                action_box("queue-edit", seq, edit_label.clone())
                    .child(icon(
                        IconName::Pencil,
                        theme::QUEUE_ACTION_GLYPH,
                        theme::faint(),
                    ))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.queue_edit_for = Some(seq);
                        this.queue_edit.update(cx, |input, cx| {
                            input.set_value(text.as_str(), window, cx);
                            input.focus(window, cx);
                        });
                        cx.notify();
                    }))
            } else {
                action_disabled(IconName::Pencil, "queue-edit", seq, edit_label)
            };
            // Steer: send this one message into the running turn now. The
            // reference calls it Steer and draws it last; the affordance is the
            // send button's own arrow, so the row reads as "send", not as a
            // second edit-like action.
            let steer_session = session.clone();
            let steer = action_box("queue-steer", seq, lang.tr("queue.sendNow").into())
                .w(px(theme::QUEUE_STEER))
                .h(px(theme::QUEUE_STEER))
                .child(icon(
                    IconName::ArrowUp,
                    theme::QUEUE_STEER_GLYPH,
                    theme::faint(),
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.steer_now(&steer_session, seq, cx);
                }));
            actions = actions
                .child(edit)
                .child(
                    action_box("queue-remove", seq, lang.tr("queue.remove").into())
                        .child(icon(
                            IconName::Trash,
                            theme::QUEUE_ACTION_GLYPH,
                            theme::faint(),
                        ))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.state.remove_queued(&remove_session, seq);
                            cx.notify();
                        })),
                )
                .child(steer);
        }
        row.child(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::preview;

    /// The reference collapses whitespace and caps the preview at 200
    /// characters, counted rather than measured in bytes.
    #[test]
    fn the_preview_collapses_whitespace_and_caps_the_length() {
        assert_eq!(preview("  a \n b\tc  "), "a b c");
        assert_eq!(preview(""), "");
        let exact = "x".repeat(200);
        assert_eq!(
            preview(&exact).chars().count(),
            200,
            "the cap itself is kept whole"
        );
        let long = "x".repeat(250);
        let capped = preview(&long);
        assert_eq!(
            capped.chars().count(),
            201,
            "200 characters plus the ellipsis"
        );
        assert!(capped.ends_with('…'), "a longer preview is cut short");
    }
}
