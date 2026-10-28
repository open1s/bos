//! The message composer: input, capability chips, and send/stop.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::Disableable as _;
use gpui_kit::*;

use std::sync::Arc;

use super::shell::Shell;
use super::todo_panel::spinner;
use super::{hairline_all, icon, txt, DivExt, IconName};
use crate::theme;

/// The glyph colour on the filled primary control.
///
/// The reference's send/stop button is the one place its CSS hardcodes a
/// colour (`.primary{color:#fff}`), so this is a literal and not a palette
/// lookup: `theme::page()` is white in light and near-black in dark.
const ON_ACCENT: u32 = 0xffffff;

/// The reference's select-slot chip, measured from `PermissionSelect.module.css`
/// (`wXeviG_*`), `ModelSelect.module.css` (`cl2Rlq_*`) and the permission JSX:
/// `.trigger{height:28px;border-radius:var(--dsw-radius-sm);gap:4px;
/// padding:0 4px 0 8px;font-size:13px;line-height:20px}` around an ellipsised
/// label and a chevron in `label-caption`.
const CHIP_H: f32 = 28.0;
const CHIP_PAD_L: f32 = 8.0;
const CHIP_PAD_R: f32 = 4.0;
const CHIP_GAP: f32 = 4.0;
const CHIP_RADIUS: f32 = 8.0;
/// `PermissionSelect` caps its trigger at 220px; `ModelSelect` at
/// `min(360px,45cqw)`, which at bsh's card is the 360 side.
const CHIP_CAP: f32 = 220.0;
const MODEL_CHIP_CAP: f32 = 360.0;
/// The modules size their own icon box at 14px (`triggerIcon svg`); the chevron
/// comes from the same icon primitives, so it takes the same box.
const CHIP_CHEVRON: f32 = 14.0;

/// The reference's draft-attachment tiles, measured from
/// `AttachmentRail.module.css` and `ComposerAttachments.module.css`: a 64px
/// square at radius-xl inside a rail whose items are 10px apart, inset 10px a
/// side with a 2px top pad and pulled 6px into the editor's gap.
const ATTACHMENT_TILE: f32 = 64.0;
const ATTACHMENT_GAP: f32 = 10.0;
const ATTACHMENT_PAD: f32 = 10.0;
const ATTACHMENT_TOP: f32 = 2.0;
const ATTACHMENT_PULL: f32 = -6.0;
/// `.remove{width:18px;height:18px}` at `top:4px;right:4px`.
const ATTACHMENT_REMOVE: f32 = 18.0;
const ATTACHMENT_REMOVE_INSET: f32 = 4.0;

/// What the model chip needs to name the model and fill its menu: the label it
/// shows, the effort suffix the trigger appends, the active id, the catalog rows
/// the menu offers, and whether a turn is running (its mark swaps for the dot).
struct ModelChipData {
    label: String,
    effort: Option<String>,
    /// The level the Effort row shows, or `Default` when the model decides.
    effort_level: Option<String>,
    current: String,
    groups: Vec<(String, Vec<String>)>,
    streaming: bool,
}

/// The effort levels the model menu offers, in the reference's own shape: a
/// "default" that hands the decision back to the model, then Off, then rising
/// levels. `None` is that default; `off` is a level of its own, because the
/// reference separates "Off turns reasoning off" from "the model decides".
pub(crate) const EFFORT_LEVELS: [Option<&str>; 5] =
    [None, Some("off"), Some("low"), Some("medium"), Some("high")];

/// The permission modes the chip's menu offers, in the reference's own order.
///
/// The reference's `PRESET_LABEL_KEYS` map is read-only, then
/// workspace-write, then full access, so the menu draws that order and every
/// other surface reads the same three modes. The chip and the pane both write
/// `Settings.permission`, which is the one field `caps::build_agent` obeys.
pub(crate) fn permission_modes() -> [(&'static str, &'static str); 3] {
    super::permission::MODES.map(|mode| (mode.value(), mode.label_key()))
}

/// The measured rule for the primary control: it stops only while a turn runs
/// AND the draft is empty (`primaryStops = running && subagent === null &&
/// (empty || blocked)`). With text in the draft the reference sends - which for
/// bsh is the queue path the dock already shows.
pub(crate) fn primary_stops(streaming: bool, can_send: bool) -> bool {
    streaming && !can_send
}

/// The measured rule for the primary control's enabled look.
///
/// The reference's `empty = draft.trim() === "" && attachments.length === 0`,
/// so an image with no words is still sendable and the arrow stays live.
pub(crate) fn can_send(draft: &str, attachments: usize) -> bool {
    !draft.trim().is_empty() || attachments > 0
}

