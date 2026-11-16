//! The composer's context meter.
//!
//! Measured from ContextMeter.module.css (round 103): a 14px ring beside the
//! percentage under the composer card, opening a panel 8px above it on the menu
//! surface. The panel's three buckets are the reference's own contextBreakdown
//! heuristic, which its README documents as the context's composition rather
//! than its provider-billed size - every figure in it carries the reference's
//! own tilde.

use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, txt, DivExt};
use crate::i18n::Lang;
use crate::model::ContextBreakdown;
use crate::state::human_tokens;
use crate::theme;

/// The reference's legend order and tints: the system prompt, the tool
/// definitions, then every other message.
const ROWS: [(&str, u32); 3] = [
    ("context.system", theme::METER_SYSTEM),
    ("context.tools", theme::METER_TOOLS),
    ("context.messages", theme::METER_MESSAGES),
];

impl Shell {
    /// The meter under the card, once a provider has reported a prompt size
    /// against a budget. The reference renders nothing before that.
    pub(crate) fn context_meter(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (used, window) = self.state.context_figures()?;
        let percent = self.state.context_percent()?;
        let size = self.state.draft().global.general.content_font_size;
        let lang = self.state.lang;
        let open = self.state.meter_open;
        // The breakdown prices every model-visible message, so it is read only
        // while the panel that draws it is up: pricing the whole transcript for
        // a panel nobody opened was a per-frame walk of the conversation.
        let breakdown = if open {
            self.state.context_breakdown()
        } else {
            None
        };
        let trigger = self.meter_trigger(size, percent, open, cx);
        Some(
            div()
                .debug_selector(|| "context-meter".to_string())
                .relative()
                .flex()
                .child(trigger)
                .when(open, |anchor| {
                    anchor.child(self.meter_panel(size, lang, percent, used, window, breakdown))
                }),
        )
    }

