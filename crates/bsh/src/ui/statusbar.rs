//! The stat pills under the composer.
//!
//! Measured from the reference's `StatsPills` (round 102): a centred row of
//! pills over a 12px gap, each a 14px glyph plus a `secondary - 1px` label with
//! a `·` between its parts, and the usage pill opens the measured token dialog.

use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, icon, txt, DivExt, IconName};
use crate::i18n::{tr_args, Lang};
use crate::model::UsageSummary;
use crate::state::{cache_hit_percent, exact_tokens, human_tokens};
use crate::theme;

impl Shell {
    /// The reference's stat pills: the time pill first, then the usage pill.
    ///
    /// The reference's time pill is a plain span unless its timing projection
    /// has values, and bsh models no per-step timings, so it stays that shape.
    /// The usage pill is the button over the token dialog.
    pub(crate) fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self.state.draft().global.general.content_font_size;
        let lang = self.state.lang;
        let (turns, steps) = self
            .state
            .active_session()
            .map(|session| (session.turn_count(), session.step_count()))
            .unwrap_or((0, 0));
        let mut row = div()
            .debug_selector(|| "status-pills".to_string())
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(theme::STAT_GAP));
        // The reference draws the counts pill only once a step has landed.
        if steps > 0 {
            row = row.child(self.stat_time_pill(size, turns, steps, lang));
        }
        if let Some(summary) = self.state.stat_summary() {
            row = row.child(self.stat_usage_pill(size, summary, lang, cx));
        }
        if let Some(model) = self.state.status_model() {
            row = row.child(self.stat_text(size, model));
        }
        if let Some(notice) = &self.state.notice {
            row = row.child(txt(
                notice.clone(),
                theme::stat_size_at(size),
                theme::warn(),
            ));
        }
        row
    }

    /// A plain label in the stat row, at the pill's own size.
    fn stat_text(&self, size: u32, text: impl Into<SharedString>) -> Div {
        txt(text, theme::stat_size_at(size), theme::faint())
            .line_height(px(theme::stat_line_at(size)))
    }

    /// The measured `·` between two parts inside one pill.
    fn stat_sep(&self, size: u32) -> Div {
        txt("·", theme::stat_size_at(size), theme::border_soft())
            .mx(px(theme::STAT_SEP_MARGIN))
            .line_height(px(theme::stat_line_at(size)))
    }

    /// One pill shell: the glyph and the label inside the measured padding.
    fn stat_pill(&self, size: u32, glyph: IconName, label: Div) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(theme::STAT_PILL_GAP))
            .px(px(theme::STAT_PILL_PAD_X))
            .py(px(theme::STAT_PILL_PAD_Y))
            .rounded(px(999.0))
            .min_w_0()
            .child(
                icon(glyph, theme::STAT_PILL_ICON, theme::faint())
                    .line_height(px(theme::stat_line_at(size))),
            )
            .child(label)
    }

    /// The time pill: `{turns} turns {steps} steps`, plus the streaming rate
    /// once a turn reports one, behind the gauge glyph.
    fn stat_time_pill(
        &self,
        size: u32,
        turns: usize,
        steps: usize,
        lang: Lang,
    ) -> impl IntoElement {
        let counts = tr_args(
            lang,
            "stats.counts",
            &[("turns", turns.to_string()), ("steps", steps.to_string())],
        );
        let mut label = div()
            .flex()
            .items_center()
            .min_w_0()
            .child(self.stat_text(size, counts));
        if let Some(rate) = self.state.tokens_per_second() {
            label = label.child(self.stat_sep(size)).child(self.stat_text(
                size,
                tr_args(
                    lang,
                    "message.tokensPerSecond",
                    &[("tps", rate.to_string())],
                ),
            ));
        }
        self.stat_pill(size, IconName::CircleGauge, label)
            .debug_selector(|| "status-time-pill".to_string())
    }

    /// The usage pill: the compact total and the session-wide cache hit over
    /// the database glyph. This is the token dialog's trigger.
    fn stat_usage_pill(
        &self,
        size: u32,
        summary: UsageSummary,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = tr_args(
            lang,
            "message.turnUsage.count",
            &[("count", human_tokens(summary.total()))],
        );
        let mut label = div()
            .flex()
            .items_center()
            .min_w_0()
            .child(self.stat_text(size, count));
        if let Some(percent) = cache_hit_percent(summary.cache_read, summary.billed_input()) {
            label = label.child(self.stat_sep(size)).child(self.stat_text(
                size,
                tr_args(lang, "stats.cacheHit", &[("percent", percent)]),
            ));
        }
        let open = self.state.stat_open;
        let pill = self
            .stat_pill(size, IconName::Database, label)
            .debug_selector(|| "status-usage-pill".to_string())
            .id("status-usage-pill")
            .cursor_pointer()
            .when(open, |pill| pill.bg(rgb(theme::hover())))
            .hover(|pill| pill.bg(rgb(theme::hover())))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.state.toggle_stat();
                cx.notify();
            }));
        // The panel rides beside the pill in a relative wrapper: the stat row is
        // the last child of the centre column, so an absolute panel paints over
        // the composer rather than under it.
        div().relative().child(pill).when(open, |anchor| {
            anchor.child(self.stat_panel(size, summary, lang))
        })
    }

    /// The measured token dialog, drawn 8px above the pill that opened it.
    fn stat_panel(&self, size: u32, summary: UsageSummary, lang: Lang) -> impl IntoElement {
        let title = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(theme::STAT_TITLE_GAP))
            .mb(px(theme::STAT_TITLE_MB))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::STAT_PILL_GAP))
                    .min_w_0()
                    .child(
                        icon(IconName::Database, theme::STAT_TITLE_ICON, theme::text())
                            .line_height(px(theme::STAT_PANEL_LINE)),
                    )
                    .child(txt(
                        crate::i18n::tr(lang, "stats.dialog.usageTitle"),
                        theme::STAT_PANEL_SIZE,
                        theme::text(),
                    )),
            )
            .child(txt(
                tr_args(
                    lang,
                    "message.turnUsage.count",
                    &[("count", exact_tokens(summary.total()))],
                ),
                theme::STAT_PANEL_SIZE,
                theme::text(),
            ))
            .font_weight(FontWeight::MEDIUM);
        let mut details = div().flex().flex_col().gap(px(theme::STAT_GRID_GAP_Y));
        if let Some(percent) = cache_hit_percent(summary.cache_read, summary.billed_input()) {
            details = details.child(self.stat_row(
                lang,
                "message.turnUsage.cacheHit",
                format!("{percent}%"),
            ));
        }
        // The reference hides its cache-write row when the count is zero, and
        // bsh's providers report no such bucket, so that row never appears.
        for (key, value) in [
            ("message.turnUsage.input", summary.uncached_input),
            ("message.turnUsage.cacheRead", summary.cache_read),
            ("message.turnUsage.output", summary.output),
        ] {
            details = details.child(self.stat_row(
                lang,
                key,
                tr_args(
                    lang,
                    "message.turnUsage.count",
                    &[("count", exact_tokens(value))],
                ),
            ));
        }
        let panel = div()
            .debug_selector(|| "status-panel".to_string())
            .absolute()
            .bottom(px(theme::stat_line_at(size)
                + theme::STAT_PILL_PAD_Y * 2.0
                + theme::STAT_PANEL_GAP))
            .left(px(0.0))
            .w(px(theme::STAT_PANEL_MIN_W))
            .max_w(px(theme::STAT_PANEL_MAX_W))
            .p(px(theme::STAT_PANEL_PAD))
            .rounded(px(theme::STAT_PANEL_RADIUS))
            .bg(rgb(theme::page()))
            .shadow_lg()
            .text_size(px(theme::STAT_PANEL_SIZE))
            .line_height(px(theme::STAT_PANEL_LINE))
            .text_color(rgb(theme::muted()))
            .occlude()
            .child(title)
            .child(
                div()
                    .w_full()
                    .h(px(0.5))
                    .mb(px(theme::STAT_RULE_MB))
                    .bg(rgb(theme::border_soft())),
            )
            .child(details);
        hairline_all(panel).border_color(rgb(theme::border_soft()))
    }

    /// One detail row: a 76px-minimum label column beside a right-aligned value.
    fn stat_row(&self, lang: Lang, key: &'static str, value: String) -> impl IntoElement {
        div()
            .debug_selector(move || format!("status-row-{key}"))
            .flex()
            .items_center()
            .w_full()
            .gap(px(theme::STAT_GRID_GAP_X))
            .child(
                div()
                    .min_w(px(theme::STAT_GRID_LABEL_MIN))
                    .flex_shrink_0()
                    .child(txt(
                        crate::i18n::tr(lang, key),
                        theme::STAT_PANEL_SIZE,
                        theme::faint(),
                    )),
            )
            .child(div().flex_1().min_w_0().flex().justify_end().child(txt(
                value,
                theme::STAT_PANEL_SIZE,
                theme::muted(),
            )))
    }
}