/// The composer's placeholder, fitted to the field's own content width.
///
/// The reference paints its placeholder as a nowrap span with
/// `text-overflow:ellipsis` inset into the card, so a narrow field shortens the
/// text instead of letting the card cut it. The kit's field paints its
/// placeholder with no ellipsis hook, so the string handed to the state is
/// fitted here: the full text while it fits, otherwise the longest prefix that
/// leaves room for the reference's ellipsis.
pub(crate) fn fit_placeholder(window: &Window, size: f32, text: &str, width: f32) -> String {
    if width <= 0.0 || text.is_empty() {
        return text.to_string();
    }
    let measure = |line: &str| -> f32 {
        if line.is_empty() {
            return 0.0;
        }
        let run = window.text_style().to_run(line.len());
        f32::from(
            window
                .text_system()
                .shape_line(line.to_string().into(), px(size), &[run], None)
                .width(),
        )
    };
    if measure(text) <= width {
        return text.to_string();
    }
    // Binary search the character count: shaping is the measurement, and a
    // placeholder is a few dozen characters, so this is at most seven shapes -
    // and only when the field's width or the language changes, never a frame.
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "\u{2026}";
        if measure(&candidate) <= width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut fitted: String = chars[..lo].iter().collect();
    fitted.push('\u{2026}');
    fitted
}

/// The measured trigger chrome the permission and model chips share.
fn trigger_chrome<E: Styled>(element: E, cap: f32) -> E {
    element
        .h(px(CHIP_H))
        .pl(px(CHIP_PAD_L))
        .pr(px(CHIP_PAD_R))
        .rounded(px(CHIP_RADIUS))
        .gap(px(CHIP_GAP))
        .max_w(px(cap))
}

/// The measured `.triggerLabel`: `text-overflow:ellipsis;white-space:nowrap;
/// min-width:0;overflow:hidden`, so a long mode or model name truncates before
/// it can push the chevron out of the chip.
fn chip_label(label: impl Into<SharedString>, weight: FontWeight) -> Div {
    txt(label, 13.0, theme::muted())
        .font_weight(weight)
        .line_height(px(20.0))
        .min_w_0()
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
}

/// The chip's text: the mode in force, in plain words.
///
/// The effective value is the one the tools obey. A preset ceiling is a
/// consequence the notice line reports when it acts, not a permanent second
/// label the reader has to decode; the collapsed preset card carries it too.
pub(crate) fn permission_chip_text(
    lang: crate::i18n::Lang,
    permission: &crate::settings::resolve::PermissionResolution,
) -> String {
    super::permission::chip_text(lang, permission)
}

/// Whether one menu row carries the checkmark: the mode the tools obey.
///
/// It used to follow the reader's REQUEST, which put the check on Full access
/// while the chip and the build were confined. Two controls answering "what may
/// this session do" must not disagree.
#[cfg(test)]
pub(crate) fn permission_choice_checked(
    permission: &crate::settings::resolve::PermissionResolution,
    value: &str,
) -> bool {
    super::permission::Mode::from_permission(&permission.effective).value() == value
}

impl Shell {
    /// Write the permission mode the composer chip offers, and persist it now.
    ///
    /// The turn build reads the COMMITTED settings, so writing only the draft
    /// made the chip look inert: the label changed and the next request did
    /// not. This commits exactly `workspace.agent.permission` (never any other
    /// unsaved draft field), saves it, and rebakes the cached policy so the
    /// next turn - and the policy pane - see the new mode without a Save.
    ///
    /// A failed write is reported in the notice line and the in-memory value is
    /// put back: the UI must not claim a mode the file does not hold.
    pub(crate) fn set_permission(&mut self, permission: &str, cx: &mut Context<Self>) {
        let lang = self.state.lang;
        match self.state.commit_permission(permission) {
            Ok(()) => {
                self.rebake_after_permission(cx);
                // The ceiling is a refusal the reader has to see: annotate the
                // chip (it reads the same resolution) and say it in the notice
                // line, instead of silently landing on the lower mode.
                let resolution = self.state.effective().permission;
                let mut message = None;
                if let Some(ceiling) = resolution.reduced_by.as_deref() {
                    message = Some(crate::i18n::tr_args(
                        lang,
                        "permission.ceilingNotice",
                        &[
                            ("requested", lang.tr(permission_label_key(permission))),
                            ("ceiling", lang.tr(permission_label_key(ceiling))),
                            (
                                "effective",
                                lang.tr(permission_label_key(&resolution.effective)),
                            ),
                        ],
                    ));
                }
                // A turn that is ALREADY streaming was built with the old
                // permission (the sandbox spec and the tool set are baked at
                // spawn). Say so where the chip was pressed, instead of
                // letting the new label imply the running turn changed.
                if let Some(id) = self.state.active_id.clone() {
                    if self.state.is_streaming(&id) {
                        let next = crate::i18n::tr(lang, "permission.nextTurn").to_string();
                        message = Some(match message {
                            Some(existing) => format!("{existing} {next}"),
                            None => next,
                        });
                    }
                }
                if let Some(message) = message {
                    self.state.notice(message);
                }
            }
            Err(error) => {
                self.state.notice(format!(
                    "{} {error}",
                    crate::i18n::tr(lang, "permission.saveFailed")
                ));
            }
        }
        cx.notify();
    }