    /// The trigger: the ring filled to the reading, then the reading.
    fn meter_trigger(
        &self,
        size: u32,
        percent: u64,
        open: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .debug_selector(|| "context-meter-trigger".to_string())
            .id("context-meter-trigger")
            .flex()
            .items_center()
            .gap(px(theme::METER_GAP))
            .px(px(theme::METER_PAD_X))
            .py(px(theme::METER_PAD_Y))
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .when(open, |row| row.bg(rgb(theme::hover())))
            .hover(|row| row.bg(rgb(theme::hover())))
            .child(self.meter_ring(percent))
            .child(
                txt(
                    format!("{percent}%"),
                    theme::small_size_at(size),
                    theme::faint(),
                )
                .line_height(px(theme::stat_line_at(size)))
                .flex_none(),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.state.toggle_meter();
                cx.notify();
            }))
    }

    /// The 14px ring: a full track with the reading's arc over it.
    ///
    /// GPUI has no arc primitive, so the ring is painted on a canvas. Both
    /// circles are polylines at the reference's 2px stroke: the track in the
    /// border-l3 colour and the arc, swept from twelve o'clock, in the tertiary
    /// label colour.
    fn meter_ring(&self, percent: u64) -> impl IntoElement {
        let sweep = percent.min(100) as f32 / 100.0;
        let stroke = px(theme::METER_STROKE);
        let radius = theme::METER_RING_RADIUS;
        let track_color = rgb(theme::border());
        let fill_color = rgb(theme::faint());
        canvas(
            |_bounds, _window, _cx| (),
            move |bounds, _state, window, _cx| {
                let center = bounds.center();
                // The reference rotates its dash array by -90 degrees, so the
                // arc starts at the top of the ring.
                let point_at = |turns: f32| {
                    let angle = turns * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    point(
                        center.x + px(radius * angle.cos()),
                        center.y + px(radius * angle.sin()),
                    )
                };
                let mut track = PathBuilder::stroke(stroke);
                track.move_to(point_at(0.0));
                for step in 1..=32 {
                    track.line_to(point_at(step as f32 / 32.0));
                }
                track.close();
                if let Ok(path) = track.build() {
                    window.paint_path(path, track_color);
                }
                if sweep > 0.0 {
                    let steps = ((sweep * 48.0).ceil() as usize).max(1);
                    let mut arc = PathBuilder::stroke(stroke);
                    arc.move_to(point_at(0.0));
                    for step in 1..=steps {
                        arc.line_to(point_at(sweep * step as f32 / steps as f32));
                    }
                    if let Ok(path) = arc.build() {
                        window.paint_path(path, fill_color);
                    }
                }
            },
        )
        .w(px(theme::METER_RING))
        .h(px(theme::METER_RING))
        .flex_none()
    }
    /// The measured breakdown panel, 8px above the trigger.
    fn meter_panel(
        &self,
        size: u32,
        lang: Lang,
        percent: u64,
        used: u64,
        window: u64,
        breakdown: Option<ContextBreakdown>,
    ) -> Div {
        let (before, after) = headline_parts(lang);
        let caption = |text: String| {
            txt(text, theme::METER_PANEL_SIZE, theme::faint())
                .line_height(px(theme::METER_PANEL_LINE))
        };
        // The reference splits its occupancy sentence on the reading, and an
        // empty half draws nothing at all.
        let mut head = div()
            .flex()
            .items_center()
            .gap(px(theme::METER_HEADER_GAP))
            .child(
                txt(
                    format!("{percent}%"),
                    theme::METER_PANEL_SIZE,
                    theme::text(),
                )
                .font_weight(FontWeight::MEDIUM)
                .line_height(px(theme::METER_PANEL_LINE)),
            );
        if !before.is_empty() {
            head = head.child(caption(before));
        }
        if !after.is_empty() {
            head = head.child(caption(after));
        }
        let head = head.child(
            div().flex_1().min_w_0().flex().justify_end().child(
                txt(
                    format!("~{} / {}", human_tokens(used), human_tokens(window)),
                    theme::METER_PANEL_SIZE,
                    theme::text(),
                )
                .font_weight(FontWeight::MEDIUM)
                .line_height(px(theme::METER_PANEL_LINE)),
            ),
        );

        // The bar: the reading split across the buckets, or one segment of the
        // reading alone while nothing else is priced.
        let split = breakdown.unwrap_or_default();
        let total = split.total();
        let shares: Vec<(u32, f32)> = if total == 0 {
            vec![(theme::METER_SYSTEM, percent.min(100) as f32)]
        } else {
            ROWS.iter()
                .enumerate()
                .map(|(index, (_, tint))| {
                    (
                        *tint,
                        percent.min(100) as f32 * split.bucket(index) as f32 / total as f32,
                    )
                })
                .filter(|(_, share)| *share > 0.0)
                .collect()
        };
        let mut bar = div()
            .debug_selector(|| "context-meter-bar".to_string())
            .flex()
            .w_full()
            .h(px(theme::METER_BAR_H))
            .mt(px(theme::METER_BAR_TOP))
            .mb(px(theme::METER_BAR_BOTTOM))
            .gap(px(theme::METER_SEG_GAP))
            .rounded(px(999.0))
            .bg(rgb(theme::hover()))
            .overflow_hidden();
        for (tint, share) in shares {
            bar = bar.child(
                div()
                    .flex_grow(share)
                    .flex_basis(px(0.0))
                    .flex_shrink(0.0)
                    .min_w(px(theme::METER_SEG_MIN_W))
                    .h_full()
                    .rounded(px(1.0))
                    .bg(rgb(tint)),
            );
        }

        // The legend is drawn only when a breakdown exists, like the reference.
        let mut rows = div()
            .debug_selector(|| "context-meter-rows".to_string())
            .flex()
            .flex_col()
            .mt(px(theme::METER_ROWS_TOP));
        if breakdown.is_some() {
            for (index, (key, tint)) in ROWS.iter().enumerate() {
                rows = rows.child(self.meter_row(lang, key, *tint, split.bucket(index)));
            }
        }

        let panel = div()
            .debug_selector(|| "context-meter-panel".to_string())
            .w(px(theme::METER_PANEL_W))
            .p(px(theme::METER_PANEL_PAD))
            .rounded(px(theme::RADIUS_LG))
            .bg(rgb(theme::page()))
            .shadow_lg()
            .text_size(px(theme::METER_PANEL_SIZE))
            .line_height(px(theme::METER_PANEL_LINE))
            .text_color(rgb(theme::muted()))
            .occlude()
            .child(head)
            .child(bar)
            .child(rows);
        let panel = hairline_all(panel).border_color(rgb(theme::border_soft()));
        // The wrapper spans the trigger. The panel hangs from the trigger's LEFT
        // edge, not centred on it: the trigger now leads the status row, so a
        // centred 264px panel ran off the column's left edge and lost its first
        // characters ("system prompt" was drawn "ystem prompt").
        div()
            .absolute()
            .bottom(px(theme::stat_line_at(size)
                + theme::METER_PAD_Y * 2.0
                + theme::METER_PANEL_GAP))
            .left(px(0.0))
            .flex()
            .justify_start()
            .child(panel)
    }

    /// One legend row: the tinted swatch and label beside the tilde figure.
    fn meter_row(&self, lang: Lang, key: &'static str, tint: u32, tokens: u64) -> Div {
        div()
            .debug_selector(move || format!("context-row-{key}"))
            .flex()
            .justify_between()
            .items_center()
            .gap(px(theme::METER_ROW_GAP))
            .py(px(theme::METER_ROW_PAD_Y))
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .w(px(theme::METER_SWATCH))
                            .h(px(theme::METER_SWATCH))
                            .mr(px(theme::METER_SWATCH_GAP))
                            .rounded(px(2.0))
                            .flex_none()
                            .bg(rgb(tint)),
                    )
                    .child(txt(
                        crate::i18n::tr(lang, key),
                        theme::METER_PANEL_SIZE,
                        theme::muted(),
                    )),
            )
            .child(
                txt(
                    format!("~{}", human_tokens(tokens)),
                    theme::METER_PANEL_SIZE,
                    theme::text(),
                )
                .line_height(px(theme::METER_PANEL_LINE)),
            )
    }
}

/// The reference's occupancy sentence, split on where the reading goes.
///
/// Its headBefore and headAfter are the halves of context.aria around a reading
/// slot, so splitting bsh's own template on the same {percent} slot gives both
/// halves without rendering the number twice: the English sentence opens with
/// the figure and the Chinese one ends with it.
fn headline_parts(lang: Lang) -> (String, String) {
    let template = crate::i18n::tr(lang, "context.aria");
    match template.split_once("{percent}") {
        Some((before, after)) => (before.trim().to_string(), after.trim().to_string()),
        None => (template, String::new()),
    }
}
