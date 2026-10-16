//! The message composer: input, capability chips, and send/stop.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::Disableable as _;
use gpui_kit::*;

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

/// What the model chip needs to name the model and fill its menu: the label it
/// shows, the effort suffix the trigger appends, the active id, the catalog rows
/// the menu offers, and whether a turn is running (its mark swaps for the dot).
struct ModelChipData {
    label: String,
    effort: Option<String>,
    current: String,
    models: Vec<String>,
    streaming: bool,
}

/// Ask for files and mention them in the composer.
/// The permission modes the chip's menu offers, in the reference's own order.
///
/// The reference's `PRESET_LABEL_KEYS` map is read-only, then
/// workspace-write, then full access, so the menu draws that order and the
/// General pane's row reads the same list. The chip and the pane both write
/// `Settings.permission`, which is the one field `caps::build_agent` obeys.
pub(crate) fn permission_modes() -> [(&'static str, &'static str); 3] {
    [
        (crate::caps::READ_ONLY, "permission.presetReadOnly"),
        (
            crate::caps::WORKSPACE_WRITE,
            "permission.presetWorkspaceWrite",
        ),
        (crate::caps::FULL_ACCESS, "permission.presetFullAccess"),
    ]
}

/// The measured rule for the primary control: it stops only while a turn runs
/// AND the draft is empty (`primaryStops = running && subagent === null &&
/// (empty || blocked)`). With text in the draft the reference sends - which for
/// bsh is the queue path the dock already shows.
pub(crate) fn primary_stops(streaming: bool, can_send: bool) -> bool {
    streaming && !can_send
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

impl Shell {
    /// Write the permission mode the composer chip offers.
    ///
    /// It is the same field the General pane's choice row writes, so the chip
    /// and that row can never disagree about what the tools are allowed to do.
    /// Full access goes through the reference's own gate rather than landing
    /// here: `request_permission` is the one entry point both controls call,
    /// and it is what keeps the confirmation from being a pane-only ritual.
    pub(crate) fn set_permission(&mut self, permission: &str, cx: &mut Context<Self>) {
        self.state.draft_mut().permission = permission.to_string();
        cx.notify();
    }

    /// Select a permission mode, opening the risk gate when the mode needs one.
    ///
    /// The reference's `choose` returns early on the current value and routes
    /// `danger-full-access` to `RiskConfirmation` instead of writing it, so
    /// the gate cannot be skipped by choosing it from a different control.
    pub(crate) fn request_permission(&mut self, permission: &str, cx: &mut Context<Self>) {
        if self.state.draft().permission == permission {
            return;
        }
        if permission == crate::caps::FULL_ACCESS {
            self.permission_acknowledged = false;
            self.permission_confirm = Some(permission.to_string());
            cx.notify();
            return;
        }
        self.set_permission(permission, cx);
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
/// mode names the confined one, because that is the mode the build falls back
/// to and the chip must not claim more access than the tools have.
pub(crate) fn permission_label_key(permission: &str) -> &'static str {
    match permission {
        crate::caps::READ_ONLY => "permission.presetReadOnly",
        crate::caps::FULL_ACCESS => "permission.presetFullAccess",
        _ => "permission.presetWorkspaceWrite",
    }
}

impl Shell {
    /// The composer card under the conversation.
    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The chip names the mode the tools obey, not a fixed label.
        let permission = self.state.draft().permission.clone();
        let lang = self.state.lang;
        let streaming = self.state.any_streaming();
        let can_send = !self.composer_value.trim().is_empty();
        let model = self.state.settings.effective().model;
        // The menu offers the same list the Models pane draws: the active
        // provider's customized catalog.
        let models = {
            let draft = self.state.draft();
            draft.catalog(&draft.active_provider).to_vec()
        };
        let column = div().pb(px(4.0)).flex().flex_col().child({
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
                            self.state.settings.content_font_size,
                        )))
                        .max_h(px(theme::COMPOSER_INPUT_MAX_H))
                        .overflow_y_scroll()
                        .child(Textarea::new(&self.composer).appearance(false)),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .pt(px(2.0))
                        .pb(px(6.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .child(self.add_button(
                            lang.tr("input.commands"),
                            super::command_menu::toggle_action,
                            cx,
                        ))
                        .child(self.permission_chip(permission, streaming, lang, cx))
                        .child(div().flex_1())
                        .child(self.model_chip(
                            ModelChipData {
                                label: model_label(&model, lang),
                                effort: effort_suffix(
                                    self.state.settings.effective().reasoning_effort.as_deref(),
                                ),
                                current: model.clone(),
                                models: models.clone(),
                                streaming,
                            },
                            lang,
                            cx,
                        ))
                        .child(self.send_button(streaming, can_send, lang, cx)),
                )
                // Measured `.card{position:absolute;bottom:calc(100% + 4px);
                // left:0;right:0}`: the menu is out of the card's flow and
                // spans the input it belongs to.
                .when_some(self.command_menu(cx), |card, menu| card.child(menu));
            hairline_all(card).border_color(rgb(theme::border_soft()))
        });
        // Measured: the meter belongs under the card, centred in the composer's
        // own box, and the reference draws nothing until a prompt size exists.
        column.when_some(self.context_meter(cx), |column, meter| {
            column.child(
                div()
                    .pt(px(theme::METER_TOP_GAP))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(meter),
            )
        })
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
            current,
            models,
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
                    let models = models.clone();
                    let current = current.clone();
                    let shell = shell.clone();
                    menu.submenu(
                        lang.tr("composer.menuModel"),
                        window,
                        cx,
                        move |submenu, _window, _cx| {
                            // The measured \`.menu\` box: \`min-width:min(240px,
                            // 100vw - 32px)\`, \`max-width:min(420px, 100vw - 32px)\`.
                            let submenu = submenu.min_w(px(240.0)).max_w(px(420.0));
                            if models.is_empty() {
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
                            for (model, active) in model_choices(&models, &current) {
                                let pick = shell.clone();
                                let name = model.clone();
                                submenu = submenu.item(
                                    PopupMenuItem::new(model).checked(active).on_click(
                                        move |_, _window, cx| {
                                            pick.update(cx, |this, cx| {
                                                this.catalog_select(&name);
                                                cx.notify();
                                            });
                                        },
                                    ),
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
    fn permission_chip(
        &self,
        permission: String,
        streaming: bool,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let shell = cx.entity();
        let chip = trigger_chrome(
            Button::new("composer-permission")
                .ghost()
                .flex()
                .items_center()
                .cursor_pointer(),
            CHIP_CAP,
        )
        .child(chip_label(
            lang.tr(permission_label_key(&permission)),
            FontWeight::MEDIUM,
        ))
        .child(
            icon(IconName::ChevronDown, CHIP_CHEVRON, theme::caption())
                .debug_selector(|| "composer-permission-chevron".to_string()),
        );
        div()
            .debug_selector(|| "composer-permission".to_string())
            .child(chip.disabled(streaming).dropdown_menu_with_anchor(
                Anchor::TopLeft,
                move |menu, _, _| {
                    let mut menu = menu;
                    for (value, key) in permission_modes() {
                        let pick = shell.clone();
                        menu = menu.item(
                            PopupMenuItem::new(lang.tr(key))
                                .checked(value == permission)
                                .on_click(move |_, _window, cx| {
                                    pick.update(cx, |this, cx| this.request_permission(value, cx));
                                }),
                        );
                    }
                    menu
                },
            ))
    }

    /// The chips naming the images attached to the next turn.
    fn attachment_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.state.attachments.is_empty() {
            return None;
        }
        let chips = self
            .state
            .attachments
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                let label = format!("{}  {}", attachment.name, '\u{2715}');
                div()
                    .debug_selector(move || format!("attachment-{index}"))
                    .id(SharedString::from(format!("attachment-{index}")))
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(rgb(theme::hover()))
                    .cursor_pointer()
                    .child(txt(label, theme::CHIP_SIZE, theme::muted()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.remove_attachment(index);
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        Some(div().flex().flex_row().gap(px(6.0)).children(chips))
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