    /// Select a permission mode, opening the risk gate when the mode needs one.
    ///
    /// The gate is kept for Full access only, and only when Full access will
    /// actually take effect: a preset ceiling that ranks below it caps the
    /// result, so there is no widening to confirm. In that case the request is
    /// applied directly and the ceiling notice explains why the session stayed
    /// lower - the same notice, and the same wording, as the composer chip.
    ///
    /// The no-op guard compares BOTH trees: a draft that already holds the
    /// value while the committed tree does not is the old bug's state, where an
    /// early return would skip the very write that makes the mode real.
    pub(crate) fn request_permission(&mut self, permission: &str, cx: &mut Context<Self>) {
        let committed = self.state.settings.workspace.agent.permission == permission;
        let drafted = self.state.draft().workspace.agent.permission == permission;
        if committed && drafted {
            return;
        }
        if permission == crate::caps::FULL_ACCESS && !committed {
            // The gate is the only route that widens a session to Full access,
            // and it cannot be skipped by picking the mode from another control.
            // It is also pointless when the preset ceiling already caps the
            // result: nothing widens, so confirming a widening that cannot
            // happen would be a lie. Read the ceiling from the same resolution
            // the chip shows, then let the applied value and its notice speak.
            let ceiling = self.state.effective().permission.preset_ceiling;
            let widened = crate::settings::resolve::PermissionResolution::resolve(
                permission,
                ceiling.as_deref(),
            );
            if widened.reduced_by.is_some() {
                self.set_permission(permission, cx);
                return;
            }
            // ONE confirmation, one sentence, two buttons: the gate names the
            // consequence and Cancel leaves the current mode untouched. There is
            // no second dialog and no separate acknowledgement step.
            self.permission_confirm = Some(permission.to_string());
            cx.notify();
            return;
        }
        self.set_permission(permission, cx);
    }

    /// Rebuild the capability list and the cached policy from the committed
    /// settings, so the next turn and the panes read the mode just chosen.
    fn rebake_after_permission(&mut self, cx: &mut Context<Self>) {
        self.caps.clear();
        self.plugins.clear();
        let choice = self
            .state
            .active_id
            .as_deref()
            .map(|id| self.state.session_choice(id))
            .unwrap_or_default();
        let built = crate::caps::build_agent(
            &self.state.settings,
            &self.state.vault,
            &choice,
            &self.state.mcp,
        );
        self.state.set_tools_schema(built.tools_schema_chars);
        self.state.set_policy(built.policy.clone());
        cx.notify();
    }
}

/// The label the model chip names, with the reference's own fallback
/// (`trigger.fallback`) when no model is resolved.
pub(crate) fn model_label(model: &str, lang: crate::i18n::Lang) -> String {
    if model.trim().is_empty() {
        lang.tr("composer.selectModel")
    } else {
        model.to_string()
    }
}

/// The reference's `.triggerEffort` suffix: the reasoning effort the model
/// carries, only when it has one (a blank one is not a level).
pub(crate) fn effort_suffix(effort: Option<&str>) -> Option<String> {
    effort
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(str::to_string)
}

/// The preset chip's label.
///
/// The master switch wins over everything: with presets off, NO preset is what
/// every chat runs, so "No preset" is the honest report even while a selection
/// is retained for the switch to restore. With presets on the effective id is
/// shown - annotated with the sandbox refusal where the guard would reject it -
/// and an empty id is the reference's "Default (no preset)".
pub(crate) fn preset_chip_label(
    lang: crate::i18n::Lang,
    effective: &str,
    presets_disabled: bool,
    sandbox_widens: bool,
) -> String {
    if presets_disabled {
        return lang.tr("input.noPreset").to_string();
    }
    if effective.trim().is_empty() {
        return lang.tr("input.defaultPreset").to_string();
    }
    if sandbox_widens {
        return format!(
            "{effective} · {}",
            lang.tr("settings.presetSandboxWidensAction")
        );
    }
    effective.to_string()
}

/// The Effort row's own label: "Effort" with the current level after it, so the
/// root menu answers "what is it set to" without opening the submenu.
pub(crate) fn effort_row_label(lang: crate::i18n::Lang, level: Option<&str>) -> String {
    format!(
        "{} · {}",
        lang.tr("composer.menuEffort"),
        effort_level_label(lang, level)
    )
}

/// One effort level's wording. `None` is the model's own decision, which the
/// reference separates from the explicit Off.
pub(crate) fn effort_level_label(lang: crate::i18n::Lang, level: Option<&str>) -> String {
    match level {
        None => lang.tr("composer.effortDefault"),
        Some("off") => lang.tr("composer.effortOff"),
        Some("low") => lang.tr("composer.effortLow"),
        Some("medium") => lang.tr("composer.effortMedium"),
        Some("high") => lang.tr("composer.effortHigh"),
        Some(other) => other.to_string(),
    }
}
/// The models the model chip's menu offers - the active provider's catalog, the
/// same list the Models pane draws - each marked when it is the active model.
pub(crate) fn model_choices(models: &[String], current: &str) -> Vec<(String, bool)> {
    models
        .iter()
        .map(|model| (model.clone(), model == current))
        .collect()
}

/// The locale key for the permission mode the tools actually obey.
///
/// The same value the General pane's choice row writes, and the reference's own
/// `PRESET_LABEL_KEYS` fallback for a value it does not know: an unrecognised
/// mode names READ ONLY, because that is the tier
/// `PermissionResolution::resolve` normalises it to and the chip must not
/// claim more access than the tools have.
pub(crate) fn permission_label_key(permission: &str) -> &'static str {
    super::permission::Mode::from_permission(permission).label_key()
}

impl Shell {
    /// The composer card under the conversation.
    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let streaming = self.state.any_streaming();
        let sendable = can_send(&self.composer_value, self.state.attachments.len());
        // One resolution per frame: `effective()` clones a whole resolved
        // workspace, and the chip asks three separate facts of it. The
        // resolution is the ACTIVE SESSION's choice, which is what the menu
        // checks and what the next turn builds; a chat that chose nothing shows
        // the reference's own "Select model" fallback.
        let effective = self.state.effective();
        // The chip names the mode the tools OBEY: the session's request capped
        // by the preset ceiling, resolved once above and annotated when the
        // ceiling lowered it.
        // The chip's rows are built once, from this resolution and the live
        // egress facts: the label, the rows and the policy cannot describe
        // different turns. The workspace file's state is the one AppState cached
        // (never read from disk per frame).
        let permission_rows = super::permission::permission_rows(lang, &effective);
        let permission = effective.permission.clone();
        let label = effective.model_name;
        // The menu rows are nicknames, so the checked row compares against the
        // nickname rather than the API id the wire carries.
        let current = effective
            .model_ref
            .as_ref()
            .map(|reference| reference.model.clone())
            .unwrap_or_default();
        let effort = effort_suffix(effective.reasoning_effort.as_deref());
        // The menu offers the same list the Models pane draws, grouped by
        // provider, with the provider the choice in effect names leading. The
        // draft is the tree the pane edits, so a provider added but not yet
        // saved is still offered.
        let groups = self.state.draft().catalogs_by_provider(&effective.provider);
        // The reference's composer row carries a container query: at 560 or
        // less of card width its gaps tighten from 12 to 8, which is what lets
        // the chips stay on one line a little longer before the row wraps.
        let chip_gap = if self.composer_w() < 560.0 { 8.0 } else { 12.0 };
        // The inline note editor sits ABOVE the card, so the composer keeps its
        // shape while a goal or a feedback note is being written.
        let column = div()
            .pb(px(4.0))
            .flex()
            .flex_col()
            .children(self.note_row(cx))
            .child({
                // Measured: radius-panel 28 over the input-major fill (white in light,
                // the same as the page) lifted by `elevation-soft` - a stroke plus two
                // very soft blurs - with a 12px column gap and an 8px top pad. GPUI's
                // shadow carries no ring, so the stroke is drawn as a `.5px` hairline.
                let card = div()
                    .debug_selector(|| "composer-card".to_string())
                    .relative()
                    .w_full()
                    .rounded(px(28.0))
                    .bg(rgb(theme::page()))
                    .shadow_sm()
                    .pt(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .children(self.attachment_row(cx))
                    .child(
                        // Measured: the input sits in a `.scroll` wrapper that gives
                        // up 4px on the right and clips at
                        // `--dsh-composer-text-max-height`, over the input's own
                        // 4/8/0/14 padding and its 36px floor. The kit's field draws
                        // 8px a side of that padding itself, so the wrapper carries
                        // the difference and the floor.
                        div()
                            .debug_selector(|| "composer-editor".to_string())
                            .id("composer-editor")
                            .mr(px(theme::COMPOSER_INPUT_MR))
                            .pl(px(theme::COMPOSER_INPUT_PAD_L - theme::FIELD_INTERIOR_PAD))
                            .pr(px(theme::COMPOSER_INPUT_PAD_R - theme::FIELD_INTERIOR_PAD))
                            .pt(px(0.0))
                            .min_h(px(theme::COMPOSER_INPUT_MIN_H))
                            .text_size(px(theme::body_size_at(
                                self.state.draft().global.general.content_font_size,
                            )))
                            .max_h(px(theme::COMPOSER_INPUT_MAX_H))
                            .overflow_y_scroll()
                            .child(
                                // The reference draws an `@path` as a chip, not as
                                // plain text: the token stays one run for editing and
                                // undo, and only its paint is ours. A press opens the
                                // file in the right pane, the way the reference's
                                // `FileReference` opens it.
                                Textarea::new(&self.composer)
                                    .appearance(false)
                                    .token(|context, _window, _cx| {
                                        super::reference_chip::chip(context)
                                    })
                                    .on_token_click(cx.listener(|this, event, window, cx| {
                                        this.open_reference_chip(event, window, cx);
                                    }))
                                    // Measured: the reference's editor takes files
                                    // and images from the clipboard before the
                                    // field sees text. The kit hands the payload
                                    // here for exactly that; a paste the shell does
                                    // not take falls through to the kit's own text
                                    // insertion.
                                    .on_paste({
                                        let shell = cx.entity().downgrade();
                                        move |item, window, cx| {
                                            shell
                                                .update(cx, |this, cx| {
                                                    this.paste_clipboard(item, window, cx)
                                                })
                                                .unwrap_or(false)
                                        }
                                    }),
                            ),
                    )
                    .child(
                        // The reference's row is flex-wrap: wrap with a
                        // container query that tightens its gaps below 560, and
                        // its trailing pair is pushed right by margin-left: auto.
                        // A flat no-wrap row pushed the send control past the
                        // card once the column got narrow; wrapping keeps every
                        // control reachable instead of drawing it over the edge.
                        div()
                            .debug_selector(|| "composer-row".to_string())
                            .px(px(8.0))
                            .pt(px(2.0))
                            .pb(px(6.0))
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(chip_gap))
                            .child(
                                div()
                                    .debug_selector(|| "composer-row-tools".to_string())
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .min_w_0()
                                    .gap(px(chip_gap))
                                    .child(self.add_button(
                                        lang.tr("input.commands"),
                                        super::command_menu::toggle_action,
                                        cx,
                                    ))
                                    // The reference's plan slot sits beside the
                                    // mode chips, and only while the mode is on.
                                    .child(self.permission_chip(
                                        permission,
                                        permission_rows,
                                        streaming,
                                        lang,
                                        cx,
                                    ))
                                    .when_some(self.plan_chip(lang, cx), |row, chip| {
                                        row.child(chip)
                                    }),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "composer-row-trailing".to_string())
                                    .ml_auto()
                                    .flex_none()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(chip_gap))
                                    .child(self.model_chip(
                                        ModelChipData {
                                            label: model_label(&label, lang),
                                            effort: effort.clone(),
                                            effort_level: effort,
                                            current,
                                            // Moved, not cloned: the menu owns the
                                            // list, and the chip does not read it
                                            // again.
                                            groups,
                                            streaming,
                                        },
                                        lang,
                                        cx,
                                    ))
                                    .child(self.send_button(streaming, sendable, lang, cx)),
                            ),
                    )
                    // Measured `.card{position:absolute;bottom:calc(100% + 4px);
                    // left:0;right:0}`: the overlay is out of the card's flow and
                    // spans the input it belongs to. One overlay at a time: the
                    // slash palette while a leading slash is being typed, otherwise
                    // the plus card.
                    .when_some(self.composer_overlay(cx), |card, menu| card.child(menu));
                hairline_all(card).border_color(rgb(theme::border_soft()))
            });
        // The context meter moved into the status row, at its left end, so the
        // reading sits with the other figures about the same turn instead of on
        // a line of its own under the card.
        column
    }

    /// The reference's `.add`: a fully round 28x28 `+` on the specific
    /// selector fill, its 14px glyph in label-primary, hovering to the
    /// interactive fill, tooltipped with the same measured label after 500ms.
    ///
    /// `.add{background:var(--dsw-specific-selector);width:28px;height:28px;
    /// border-radius:999px;display:grid;place-items:center}` with a 14px
    /// `IconPlusOutlineMedium` child and `aria-label: input.commands`. The
    /// reference's own `+` is `onToggleCommandMenu`: its press opens the
    /// measured command card over the composer, whose first row is the file
    /// picker this control used to open directly.
    fn add_button(
        &self,
        label: impl Into<SharedString>,
        action: fn(&mut Shell, &mut Window, &mut Context<Shell>),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tooltip = label.into();
        div()
            .debug_selector(|| "composer-add".to_string())
            .id("composer-add")
            .w(px(28.0))
            .h(px(28.0))
            .flex_none()
            .rounded_full()
            .bg(rgb(theme::new_session_fill()))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .child(icon(IconName::Plus, 14.0, theme::text()))
            .on_click(cx.listener(move |this, _event, window, cx| {
                action(this, window, cx);
            }))
    }

    /// The reference's model trigger and the menu it opens.
    ///
    /// Its JSX is \`[triggerIcon?] [triggerLabel] [triggerEffort?] [dot|chevron]\`,
    /// and the icon sits behind \`display:var(--dsh-composer-model-icon-display,
    /// none)\` - hidden unless the host turns it on - so bsh draws none. The menu
    /// is the reference's \`menu.model\` cell, whose list is the active provider's
    /// catalog (the same list the Models pane draws) and whose rows check the
    /// active model. Owed: the provider grouping and the search row, because bsh's
    /// directory is a local per-provider catalog rather than an async fetch; the
    /// \`menu.effort\` cell, because \`reasoning_effort\` is a free string with no
    /// level list to open - the trigger does show its suffix; and the root cells'
    /// trailing value, because a kit submenu item carries only its label.
    fn model_chip(
        &self,
        data: ModelChipData,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ModelChipData {
            label,
            effort,
            effort_level,
            current,
            groups,
            streaming,
        } = data;
        let shell = cx.entity();
        let mut chip = trigger_chrome(
            Button::new("composer-model")
                .ghost()
                .flex()
                .items_center()
                .cursor_pointer(),
            MODEL_CHIP_CAP,
        )
        .child(chip_label(label, FontWeight::NORMAL));
        if let Some(effort) = effort {
            // \`.triggerEffort\` is label-caption, ellipsised, and gives up its
            // space before the label does (\`flex-shrink:1000\`).
            chip = chip.child(
                div()
                    .min_w_0()
                    .flex_shrink(1000.0)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .text_color(rgb(theme::caption()))
                    .child(effort),
            );
        }
        let chip = chip.child(if streaming {
            // The reference's mark is `busy ? StateDot{state:"ongoing"} :
            // IconChevronDownOutlineRegular`, and its trigger stays enabled while
            // a turn runs - only the option rows carry `disabled: busy`.
            div()
                .debug_selector(|| "composer-model-busy".to_string())
                .child(spinner())
                .into_any_element()
        } else {
            icon(IconName::ChevronDown, CHIP_CHEVRON, theme::caption())
                .debug_selector(|| "composer-model-chevron".to_string())
                .into_any_element()
        });
        div()
            .debug_selector(|| "composer-model".to_string())
            .child(
                chip.dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, window, cx| {
                    // The kit's builder is \`Fn\`, so every capture is cloned into
                    // the submenu's own closure.
                    let groups = groups.clone();
                    let current = current.clone();
                    let shell = shell.clone();
                    // The Effort submenu is a second closure inside an `Fn`
                    // builder, so it needs its own copies of both captures.
                    let effort_shell = shell.clone();
                    let effort_row = effort_level.clone();
                    menu.submenu(
                        lang.tr("composer.menuModel"),
                        window,
                        cx,
                        move |submenu, _window, _cx| {
                            // The measured \`.menu\` box: \`min-width:min(240px,
                            // 100vw - 32px)\`, \`max-width:min(420px, 100vw - 32px)\`.
                            let submenu = submenu.min_w(px(240.0)).max_w(px(420.0));
                            if groups.is_empty() {
                                let text = lang.tr("composer.noModels");
                                return submenu.item(PopupMenuItem::element(
                                    move |_window, _cx| {
                                        // \`.empty\` is 12px/18px in label-tertiary
                                        // over an 8px pad.
                                        txt(text.clone(), 12.0, theme::faint())
                                            .px(px(8.0))
                                            .py(px(8.0))
                                            .into_any_element()
                                    },
                                ));
                            }
                            let mut submenu = submenu;
                            for (provider, models) in groups.iter() {
                                let heading = provider.clone();
                                submenu =
                                    submenu.item(PopupMenuItem::element(move |_window, _cx| {
                                        txt(heading.clone(), 11.0, theme::faint())
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .into_any_element()
                                    }));
                                for (model, active) in model_choices(models, &current) {
                                    let pick = shell.clone();
                                    let name = model.clone();
                                    let owner = provider.clone();
                                    submenu = submenu.item(
                                        PopupMenuItem::new(model).checked(active).on_click(
                                            move |_, _window, cx| {
                                                pick.update(cx, |this, cx| {
                                                    this.state.select_model_from(&owner, &name);
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    );
                                }
                            }
                            submenu
                        },
                    )
                    // The reference's second root cell: the reasoning level, with
                    // the current one named on the row itself because a kit submenu
                    // item carries only its label.
                    .submenu(
                        effort_row_label(lang, effort_level.as_deref()),
                        window,
                        cx,
                        move |submenu, _window, _cx| {
                            let mut submenu = submenu.min_w(px(200.0));
                            for level in EFFORT_LEVELS {
                                let pick = effort_shell.clone();
                                let active = effort_row.as_deref() == level;
                                submenu = submenu.item(
                                    PopupMenuItem::new(effort_level_label(lang, level))
                                        .checked(active)
                                        .on_click(move |_, _window, cx| {
                                            pick.update(cx, |this, cx| {
                                                this.state.set_reasoning_effort(level);
                                                cx.notify();
                                            });
                                        }),
                                );
                            }
                            submenu
                        },
                    )
                    .min_w(px(240.0))
                }),
            )
    }

    /// The reference's permission trigger: the same select chrome plus the
    /// `.chevron` in `label-caption` that opens the mode menu.
    ///
    /// Its JSX renders `[triggerIcon?] [triggerLabel] [badge?] [chevron]` and
    /// disables the chip while `busy || !writable || options.length === 0` with
    /// `color:label-dimmed;cursor:default` - the measured reason the mode cannot
    /// be switched mid-turn. Owed: the 14px `triggerIcon` per-preset glyph and
    /// the Auto badge (`sup`, 8px/600, `EXP`), because bsh's two modes are not
    /// the reference's presets, and `label-dimmed` (the kit's own disabled tint
    /// stands in).
    /// DSH enables an agent preset for one conversation, not only globally.
    ///
    /// The chip shows the session's own preset - or "Default" when it inherits
    /// the global one - and its menu writes Session::agent_preset, which the
    /// next turn's build reads through Session::choice. A running turn stays
    /// enabled: the pick applies to the next turn, like the model chip.
    pub(crate) fn agent_preset_chip(
        &self,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let explicit = self.state.active_session_preset();
        // Show the preset the turn ACTUALLY runs, not only the one this chat
        // named: a chat that inherits the global default displays that preset,
        // and the menu's "Default" row still records the inheritance.
        let resolved = self
            .state
            .active_id
            .as_deref()
            .map(|id| self.state.effective_in(id))
            .unwrap_or_else(|| self.state.effective());
        let effective = resolved.preset_id;
        let presets_disabled = resolved.presets_disabled;
        // A session that recorded a preset the sandbox guard now refuses must
        // say so at the chip, not only when the next send is rejected.
        let refused_effective =
            !effective.trim().is_empty() && self.state.preset_sandbox_widens(&effective);
        let label = preset_chip_label(lang, &effective, presets_disabled, refused_effective);
        let listed: Vec<(String, bool)> = self
            .state
            .settings
            .global
            .preset_ids()
            .into_iter()
            .map(|name| {
                let widens = self.state.preset_sandbox_widens(&name);
                (name, widens)
            })
            .collect();
        let refused_label = lang.tr("settings.presetSandboxWidensAction").to_string();
        let session = self.state.active_id.clone().unwrap_or_default();
        let shell = cx.entity();
        let chip = trigger_chrome(
            Button::new("hero-preset")
                .ghost()
                .flex()
                .items_center()
                .cursor_pointer()
                .border_1()
                .border_color(rgb(theme::border())),
            CHIP_CAP,
        )
        .child(icon(IconName::Bot, 14.0, theme::muted()))
        .child(chip_label(label, FontWeight::MEDIUM))
        .child(
            icon(IconName::ChevronDown, CHIP_CHEVRON, theme::caption())
                .debug_selector(|| "agent-preset-chevron".to_string()),
        );
        div()
            .debug_selector(|| "agent-preset".to_string())
            .child(
                chip.dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let mut menu = menu;
                    let pick_default = shell.clone();
                    let session_default = session.clone();
                    let current = explicit.clone();
                    // With presets off the menu states WHY it cannot record a
                    // choice instead of offering rows that would change nothing
                    // visible: the selection is kept, and the switch that turns
                    // it back on lives in the Agent presets pane.
                    if presets_disabled {
                        menu = menu
                            .item(PopupMenuItem::new(lang.tr("input.presetsOff")).disabled(true));
                    }
                    menu = menu.item(
                        PopupMenuItem::new(lang.tr("input.defaultPreset"))
                            .checked(current.is_none())
                            .disabled(presets_disabled)
                            .on_click(move |_, _window, cx| {
                                pick_default.update(cx, |this, cx| {
                                    this.state.select_session_preset(&session_default, "");
                                    cx.notify();
                                });
                            }),
                    );
                    for (name, widens) in listed.iter() {
                        let pick = shell.clone();
                        let sid = session.clone();
                        let picked = name.clone();
                        let chosen = explicit.as_deref() == Some(name.as_str());
                        if presets_disabled {
                            menu = menu.item(
                                PopupMenuItem::new(name.clone())
                                    .checked(chosen)
                                    .disabled(true),
                            );
                        } else if *widens {
                            // Disabled AND labelled: a click cannot record a
                            // preset the send gate would refuse, and the reason
                            // is visible where the choice is made.
                            menu = menu.item(
                                PopupMenuItem::new(format!("{name} · {refused_label}"))
                                    .checked(chosen)
                                    .disabled(true),
                            );
                        } else {
                            menu = menu.item(
                                PopupMenuItem::new(name.clone()).checked(chosen).on_click(
                                    move |_, _window, cx| {
                                        pick.update(cx, |this, cx| {
                                            this.state.select_session_preset(&sid, &picked);
                                            cx.notify();
                                        });
                                    },
                                ),
                            );
                        }
                    }
                    menu
                }),
            )
    }

    fn permission_chip(
        &self,
        permission: crate::settings::resolve::PermissionResolution,
        choices: Vec<super::permission::ModeChoice>,
        streaming: bool,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let shell = cx.entity();
        // The text is computed once, outside the menu builder: the kit's
        // builder is `Fn`, so every capture is cloned into it anyway.
        let text = permission_chip_text(lang, &permission);
        let effective = permission.effective.clone();
        // Three rows, each one line: the label and a short qualifier, so the
        // modes can be compared at a glance. The check follows the mode in
        // force, and a row a preset ceiling cannot grant is disabled and states
        // which mode the preset keeps. The sandbox state is not drawn here: it
        // is already in the Settings switch row, the terminal status line and
        // each call's audit, and a note in this menu was the wordy copy the
        // owner asked to remove.
        let chip = trigger_chrome(
            Button::new("composer-permission")
                .ghost()
                .flex()
                .items_center()
                .cursor_pointer(),
            CHIP_CAP,
        )
        .child(
            chip_label(text, FontWeight::MEDIUM)
                .debug_selector(move || format!("permission-chip-{effective}")),
        )
        .child(
            icon(IconName::ChevronDown, CHIP_CHEVRON, theme::caption())
                .debug_selector(|| "composer-permission-chevron".to_string()),
        );
        div()
            .debug_selector(|| "composer-permission".to_string())
            .child(chip.disabled(streaming).dropdown_menu_with_anchor(
                Anchor::TopLeft,
                move |menu, _, _| {
                    // Short rows fit the kit's own 500px ceiling on one line;
                    // the 330 minimum is the reference's. No warning line sits
                    // above them: the menu says what a mode permits and nothing
                    // else, and the sandbox state has its own surfaces.
                    let mut menu = menu.min_w(px(330.0));
                    for choice in choices.clone() {
                        let item = PopupMenuItem::new(choice.text).checked(choice.checked);
                        if !choice.enabled {
                            menu = menu.item(item.disabled(true));
                            continue;
                        }
                        let pick = shell.clone();
                        let value = choice.mode.value();
                        menu = menu.item(item.on_click(move |_, _window, cx| {
                            pick.update(cx, |this, cx| this.request_permission(value, cx));
                        }));
                    }
                    menu
                },
            ))
    }

    /// The reference's draft-attachment rail: one 64px tile per image, under
    /// the card's own 2px top pad and a 10px side inset, pulled 6px into the
    /// editor's gap.
    ///
    /// Measured (`ComposerAttachments.module.css` / `AttachmentRail.module.css`):
    /// `.rail{margin-bottom:-6px;padding:2px 10px 0}`, `.item{height:64px}`,
    /// `.thumbnail{width:64px;height:64px;border:.5px solid
    /// border-l2-darkmode-thin;border-radius:radius-xl;background:
    /// interactive-bg-hover}` with an `object-fit:cover` picture inside, and
    /// `.remove{width:18px;height:18px;border-radius:50%;background:
    /// button-contrast-fill;color:label-primary-inverted;top:4px;right:4px;
    /// opacity:0}` revealed by `.item:hover .remove`.
    fn attachment_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.state.attachments.is_empty() {
            return None;
        }
        let tiles = self
            .state
            .attachments
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                let group = SharedString::from(format!("attachment-tile-{index}"));
                let remove = div()
                    .debug_selector(move || format!("attachment-remove-{index}"))
                    .id(SharedString::from(format!("attachment-remove-{index}")))
                    .absolute()
                    .top(px(ATTACHMENT_REMOVE_INSET))
                    .right(px(ATTACHMENT_REMOVE_INSET))
                    .w(px(ATTACHMENT_REMOVE))
                    .h(px(ATTACHMENT_REMOVE))
                    .rounded_full()
                    .bg(rgb(theme::text()))
                    .opacity(0.0)
                    .group_hover(group.clone(), |style| style.opacity(1.0))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(IconName::X, 12.0, theme::page()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.remove_attachment(index);
                        cx.notify();
                    }));
                let tile = div()
                    .debug_selector(move || format!("attachment-{index}"))
                    .id(SharedString::from(format!("attachment-{index}")))
                    .group(group)
                    .relative()
                    .flex_none()
                    .w(px(ATTACHMENT_TILE))
                    .h(px(ATTACHMENT_TILE))
                    .rounded(px(theme::RADIUS_XL))
                    .overflow_hidden()
                    .bg(rgb(theme::hover()))
                    .child(tile_image(attachment))
                    .child(remove);
                hairline_all(tile).border_color(rgb(theme::border_soft()))
            })
            .collect::<Vec<_>>();
        Some(
            div()
                .debug_selector(|| "attachments".to_string())
                .id("attachments")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(ATTACHMENT_GAP))
                .pl(px(ATTACHMENT_PAD))
                .pr(px(ATTACHMENT_PAD))
                .pt(px(ATTACHMENT_TOP))
                .mb(px(ATTACHMENT_PULL))
                .overflow_x_scroll()
                .children(tiles),
        )
    }
    /// The reference's `.primary`: a 34x34 round control on the info fill with
    /// a white 16px glyph, lifted the measured 2px and dimmed to 40% when there
    /// is nothing to send.
    ///
    /// `.primary{corner-shape:round;background:var(--dsw-alias-button-info-fill);
    /// color:#fff;border-radius:999px;width:34px;height:34px;
    /// transform:translateY(-2px)}` with `.primary:disabled{opacity:.4}`, and
    /// the glyphs are inline 16px svgs: the stop mark is
    /// `rect x=3 y=3 width=10 height=10 rx=3`, the send mark a filled arrow.
    fn send_button(
        &self,
        streaming: bool,
        can_send: bool,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let stops = primary_stops(streaming, can_send);
        let enabled = stops || can_send;
        let label: SharedString = lang
            .tr(if stops { "input.stop" } else { "input.send" })
            .into();
        let glyph = if stops {
            div()
                .debug_selector(|| "composer-stop-mark".to_string())
                .w(px(10.0))
                .h(px(10.0))
                .rounded(px(3.0))
                .bg(rgb(ON_ACCENT))
        } else {
            icon(IconName::ArrowUp, 16.0, ON_ACCENT)
        };
        div()
            .debug_selector(|| "composer-send".to_string())
            .id("composer-send")
            .w(px(34.0))
            .h(px(34.0))
            .flex_none()
            .rounded_full()
            .bg(rgb(theme::accent()))
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .top(px(-2.0))
            .when(!enabled, |s| s.opacity(0.4))
            .cursor_pointer()
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .child(glyph)
            .on_click(cx.listener(move |this, _event, window, cx| {
                if stops {
                    this.stop_active(cx);
                } else {
                    this.send(window, cx);
                }
            }))
    }
}

/// One attachment's 64px picture, or its file name when the payload did not
/// decode as an image.
///
/// The preview is already square and at 2x, so the element only asks GPUI for
/// a cover fit; GPUI caches the decoded draw by the image's content id, so a
/// re-created `Image` each frame reuses the atlas tile instead of adding one.
fn tile_image(attachment: &crate::state::Attachment) -> AnyElement {
    let Some(preview) = attachment.preview.clone() else {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .px(px(6.0))
            .child(txt(
                attachment.name.clone(),
                theme::CHIP_SIZE,
                theme::muted(),
            ))
            .into_any_element();
    };
    img(Arc::new(gpui_kit::Image::from_bytes(
        gpui_kit::ImageFormat::Png,
        preview.to_vec(),
    )))
    .w_full()
    .h_full()
    .object_fit(ObjectFit::Cover)
    .into_any_element()
}
