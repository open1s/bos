//! The settings dialog.
//!
//! Shaped after the DeepSeek Harness reference: a vertical rail of sections, a
//! scrolling pane whose header states what the section covers, and one
//! Cancel/Save pair at the foot. Everything edits a draft, so nothing reaches
//! the agent until Save; Cancel drops the draft.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputContentType, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, hairline_bottom, icon, txt, DivExt, IconName};
use crate::i18n::{tr, tr_args, Lang};
use crate::settings::Settings;
use crate::theme;

/// The rail, in display order; the index is the section number narrowest the
/// state stores. Public within the crate so the command line can deep-link.
pub(crate) const SECTIONS: [&str; 14] = [
    "settings.account",
    "settings.general",
    "settings.models",
    "settings.tools",
    "settings.plugin-inventory",
    "settings.shell",
    "settings.agent-loop",
    "settings.mcp",
    "settings.archived-sessions",
    "settings.workspaces",
    "settings.context",
    "settings.agent-presets",
    "settings.plugins",
    "settings.automation",
];

/// The rail's cells, measured from the reference.
///
/// DSH registers five `settings.section`s and sorts them by `order`: account
/// (-10), general (0), models (10), plugins (15, "Built-in plugins") and
/// agent-presets (20). Everything else this crate's `SECTIONS` carries is a
/// card, a tab or a sidebar panel in the reference, so it is not a rail cell
/// here either. See `docs/ui-mirror/reference/dsh-settings-rail.md`.
pub(crate) const NAV: [&str; 5] = ["account", "general", "models", "plugins", "agent-presets"];

/// The panes the Built-in plugins section hosts, in the reference's own order:
/// the inventory tab first (`settings.plugins.tab`, order 10), then the plugin
/// packages' cards (Shell 10, Agent loop 20), then the panes DSH reaches from
/// its sidebar rather than from a settings cell.
pub(crate) const VIEWS: [&str; 9] = [
    "plugin-inventory",
    "shell",
    "agent-loop",
    "tools",
    "mcp",
    "archived-sessions",
    "workspaces",
    "context",
    "automation",
];

/// Resolve a section id (the part after `settings.`) to its rail index.
/// Parse a capacity the way the reference documents it: a number, optionally
/// with a K or M suffix, so 128K and 1M are accepted beside 131072.
fn parse_capacity(text: &str) -> Option<usize> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (digits, scale) = match trimmed.chars().last()? {
        'k' | 'K' => (&trimmed[..trimmed.len() - 1], 1024),
        'm' | 'M' => (&trimmed[..trimmed.len() - 1], 1024 * 1024),
        _ => (trimmed, 1),
    };
    digits.trim().parse::<usize>().ok()?.checked_mul(scale)
}

pub(crate) fn section_index(id: &str) -> Option<usize> {
    SECTIONS
        .iter()
        .position(|key| key.trim_start_matches("settings.") == id)
}

/// The text fields the Models and Context sections edit.
pub(crate) struct SettingsInputs {
    model: Entity<InputState>,
    base_url: Entity<InputState>,
    api_key: Entity<InputState>,
    system_prompt: Entity<InputState>,
    budget: Entity<InputState>,
    shell_timeout: Entity<InputState>,
    output_cap: Entity<InputState>,
    pub(crate) max_steps: Entity<InputState>,
    max_tokens: Entity<InputState>,
    provider_name: Entity<InputState>,
}

impl SettingsInputs {
    /// Build the fields.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            model: cx.new(|cx| InputState::new(window, cx)),
            base_url: cx.new(|cx| InputState::new(window, cx)),
            api_key: cx.new(|cx| InputState::new(window, cx).masked(true)),
            system_prompt: cx.new(|cx| InputState::new(window, cx)),
            budget: cx.new(|cx| InputState::new(window, cx)),
            shell_timeout: cx.new(|cx| InputState::new(window, cx)),
            output_cap: cx.new(|cx| InputState::new(window, cx)),
            max_steps: cx.new(|cx| InputState::new(window, cx)),
            max_tokens: cx.new(|cx| InputState::new(window, cx)),
            provider_name: cx.new(|cx| InputState::new(window, cx)),
        }
    }

    /// Load the fields from the settings the dialog opened on.
    pub(crate) fn sync(&self, settings: &Settings, window: &mut Window, cx: &mut Context<Shell>) {
        let effective = settings.effective();
        set(&self.model, effective.model, window, cx);
        set(&self.base_url, effective.base_url, window, cx);
        set(&self.api_key, effective.api_key, window, cx);
        set(&self.system_prompt, effective.system_prompt, window, cx);
        set(
            &self.budget,
            effective.context_budget.to_string(),
            window,
            cx,
        );
        set(
            &self.shell_timeout,
            effective.shell_timeout_secs.to_string(),
            window,
            cx,
        );
        set(
            &self.output_cap,
            effective.tool_output_cap.to_string(),
            window,
            cx,
        );
        set(&self.max_steps, effective.max_steps.to_string(), window, cx);
        set(
            &self.max_tokens,
            effective
                .max_tokens
                .map(|tokens| tokens.to_string())
                .unwrap_or_default(),
            window,
            cx,
        );
    }

    /// Write the fields back into the draft.
    pub(crate) fn read(&self, settings: &mut Settings, cx: &App) {
        settings.model = self.model.read(cx).value().to_string();
        settings.base_url = self.base_url.read(cx).value().to_string();
        settings.api_key = self.api_key.read(cx).value().to_string();
        settings.system_prompt = self.system_prompt.read(cx).value().to_string();
        let budget = self.budget.read(cx).value().trim().to_string();
        if let Some(parsed) = parse_capacity(&budget) {
            settings.context_budget = parsed;
        }
        let timeout = self.shell_timeout.read(cx).value().trim().to_string();
        if let Ok(parsed) = timeout.parse::<u64>() {
            settings.shell_timeout_secs = parsed.max(1);
        }
        let cap = self.output_cap.read(cx).value().trim().to_string();
        if let Ok(parsed) = cap.parse::<usize>() {
            settings.tool_output_cap = parsed.max(256);
        }
        let steps = self.max_steps.read(cx).value().trim().to_string();
        if let Ok(parsed) = steps.parse::<usize>() {
            settings.max_steps = parsed.max(1);
        }
        let tokens = self.max_tokens.read(cx).value().trim().to_string();
        if tokens.is_empty() {
            settings.max_tokens = None;
        } else if let Some(parsed) = parse_capacity(&tokens) {
            settings.max_tokens = Some(parsed.min(u32::MAX as usize) as u32);
        }
    }
}

/// Replace one field's text.
fn set(input: &Entity<InputState>, value: String, window: &mut Window, cx: &mut Context<Shell>) {
    input.update(cx, |state, cx| state.set_value(value, window, cx));
}

/// Load a saved provider into the form: the draft carries the values the form
/// saves, and the inputs are what the user sees.
pub(crate) fn load_provider_into_form(
    shell: &mut Shell,
    profile: &crate::settings::Profile,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    set(
        &shell.settings_inputs.provider_name,
        profile.name.clone(),
        window,
        cx,
    );
    set(
        &shell.settings_inputs.model,
        profile.model.clone(),
        window,
        cx,
    );
    set(
        &shell.settings_inputs.base_url,
        profile.base_url.clone(),
        window,
        cx,
    );
    set(
        &shell.settings_inputs.api_key,
        profile.api_key.clone(),
        window,
        cx,
    );
    shell.state.begin_provider_edit(profile);
}

/// Switch the interface language.
fn language_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    let lang = if key == "zh" { Lang::Zh } else { Lang::En };
    shell.state.lang = lang;
    shell.state.settings.lang = lang.locale().to_string();
    let _ = shell.state.settings.save();
    cx.notify();
}

/// Switch the colour mode and remember it.
fn theme_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    shell.state.settings.theme_mode = key.to_string();
    let _ = shell.state.settings.save();
    crate::theme::apply_here(key, cx);
    cx.notify();
}

/// Step the conversation font size and remember it.
///
/// The reference stepper accepts 10..22 px and moves the conversation text by
/// the step, so the shell re-renders instead of only storing the number.
fn font_size_action(shell: &mut Shell, delta: i32, cx: &mut Context<Shell>) {
    let current = shell.state.settings.content_font_size as i32;
    let next = (current + delta).clamp(
        crate::theme::CONTENT_MIN as i32,
        crate::theme::CONTENT_MAX as i32,
    ) as u32;
    if next == shell.state.settings.content_font_size {
        return;
    }
    shell.state.settings.content_font_size = next;
    let _ = shell.state.settings.save();
    cx.notify();
}

/// The reference's own label for a permission preset.
///
/// Routed through the composer chip's single namer so the pane and the chip
/// cannot drift apart.
fn preset_label(lang: Lang, permission: &str) -> String {
    tr(lang, super::composer::permission_label_key(permission))
}

/// Choose the default permission mode for new sessions.
///
/// The reference's trinary, measured verbatim in
/// `dsh-client-ui-permission-presets`: `read-only`, `workspace-write` and
/// `danger-full-access`. Full access goes through `request_permission` rather
/// than landing here, so the risk gate cannot be skipped from the pane either.
fn permission_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    shell.request_permission(key, cx);
}

/// Write one of the General pane's string preferences.
fn preference_action(
    shell: &mut Shell,
    field: fn(&mut crate::settings::Settings) -> &mut String,
    value: &str,
    cx: &mut Context<Shell>,
) {
    let draft = shell.state.draft_mut();
    if field(draft) == value {
        return;
    }
    *field(draft) = value.to_string();
    cx.notify();
}

/// Show or hide the coding view, and save it at once.
///
/// Measured: `DeveloperToolsRow` writes through the controller rather than into a
/// draft, and reports `developerTools.error` inline when the write fails, so a
/// refusal is visible on the row instead of looking like it worked.
fn developer_tools_action(shell: &mut Shell, on: bool, cx: &mut Context<Shell>) {
    shell.state.draft_mut().developer_tools = on;
    if shell.state.settings.save().is_err() {
        shell.developer_tools_failed = true;
    }
    cx.notify();
}

/// Choose the API protocol the agent speaks; automatic follows the model.
fn api_mode_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    let mode = if key == "auto" {
        String::new()
    } else {
        key.to_string()
    };
    shell.state.draft_mut().api_mode = mode;
    cx.notify();
}

/// Attach or detach the memory store.
fn memory_action(shell: &mut Shell, on: bool, cx: &mut Context<Shell>) {
    shell.state.draft_mut().memory_enabled = on;
    cx.notify();
}

/// How much of a tool call the transcript shows.
///
/// The reference's own modes, measured from its `TranscriptViewPolicy`
/// (`compact | standard | detailed | verbose`, with `normal` and `expanded`
/// kept as legacy spellings) and applied by `derivePresentationPolicy`, which
/// decides completed-turn folding, step grouping, whether a live process step is
/// shown and whether a settled reasoning block keeps its preview.
fn transcript_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    preference_action(shell, |settings| &mut settings.transcript_view, key, cx);
}

/// Where a link in the transcript opens.
fn link_opening_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    preference_action(shell, |settings| &mut settings.link_opening, key, cx);
}

/// What Enter does while a turn streams.
fn composer_enter_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    preference_action(shell, |settings| &mut settings.composer_enter, key, cx);
}

/// How much performance and usage detail the footer shows.
fn performance_usage_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    preference_action(shell, |settings| &mut settings.performance_usage, key, cx);
}

/// The one reveal path the account pane's button and the header action share.
///
/// The failure is recorded on the shell as well as shown as a notice, because
/// the header action prints the reference's own inline error beside the button.
pub(crate) fn reveal_config_shared(shell: &mut Shell, cx: &mut Context<Shell>) {
    let lang = shell.state.lang;
    match crate::settings::reveal_config() {
        Ok(()) => {
            shell.config_open_failed = false;
            shell
                .state
                .notice(tr(lang, "settings.openedConfig").to_string());
        }
        Err(error) => {
            shell.config_open_failed = true;
            shell
                .state
                .notice(format!("{} {error}", tr(lang, "settings.revealFailed")));
        }
    }
    cx.notify();
}

/// The editable fields whose shipped default the dialog can restore.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The shell command timeout.
    ShellTimeout,
    /// The per-stream output cap.
    OutputCap,
    /// The ReAct step ceiling.
    MaxSteps,
    /// The context budget.
    Budget,
    /// The cap on completion tokens per turn.
    MaxTokens,
}

impl Field {
    /// The shipped default for this field, as the text its input holds.
    fn default_text(self) -> String {
        let default = Settings::default();
        match self {
            Field::ShellTimeout => default.shell_timeout_secs.to_string(),
            Field::OutputCap => default.tool_output_cap.to_string(),
            Field::MaxSteps => default.max_steps.to_string(),
            Field::Budget => default.context_budget.to_string(),
            Field::MaxTokens => default
                .max_tokens
                .map(|tokens| tokens.to_string())
                .unwrap_or_default(),
        }
    }
}

/// One button of the permission risk gate.
///
/// The reference draws both gate buttons from one component whose two
/// minimum widths are measured (`.modalAction` is 72 for the cancel, and
/// `.confirmAction` is 136 for the confirm), so the width travels with the
/// label and the enablement rather than as three more positional arguments.
struct GateButton {
    /// The control id, stable for the click tests.
    id: &'static str,
    /// The button's own label, already localized.
    label: SharedString,
    /// Whether this is the filled confirm rather than the outline cancel.
    primary: bool,
    /// The measured minimum width for this position.
    min_w: f32,
    /// Whether the reference's acknowledgement has released the button.
    enabled: bool,
    /// The write the press performs.
    action: fn(&mut Shell, &mut Context<Shell>),
}

/// One option of a [`SelectRow`]: the value the setting stores and the label
/// the reference shows for it.
///
/// The pair is named because the option list outlives the call that built it -
/// the kit's menu builder is static, so the list is owned before it is
/// captured.
type RowOption = (&'static str, String);

/// Everything one General-pane select row needs: the control's own id and its
/// two copy keys, the current value, the offered options and the write a pick
/// performs.
///
/// The reference's own PreferenceRow is one component over one prop record, so
/// the rows are data here too; a positional call with seven arguments was both
/// hard to read and one over clippy's argument limit.
struct SelectRow {
    /// The control id, stable for the click tests.
    id: &'static str,
    /// The row's title key.
    title: &'static str,
    /// The row's description key, which the reference's LanguageRow alone
    /// omits.
    desc: Option<&'static str>,
    /// The value the setting currently holds.
    current: String,
    /// The offered values and their labels, in the reference's own order.
    options: Vec<RowOption>,
    /// The write a pick performs.
    action: fn(&mut Shell, &str, &mut Context<Shell>),
}

impl Shell {
    /// The modal card.
    pub(crate) fn render_settings(
        &mut self,
        view_width: f32,
        view_height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.caps.is_empty() {
            let built = crate::caps::build_agent(self.state.draft());
            self.caps = crate::caps::capabilities(&built.agent);
            self.plugins = built.plugins;
            // The dialog's build already knows the tool array, so the meter's
            // tools bucket follows the draft without a second build.
            self.state.set_tools_schema(built.tools_schema_chars);
        }
        let lang = self.state.lang;
        let section = self.state.settings_section;

        // Measured (SettingsRoot.module.css): the rail is 188 wide over an 18px
        // gap with 22/12 padding, a 16px/500 title, then 4px-spaced 40px cells with
        // radius 12 and 9/16/9/12 padding. Like the reference's, it carries no own
        // background or divider - the panel surface runs under it, and the cell
        // label is label-primary whether or not the cell is the active one.
        let mut list = div().flex().flex_col().gap(px(4.0));
        // The rail is the reference's five cells: `NAV` names them and
        // `section_index` resolves each to its pane.
        for (cell, id) in NAV.iter().enumerate() {
            let Some(index) = section_index(id) else {
                continue;
            };
            let selected = index == section;
            list = list.child(
                div()
                    .id(SharedString::from(format!("set-{index}")))
                    .debug_selector(move || format!("set-nav-{cell}"))
                    .h(px(40.0))
                    .pt(px(9.0))
                    .pb(px(9.0))
                    .pl(px(12.0))
                    .pr(px(16.0))
                    .rounded(px(12.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .when(selected, |row| row.bg(rgb(theme::selected_row())))
                    .when(!selected, |row| row.hover(|s| s.bg(rgb(theme::hover()))))
                    .child(
                        txt(tr(lang, SECTIONS[index]), 14.0, theme::text())
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden(),
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.settings_section = index;
                        cx.notify();
                    })),
            );
        }
        let nav = div()
            .w(px(188.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .pt(px(22.0))
            .px(px(12.0))
            .child(
                txt(tr(lang, "settings.title"), 16.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .px(px(12.0)),
            )
            .child(list);

        let (title, hint, content) = match section {
            0 => self.section_account(),
            1 => self.section_general(cx),
            2 => self.section_models(cx),
            3 => self.section_tools(cx),
            4 => self.section_plugins(cx),
            5 => self.section_shell(cx),
            6 => self.section_agent_loop(cx),
            7 => self.section_mcp(),
            8 => self.section_archived(cx),
            9 => self.section_workspaces(cx),
            10 => self.section_context(cx),
            11 => self.section_agent_presets(cx),
            12 => self.section_builtin_plugins(cx),
            _ => self.section_automation(cx),
        };

        // Measured (SettingsRoot.module.css): a 54px header row holding the section
        // title and the close control, then the scrolling options box on 0/24/24
        // padding. The save bar is the section form's own footer - 16px of top
        // padding, an 8px gap and ONE 13px Save on the label-primary fill; the
        // reference has no modal-wide Cancel, because the close control is what
        // discards the draft.
        let header = div()
            .debug_selector(|| "settings-header".to_string())
            .flex_none()
            .h(px(54.0))
            .pt(px(20.0))
            .pl(px(10.0))
            .pr(px(14.0))
            .pb(px(8.0))
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(px(8.0))
            .child(
                txt(tr(lang, title), 16.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .flex_1()
                    .min_w_0(),
            )
            .child(self.document_action(cx))
            .child(
                div()
                    .id("settings-close")
                    .debug_selector(|| "settings-close".to_string())
                    .w(px(28.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::Close, 14.0, theme::text()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.cancel_settings(cx);
                    })),
            );

        let footer = div()
            .pt(px(16.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(div().flex_1().min_w_0())
            .child(
                div()
                    .id("settings-save")
                    .debug_selector(|| "settings-save".to_string())
                    .px(px(14.0))
                    .py(px(5.0))
                    .rounded(px(12.0))
                    .bg(rgb(theme::text()))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.9))
                    .child(txt(tr(lang, "settings.save"), 13.0, theme::page()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.save_settings(cx);
                    })),
            );

        let pane = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("settings-pane")
                    .debug_selector(|| "settings-pane".to_string())
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(24.0))
                    .pb(px(24.0))
                    .flex()
                    .flex_col()
                    .child(txt(tr(lang, hint), 12.0, theme::faint()).mb(px(12.0)))
                    .children(content)
                    .child(footer),
            );

        // Measured (SettingsRoot.module.css): 800 wide and as tall as
        // min(800, viewport - 48) - the mask's own 24px inset on each side - on
        // radius-panel (28) with the prominent elevation and no border.
        let panel_w = (view_width - 48.0).clamp(320.0, 800.0);
        let panel_h = (view_height - 48.0).clamp(240.0, 800.0);

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgba(0x0F111580))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("settings-panel")
                    .debug_selector(|| "settings-panel".to_string())
                    .w(px(panel_w))
                    .h(px(panel_h))
                    .rounded(px(28.0))
                    .bg(rgb(theme::page()))
                    .shadow_lg()
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .child(nav)
                    .child(pane),
            )
    }

    /// A read-only label/value row.
    fn settings_row(&self, label: &'static str, value: String) -> AnyElement {
        self.settings_control_row(label, txt(value, 13.0, theme::text()))
    }

    /// A label plus any trailing control.
    fn settings_control_row(&self, label: &'static str, control: impl IntoElement) -> AnyElement {
        hairline_bottom(
            div()
                .py(px(12.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .border_color(rgb(theme::border_soft()))
                // The label keeps its natural width and the control
                // wraps instead. With the label as the flexible child a wide
                // control - the four API-protocol chips - squeezed it to zero
                // width, and the text then wrapped one character per line.
                .child(
                    txt(tr(self.state.lang, label), 13.0, theme::text())
                        .font_weight(FontWeight::MEDIUM)
                        .flex_none(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .justify_end()
                        .child(control),
                ),
        )
        .into_any_element()
    }

    /// A note under the rows.
    fn settings_note(&self, key: &'static str, warning: bool) -> AnyElement {
        txt(
            tr(self.state.lang, key),
            12.0,
            if warning {
                theme::warn()
            } else {
                theme::faint()
            },
        )
        .pt(px(12.0))
        .into_any_element()
    }

    /// The measured Font size row: a two-line label block and the reference's
    /// own 72x36 value stepper - the number alone, its chevrons revealed on
    /// hover - with the `px` unit beside it.
    fn font_size_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let size = self.state.settings.content_font_size;
        let arrows = div()
            .absolute()
            .right(px(8.0))
            .top(px(6.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .opacity(0.0)
            .group_hover("font-stepper", |s| s.opacity(1.0))
            .child(self.font_step("font-increase", 1, cx))
            .child(self.font_step("font-decrease", -1, cx));
        let stepper = div()
            .id(SharedString::from("font-stepper"))
            .debug_selector(|| "font-stepper".to_string())
            .group("font-stepper")
            .relative()
            .min_w(px(72.0))
            .h(px(36.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(12.0))
            .bg(rgb(theme::selected_row()))
            .child(txt(format!("{size}"), 14.0, theme::text()))
            .child(arrows);
        hairline_bottom(
            div()
                .py(px(16.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .border_color(rgb(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(4.0))
                        .pr(px(48.0))
                        .child(txt(tr(lang, "settings.fontSize"), 14.0, theme::text()))
                        .child(txt(tr(lang, "settings.fontSizeHint"), 12.0, theme::faint())),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(stepper)
                        .child(txt(tr(lang, "settings.fontSizeUnit"), 14.0, theme::muted())),
                ),
        )
        .into_any_element()
    }

    /// One chevron of the Font size stepper: the reference's own 17x12 chip at
    /// the measured radius, inert at either end of the range.
    fn font_step(&self, id: &'static str, delta: i32, cx: &mut Context<Self>) -> AnyElement {
        let size = self.state.settings.content_font_size as i32;
        let at_end = (size + delta).clamp(
            crate::theme::CONTENT_MIN as i32,
            crate::theme::CONTENT_MAX as i32,
        ) == size;
        let glyph = if delta > 0 {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };
        let button = div()
            .id(SharedString::from(id))
            .debug_selector(move || id.to_string())
            .w(px(17.0))
            .h(px(12.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(rgb(theme::page()))
            .child(icon(
                glyph,
                9.0,
                if at_end {
                    theme::faint()
                } else {
                    theme::text()
                },
            ));
        if at_end {
            return button.into_any_element();
        }
        button
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .on_click(
                cx.listener(move |this, _event, _window, cx| font_size_action(this, delta, cx)),
            )
            .into_any_element()
    }

    /// The measured Appearance row: three cubes, an icon over its label, in the
    /// reference's wrapping row. The selected cube takes its platform fill and a
    /// stronger border; the others keep the hairline and fill on hover.
    fn appearance_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let current = self.state.settings.theme_mode.clone();
        let cubes = [
            ("light", IconName::Sun, "settings.modeLight"),
            ("dark", IconName::Moon, "settings.modeDark"),
            ("system", IconName::Monitor, "settings.modeSystem"),
        ];
        let mut row = div().flex().flex_row().flex_wrap().gap(px(8.0));
        for (id, glyph, key) in cubes {
            let selected = current == id;
            let pick = id.to_string();
            row = row.child(hairline_all(
                div()
                    .id(SharedString::from(format!("theme-cube-{id}")))
                    .debug_selector(move || format!("theme-cube-{id}"))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .flex_basis(px(180.0))
                    .flex_grow(1.0)
                    .flex_shrink(1.0)
                    .gap(px(4.0))
                    .px(px(32.0))
                    .py(px(20.0))
                    .rounded(px(20.0))
                    .border_color(rgb(if selected {
                        theme::border_strong()
                    } else {
                        theme::border()
                    }))
                    .when(selected, |el| el.bg(rgb(theme::selected_row())))
                    .cursor_pointer()
                    .when(!selected, |el| el.hover(|s| s.bg(rgb(theme::hover()))))
                    .child(icon(glyph, 16.0, theme::text()))
                    .child(txt(tr(lang, key), 14.0, theme::text()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        theme_action(this, &pick, cx);
                    })),
            ));
        }
        hairline_bottom(
            div()
                .py(px(16.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .border_color(rgb(theme::border_soft()))
                .child(txt(tr(lang, "settings.appearance"), 14.0, theme::text()))
                .child(row),
        )
        .into_any_element()
    }

    /// An editable text field.
    fn input_row(
        &self,
        label: &'static str,
        input: &Entity<InputState>,
        field: Field,
        cx: &mut Context<Shell>,
    ) -> AnyElement {
        self.field_row(label, input, false, field, cx)
    }

    /// The shared shape of the two: a label over a bordered, borderless-inner
    /// field.
    fn field_row(
        &self,
        label: &'static str,
        input: &Entity<InputState>,
        secret: bool,
        field: Field,
        cx: &mut Context<Shell>,
    ) -> AnyElement {
        let control = Input::new(input).appearance(false);
        let control = if secret {
            control
                .content_type(InputContentType::Password)
                .mask_toggle()
        } else {
            control
        };
        let default = field.default_text();
        let current = input.read(cx).value();
        let overridden = current.as_ref() != default.as_str();
        let mut header = div().flex().flex_row().items_center().gap(px(8.0)).child(
            txt(tr(self.state.lang, label), 13.0, theme::text())
                .font_weight(FontWeight::MEDIUM)
                .flex_1()
                .min_w_0(),
        );
        if overridden {
            let reset_input = input.clone();
            header = header
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(4.0))
                        .bg(rgb(theme::hover()))
                        .child(txt(
                            tr(self.state.lang, "settings.overridden"),
                            11.0,
                            theme::muted(),
                        )),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("reset-{label}")))
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(txt(
                            tr(self.state.lang, "settings.reset"),
                            12.0,
                            theme::muted(),
                        ))
                        .on_click(cx.listener(move |_this, _event, window, cx| {
                            set(&reset_input, default.clone(), window, cx);
                            cx.notify();
                        })),
                );
        }
        // Measured (settings-form/fields.module.css): a 6px-gap column with 12/0
        // padding and a .5px border-l2 rule under it, a 13px/500 label, and a
        // 34px control on the border-l4 hairline (radius 12) over the layer-3 fill.
        hairline_bottom(
            div()
                .py(px(12.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .border_color(rgb(theme::border_soft()))
                .child(header)
                .child(hairline_all(
                    div()
                        .debug_selector(move || format!("field-{label}"))
                        .min_h(px(34.0))
                        .rounded(px(12.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .bg(rgb(theme::new_session_fill()))
                        .border_color(rgb(theme::border()))
                        .child(control),
                )),
        )
        .into_any_element()
    }

    /// Account: identity, paths and the way to the configuration file.
    fn section_account(&self) -> (&'static str, &'static str, Vec<AnyElement>) {
        let draft = self.state.draft();
        let effective = draft.effective();
        let rows = vec![
            self.settings_row(
                "settings.identity",
                self.state.lang.tr("side.local").to_string(),
            ),
            self.settings_row(
                "settings.workspace",
                format!("{}  {}", effective.name, effective.root),
            ),
            self.settings_row("settings.version", env!("CARGO_PKG_VERSION").to_string()),
            self.settings_row(
                "settings.configFile",
                crate::settings::settings_path().display().to_string(),
            ),
            self.settings_row(
                "settings.dataDir",
                crate::settings::dir().display().to_string(),
            ),
        ];
        ("settings.account", "settings.accountHint", rows)
    }

    /// The header's Open-configuration-file action, measured from the
    /// reference's own `SettingsDocumentAction`: an error line beside an
    /// outline button, and no action at all while the file cannot be reached.
    fn document_action(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let failed = self.config_open_failed;
        // The wrapper carries its own selector: the button below owns the
        // `settings-open-config` id, and two elements answering the same
        // selector would make the click test ambiguous.
        let mut action = div()
            .debug_selector(|| "settings-open-config-action".to_string())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .min_w_0();
        if failed {
            action = action.child(
                div()
                    .debug_selector(|| "settings-open-config-error".to_string())
                    .max_w(px(180.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(txt(tr(lang, "settings.revealFailed"), 12.0, theme::error())),
            );
        }
        action
            .child(
                div()
                    .id("settings-open-config")
                    .debug_selector(|| "settings-open-config".to_string())
                    .h(px(28.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(txt(tr(lang, "settings.openConfig"), 13.0, theme::text()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.open_config(cx);
                    })),
            )
            .into_any_element()
    }

    /// The reference's shared settings-row anatomy.
    ///
    /// Measured from `PreferenceRow.module.css`: `padding:16px 0` over a .5px
    /// border-l2 rule, an 8px gap, a text block whose title is 14/22
    /// label-primary over a 12/18 label-tertiary description and which carries a
    /// 48px right pad, and whatever control the row owns.
    fn preference_row(
        &self,
        title: &'static str,
        desc: Option<&'static str>,
        control: impl IntoElement,
    ) -> AnyElement {
        hairline_bottom(
            div()
                .py(px(16.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .border_color(rgb(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(px(4.0))
                        .min_w_0()
                        .pr(px(48.0))
                        .child(txt(tr(self.state.lang, title), 14.0, theme::text()))
                        .children(
                            desc.map(|key| txt(tr(self.state.lang, key), 12.0, theme::faint())),
                        ),
                )
                .child(control),
        )
        .into_any_element()
    }

    /// The reference's own toggle, measured from `Switch.module.css`: a 36x20
    /// track at a 999px radius and a 2px pad over a 16px thumb that slides 16px,
    /// filled with brand-primary while checked and bordered in border-l3 while
    /// not. gpui-kit ships no Switch, so this is a hand-built `role="switch"`
    /// equivalent with the reference's own click rule (toggle on press).
    fn switch(
        &self,
        id: &'static str,
        checked: bool,
        enabled: bool,
        action: fn(&mut Shell, bool, &mut Context<Shell>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fill = if checked {
            theme::accent()
        } else {
            theme::border_strong()
        };
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .flex_none()
            .w(px(36.0))
            .h(px(20.0))
            .p(px(2.0))
            .rounded(px(999.0))
            .bg(rgb(fill))
            .when(enabled, |track| track.cursor_pointer())
            .when(!enabled, |track| track.opacity(0.5))
            .child(
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded(px(999.0))
                    .bg(rgb(theme::page()))
                    .when(checked, |thumb| thumb.ml(px(16.0))),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if enabled {
                    action(this, !checked, cx);
                }
            }))
            .into_any_element()
    }

    /// The reference's select control, measured from `PreferenceRow.module.css`:
    /// `height:36px`, `border-radius:radius-md`, `padding:0 14px`, a 12px gap,
    /// 14/22 label-primary over the module-platform fill, the value then a
    /// chevron, and the option list anchored to the row's end.
    fn select_row(&self, row: SelectRow, cx: &mut Context<Self>) -> AnyElement {
        let SelectRow {
            id,
            title,
            desc,
            current,
            options,
            action,
        } = row;
        let current = current.as_str();
        let options = options.as_slice();
        let shell = cx.entity();
        let value = options
            .iter()
            .find(|(key, _)| *key == current)
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| current.to_string());
        // The kit's menu builder is `'static`, so the option list and the
        // selected key are owned before they are captured.
        let entries: Vec<(&'static str, String)> = options.to_vec();
        let selected = current.to_string();
        let control = Button::new(SharedString::from(id))
            .ghost()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .bg(rgb(theme::selected_row()))
            .cursor_pointer()
            .child(txt(value, 14.0, theme::text()))
            .child(icon(IconName::ChevronDown, 12.0, theme::caption()))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut menu = menu.min_w(px(220.0));
                for (key, text) in entries.iter() {
                    let pick = shell.clone();
                    let key = *key;
                    menu = menu.item(
                        PopupMenuItem::new(text.clone())
                            .checked(*key == selected)
                            .on_click(move |_, _window, cx| {
                                pick.update(cx, |this, cx| action(this, key, cx))
                            }),
                    );
                }
                menu
            });
        self.preference_row(title, desc, control)
    }

    /// The reference's ShortcutsRow control, measured: a 36-tall button on the
    /// module-platform fill at radius-md with a 90px minimum width, holding the
    /// `shortcuts.view` label.
    fn shortcuts_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let control = div()
            .id("shortcuts-open")
            .debug_selector(|| String::from("shortcuts-open"))
            .flex_none()
            .h(px(36.0))
            .min_w(px(90.0))
            .px(px(14.0))
            .rounded(px(12.0))
            .bg(rgb(theme::selected_row()))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(txt(tr(lang, "settings.shortcutsView"), 14.0, theme::text()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.shortcuts_open = !this.shortcuts_open;
                cx.notify();
            }));
        self.preference_row(
            "settings.shortcutsTitle",
            Some("settings.shortcutsDescription"),
            control,
        )
    }

    /// The reference's `CurrentVersionRow`: one text node and no control.
    fn current_version_row(&self) -> AnyElement {
        hairline_bottom(
            div()
                .debug_selector(|| String::from("general-item-current-version"))
                .py(px(16.0))
                .flex()
                .items_center()
                .border_color(rgb(theme::border_soft()))
                .child(txt(
                    tr_args(
                        self.state.lang,
                        "settings.general.currentVersion",
                        &[("version", env!("CARGO_PKG_VERSION").to_string())],
                    ),
                    12.0,
                    theme::faint(),
                )),
        )
        .into_any_element()
    }

    /// General: the reference's own rows in its own order.
    ///
    /// Measured from `dsh-client-ui-settings-general`'s `settings.general.item`
    /// registrations, sorted by order: permission (-20), language (0), appearance
    /// (10), font-size (11), transcript-view (12), developer-tools (15),
    /// shortcuts (16), link-opening (17), composer-enter (20), performance-usage
    /// (30), current-version (100). `session-log-deepseek` (90) is host-gated in
    /// the reference and renders nothing there, so it is not drawn here either.
    /// The one row the reference does NOT carry in General is the free-text
    /// section note, which its `GeneralSection` omits; the shell-only settings
    /// moved to the Shell card see `section_shell`.
    fn section_general(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let lang = self.state.lang;
        let draft = self.state.draft().clone();
        let permission = draft.permission.clone();
        let language = if self.state.lang == Lang::Zh {
            "zh"
        } else {
            "en"
        };
        let transcript = draft.transcript_view.clone();
        let links = draft.link_opening.clone();
        let enter = draft.composer_enter.clone();
        let performance = draft.performance_usage.clone();
        let developer_tools = draft.developer_tools;
        let failed = self.developer_tools_failed;
        let mut rows = vec![
            self.select_row(
                SelectRow {
                    id: "set-perm",
                    title: "settings.permission",
                    desc: Some("permission.description"),
                    current: permission,
                    options: vec![
                        (
                            crate::caps::READ_ONLY,
                            preset_label(lang, crate::caps::READ_ONLY),
                        ),
                        (
                            crate::caps::WORKSPACE_WRITE,
                            preset_label(lang, crate::caps::WORKSPACE_WRITE),
                        ),
                        (
                            crate::caps::FULL_ACCESS,
                            preset_label(lang, crate::caps::FULL_ACCESS),
                        ),
                    ],
                    action: permission_action,
                },
                cx,
            ),
            self.select_row(
                SelectRow {
                    id: "set-lang",
                    title: "settings.language",
                    // The one measured row with no description line.
                    desc: None,
                    current: language.to_string(),
                    options: vec![
                        ("en", String::from("English")),
                        ("zh", String::from("中文")),
                    ],
                    action: language_action,
                },
                cx,
            ),
            self.appearance_row(cx),
            self.font_size_row(cx),
            self.select_row(
                SelectRow {
                    id: "set-transcript",
                    title: "settings.transcriptTitle",
                    desc: Some("settings.transcriptDescription"),
                    current: transcript,
                    options: vec![
                        ("compact", tr(lang, "settings.transcriptCompact")),
                        ("standard", tr(lang, "settings.transcriptStandard")),
                        ("detailed", tr(lang, "settings.transcriptDetailed")),
                        ("verbose", tr(lang, "settings.transcriptVerbose")),
                    ],
                    action: transcript_action,
                },
                cx,
            ),
            self.developer_tools_row(developer_tools, failed, cx),
            self.shortcuts_row(cx),
            self.select_row(
                SelectRow {
                    id: "set-links",
                    title: "settings.linksTitle",
                    desc: Some("settings.linksDescription"),
                    current: links,
                    options: vec![
                        ("sidebar", tr(lang, "settings.linksSidebar")),
                        ("new-tab", tr(lang, "settings.linksNewTab")),
                    ],
                    action: link_opening_action,
                },
                cx,
            ),
            self.select_row(
                SelectRow {
                    id: "set-enter",
                    title: "settings.enterTitle",
                    desc: Some("settings.enterDescription"),
                    current: enter,
                    options: vec![
                        ("queue", tr(lang, "settings.enterQueue")),
                        ("steer", tr(lang, "settings.enterSteer")),
                    ],
                    action: composer_enter_action,
                },
                cx,
            ),
            self.select_row(
                SelectRow {
                    id: "set-perf",
                    title: "settings.performanceTitle",
                    desc: Some("settings.performanceDescription"),
                    current: performance,
                    options: vec![
                        ("compact", tr(lang, "settings.performanceCompact")),
                        ("detailed", tr(lang, "settings.performanceDetailed")),
                    ],
                    action: performance_usage_action,
                },
                cx,
            ),
            self.current_version_row(),
        ];
        if self.shortcuts_open {
            rows.push(self.shortcuts_disclosure(cx));
        }
        ("settings.general", "settings.generalHint", rows)
    }

    /// The Show coding view row: the 24px-gap variant of the shared anatomy,
    /// whose error line is the reference's own `developerTools.error`.
    fn developer_tools_row(&self, on: bool, failed: bool, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let mut text = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(4.0))
            .min_w_0()
            .pr(px(48.0))
            .child(txt(
                tr(lang, "settings.developerToolsTitle"),
                14.0,
                theme::text(),
            ))
            .child(txt(
                tr(lang, "settings.developerToolsDescription"),
                12.0,
                theme::faint(),
            ));
        if failed {
            text = text.child(txt(
                tr(lang, "settings.developerToolsError"),
                12.0,
                theme::error(),
            ));
        }
        hairline_bottom(
            div()
                .py(px(16.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(24.0))
                .border_color(rgb(theme::border_soft()))
                .child(text)
                .child(self.switch(
                    "settings-developer-tools",
                    on,
                    true,
                    developer_tools_action,
                    cx,
                )),
        )
        .into_any_element()
    }

    /// What the reference's Keyboard shortcuts row opens, in the only form this
    /// shell can honour: gpui-kit's keymap holds no bindings here (`bind_keys` is
    /// never called in this crate), so there is nothing to edit and the row says
    /// so with the reference's own `shortcuts.unbound` wording rather than
    /// opening an empty editor.
    fn shortcuts_disclosure(&self, cx: &Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let keys: Vec<String> = {
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            keymap
                .bindings()
                .map(|binding| {
                    let chord: Vec<String> = binding
                        .keystrokes()
                        .iter()
                        .map(|stroke| stroke.to_string())
                        .collect();
                    format!("{}  {}", chord.join(" "), binding.action().name())
                })
                .collect()
        };
        let mut body = div().flex().flex_col().gap(px(6.0)).pb(px(16.0));
        if keys.is_empty() {
            body = body.child(txt(
                tr(lang, "settings.shortcutsUnbound"),
                12.0,
                theme::faint(),
            ));
        } else {
            for line in keys {
                body = body.child(txt(line, 12.0, theme::muted()));
            }
        }
        body.into_any_element()
    }

    /// Models: the endpoint and the system prompt.
    /// Agent presets: the declarative agent a preset file names.
    ///
    /// Selecting one is not a label - the preset supplies the model, prompt and
    /// step ceiling through effective(), and narrows the tools the agent is
    /// built with, so pressing a row changes what the next turn runs on.
    fn section_agent_presets(
        &self,
        cx: &mut Context<Shell>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let draft = self.state.draft();
        let active = draft.preset.clone();
        let listed: Vec<String> = draft
            .presets
            .iter()
            .map(|preset| preset.name.clone())
            .collect();
        let mut rows: Vec<AnyElement> = vec![div()
            .debug_selector(|| "preset-default".to_string())
            .id(SharedString::from("preset-default"))
            .px(px(6.0))
            .py(px(6.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::hover())))
            .when(active.is_empty(), |element| {
                element.bg(rgb(theme::selected_row()))
            })
            .child(txt(
                tr(self.state.lang, "settings.presetDefault"),
                13.0,
                theme::text(),
            ))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.state.select_preset("");
                cx.notify();
            }))
            .into_any_element()];
        for (index, name) in listed.into_iter().enumerate() {
            let selected = name == active;
            let picked = name.clone();
            rows.push(
                div()
                    .debug_selector(move || format!("preset-{index}"))
                    .id(SharedString::from(format!("preset-{index}")))
                    .px(px(6.0))
                    .py(px(6.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .when(selected, |element| element.bg(rgb(theme::selected_row())))
                    .child(txt(name, 13.0, theme::text()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.select_preset(&picked);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        ("settings.agent-presets", "settings.agentPresetsHint", rows)
    }

    /// The Models pane, in the reference's own three parts.
    ///
    /// Measured from `ModelsSection.module.css`: a `rows` list of provider cards
    /// 8px apart with 12px above it, and one add block below. Each card is a
    /// `rowCard` whose head is the provider's identity - its name, the `Custom`
    /// tag for a hand-declared route, and the API-key dot - beside its Edit and
    /// Delete actions, with the editor opening inside the same card. The add
    /// block shows its Add button, or the add card while that is open, never both.
    fn section_models(
        &self,
        cx: &mut Context<Shell>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        // Discovered profiles first, then saved ones; only the saved ones are
        // bsh's own rows to remove, because the rest are rewritten from the BOS
        // config on every load.
        let listed: Vec<(crate::settings::Profile, bool)> = {
            let draft = self.state.draft();
            let mut listed: Vec<(crate::settings::Profile, bool)> = draft
                .profiles
                .iter()
                .map(|profile| (profile.clone(), false))
                .collect();
            listed.extend(
                draft
                    .providers
                    .iter()
                    .map(|profile| (profile.clone(), true)),
            );
            listed
        };
        let mut cards: Vec<AnyElement> = Vec::new();
        for (index, (profile, removable)) in listed.iter().enumerate() {
            cards.push(self.provider_card(profile, *removable, index, cx));
        }
        let rows: Vec<AnyElement> = vec![
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .mt(px(12.0))
                .children(cards)
                .into_any_element(),
            self.add_provider_block(cx),
        ];
        ("settings.models", "settings.modelsHint", rows)
    }

    /// One provider card: the measured `rowCard` head, and the editor under it
    /// while this provider is the one being edited.
    fn provider_card(
        &self,
        profile: &crate::settings::Profile,
        removable: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let open = self.state.editing_provider.as_deref() == Some(profile.name.as_str());

        // `rowIdentity`: the name, the hand-declared tag, the API-key dot.
        // bsh keeps "which endpoint the agent is built against" here, behind a
        // click on the name, because its composer chip picks a MODEL and not a
        // provider. The reference has no such control on this page - its model
        // chip owns the choice - so this is the one BSH-only affordance on the
        // card, and it deliberately adds no chrome of its own.
        let select_name = profile.name.clone();
        let mut identity = div()
            .id(SharedString::from(format!("provider-{index}")))
            .debug_selector(move || format!("provider-{index}"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .cursor_pointer()
            .child(txt(profile.name.clone(), 14.0, theme::text()).font_weight(FontWeight::MEDIUM));
        if removable {
            identity = identity.child(hairline_all(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .border_color(rgb(theme::border()))
                    .child(txt(tr(lang, "settings.customTag"), 11.0, theme::muted())),
            ));
        }
        // The reference's dot is green only for a credential it confirmed
        // present, and red only for a named reference it proved missing. bsh
        // holds the key as a plain string and names no reference, so no state
        // here could prove one missing and the missing dot is never drawn.
        if !profile.api_key.trim().is_empty() {
            identity = identity.child(
                div()
                    .debug_selector(move || format!("provider-key-{index}"))
                    .flex_none()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(rgb(theme::success())),
            );
        }
        identity = identity.on_click(cx.listener(move |this, _event, _window, cx| {
            this.state.select_provider(&select_name);
            cx.notify();
        }));

        // `rowActions`: 4px apart, pushed to the right edge.
        let mut actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .ml_auto();
        let edit_profile = profile.clone();
        actions = actions.child(
            div()
                .id(SharedString::from(format!("provider-edit-{index}")))
                .debug_selector(move || format!("provider-edit-{index}"))
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::hover())))
                .child(txt(tr(lang, "settings.editProvider"), 12.0, theme::text()))
                .on_click(cx.listener(move |this, _event, window, cx| {
                    if this.state.editing_provider.as_deref() == Some(edit_profile.name.as_str()) {
                        this.state.editing_provider = None;
                        this.state.provider_refusal = None;
                    } else {
                        this.models_add_open = false;
                        this.models_advanced_open = false;
                        load_provider_into_form(this, &edit_profile, window, cx);
                    }
                    cx.notify();
                })),
        );
        if removable {
            let name = profile.name.clone();
            actions = actions.child(
                div()
                    .id(SharedString::from(format!("provider-remove-{index}")))
                    .debug_selector(move || format!("provider-remove-{index}"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(txt(tr(lang, "settings.modelsRemove"), 12.0, theme::error()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.begin_provider_remove(&name);
                        cx.notify();
                    })),
            );
        }

        let mut card = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(identity)
                .child(actions),
        );
        if open {
            if let Some(key) = self.state.provider_refusal {
                card = card.child(self.settings_note(key, true));
            }
            card = card.child(self.provider_editor(profile, cx));
        }
        hairline_all(
            card.py(px(12.0))
                .px(px(14.0))
                .rounded(px(16.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border_soft())),
        )
        .into_any_element()
    }

    /// The editor a card opens: the reference's API-key field over its
    /// Customized settings fold, with the Apply/Cancel pair under both.
    fn provider_editor(
        &self,
        profile: &crate::settings::Profile,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let inputs = &self.settings_inputs;
        let mut body: Vec<AnyElement> = vec![self.editor_field(
            "settings.apiKey",
            self.editor_input(&inputs.api_key, "field-editor-api-key".to_string()),
        )];

        // The fold carries what the reference keeps behind it: the endpoint, the
        // protocol, and the model catalog. Only the key shows above it.
        let fold_open = self.models_advanced_open;
        let mut fold = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pt(px(10.0))
            .border_t_1()
            .border_color(rgb(theme::border_soft()))
            .child(
                div()
                    .id("models-customized")
                    .debug_selector(|| "models-customized".to_string())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .child(if fold_open {
                        icon(IconName::ChevronDown, 12.0, theme::muted())
                    } else {
                        icon(IconName::ChevronRight, 12.0, theme::muted())
                    })
                    .child(
                        txt(tr(lang, "settings.customized"), 12.0, theme::muted())
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.models_advanced_open = !this.models_advanced_open;
                        cx.notify();
                    })),
            );
        if fold_open {
            let mut inner: Vec<AnyElement> = vec![
                self.editor_field(
                    "settings.customBaseUrl",
                    self.editor_input(&inputs.base_url, "field-editor-base-url".to_string()),
                ),
                self.editor_field("settings.apiMode", self.api_mode_choices(cx)),
                txt(tr(lang, "settings.advancedHint"), 12.0, theme::faint()).into_any_element(),
            ];
            inner.extend(self.catalog_rows(cx));
            inner.extend(self.models_picker_rows(cx));
            fold = fold.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .pt(px(12.0))
                    .children(inner),
            );
        }
        body.push(fold.into_any_element());

        let original = profile.name.clone();
        let apply = div()
            .id("models-apply")
            .debug_selector(|| "models-apply".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .bg(rgb(theme::text()))
            .cursor_pointer()
            .hover(|s| s.opacity(0.9))
            .child(txt(tr(lang, "settings.apply"), 14.0, theme::page()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                let typed = this
                    .settings_inputs
                    .provider_name
                    .read(cx)
                    .value()
                    .trim()
                    .to_string();
                this.settings_inputs.read(this.state.draft_mut(), cx);
                let outcome = this.state.update_provider(&original, &typed);
                this.state.provider_refusal = outcome.err().map(|refusal| refusal.key());
                if this.state.provider_refusal.is_none() {
                    this.state.editing_provider = None;
                }
                cx.notify();
            }));
        let cancel = div()
            .id("models-editor-cancel")
            .debug_selector(|| "models-editor-cancel".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(txt(tr(lang, "settings.cancel"), 14.0, theme::text()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.state.editing_provider = None;
                this.state.provider_refusal = None;
                cx.notify();
            }));
        body.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(cancel)
                .child(apply)
                .into_any_element(),
        );

        div()
            .debug_selector(|| "provider-editor".to_string())
            .flex()
            .flex_col()
            .gap(px(14.0))
            .px(px(16.0))
            .py(px(14.0))
            .rounded(px(16.0))
            .bg(rgb(theme::new_session_fill()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(8.0))
                    .child(
                        txt(profile.name.clone(), 14.0, theme::text())
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(txt(profile.model.clone(), 12.0, theme::faint())),
            )
            .children(body)
            .into_any_element()
    }

    /// One editor field: the measured `field` - a 12px/500 label over its
    /// control, 6px apart.
    fn editor_field(&self, label: &'static str, control: AnyElement) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .min_w_0()
            .child(
                txt(tr(self.state.lang, label), 12.0, theme::muted())
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(control)
            .into_any_element()
    }

    /// The measured 32px editor input on the layer-1 fill.
    fn editor_input(&self, input: &Entity<InputState>, id: String) -> AnyElement {
        let selector = id.clone();
        hairline_all(
            div()
                .id(SharedString::from(id))
                .debug_selector(move || selector)
                .h(px(32.0))
                .rounded(px(8.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border()))
                .child(Input::new(input).appearance(false)),
        )
        .into_any_element()
    }

    /// The API-protocol chips, stacked the way the reference stacks them.
    fn api_mode_choices(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let current = {
            let draft = self.state.draft();
            if draft.api_mode.is_empty() {
                "auto".to_string()
            } else {
                draft.api_mode.clone()
            }
        };
        let options = [
            ("auto", "settings.apiAuto"),
            ("chat", "settings.apiChat"),
            ("responses", "settings.apiResponses"),
            ("anthropic", "settings.apiAnthropic"),
        ];
        let mut group = div().flex().flex_row().flex_wrap().gap(px(6.0));
        for (key, label) in options {
            let selected = key == current;
            group = group.child(
                div()
                    .id(SharedString::from(format!("set-api-mode-{key}")))
                    .debug_selector(move || format!("set-api-mode-{key}"))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    }))
                    .bg(rgb(if selected {
                        theme::accent()
                    } else {
                        theme::page()
                    }))
                    .cursor_pointer()
                    .hover(move |s| {
                        s.bg(rgb(if selected {
                            theme::accent()
                        } else {
                            theme::hover()
                        }))
                    })
                    .child(txt(
                        tr(lang, label),
                        theme::CHIP_SIZE,
                        if selected {
                            theme::page()
                        } else {
                            theme::muted()
                        },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        api_mode_action(this, key, cx);
                    })),
            );
        }
        group.into_any_element()
    }

    /// The add block: the Add button, or the open add card.
    fn add_provider_block(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        if !self.models_add_open {
            return div()
                .flex()
                .child(hairline_all(
                    div()
                        .id("provider-add")
                        .debug_selector(|| "provider-add".to_string())
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .flex_1()
                        .min_w(px(180.0))
                        .h(px(44.0))
                        .rounded(px(16.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .border_color(rgb(theme::border()))
                        .child(icon(IconName::Plus, 14.0, theme::text()))
                        .child(txt(
                            tr(lang, "settings.addModelProvider"),
                            14.0,
                            theme::text(),
                        ))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.state.editing_provider = None;
                            this.state.provider_refusal = None;
                            this.models_add_open = true;
                            this.models_advanced_open = false;
                            cx.notify();
                        })),
                ))
                .into_any_element();
        }

        let inputs = &self.settings_inputs;
        let original = self.state.editing_provider.clone().unwrap_or_default();
        let create_label = tr(lang, "settings.createProvider");
        let create = div()
            .id("models-create")
            .debug_selector(|| "models-create".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .bg(rgb(theme::text()))
            .cursor_pointer()
            .hover(|s| s.opacity(0.9))
            .child(txt(create_label, 14.0, theme::page()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                let typed = this
                    .settings_inputs
                    .provider_name
                    .read(cx)
                    .value()
                    .trim()
                    .to_string();
                this.settings_inputs.read(this.state.draft_mut(), cx);
                let outcome = if original.is_empty() {
                    this.state.add_provider(&typed)
                } else {
                    this.state.update_provider(&original, &typed)
                };
                this.state.provider_refusal = outcome.err().map(|refusal| refusal.key());
                if this.state.provider_refusal.is_none() {
                    this.state.editing_provider = None;
                    this.models_add_open = false;
                    this.models_advanced_open = false;
                }
                cx.notify();
            }));
        let cancel = div()
            .id("models-add-cancel")
            .debug_selector(|| "models-add-cancel".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(txt(tr(lang, "settings.cancel"), 14.0, theme::text()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.models_add_open = false;
                this.models_advanced_open = false;
                this.state.editing_provider = None;
                this.state.provider_refusal = None;
                cx.notify();
            }));

        let panel: Vec<AnyElement> = vec![
            self.editor_field(
                "settings.customRoute",
                self.editor_input(&inputs.provider_name, "field-provider-id".to_string()),
            ),
            self.editor_field(
                "settings.modelId",
                self.editor_input(&inputs.model, "field-new-model".to_string()),
            ),
            self.editor_field(
                "settings.customBaseUrl",
                self.editor_input(&inputs.base_url, "field-new-base-url".to_string()),
            ),
            self.editor_field(
                "settings.apiKey",
                self.editor_input(&inputs.api_key, "field-new-api-key".to_string()),
            ),
            self.editor_field("settings.apiMode", self.api_mode_choices(cx)),
            txt(tr(lang, "settings.customRouteHint"), 12.0, theme::faint()).into_any_element(),
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(cancel)
                .child(create)
                .into_any_element(),
        ];

        div()
            .debug_selector(|| "models-add-card".to_string())
            .flex()
            .flex_col()
            .gap(px(14.0))
            .px(px(16.0))
            .py(px(14.0))
            .rounded(px(16.0))
            .bg(rgb(theme::new_session_fill()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.0))
                    .child(self.models_mode_switch())
                    .child(txt(
                        tr(lang, "settings.addCustomHint"),
                        12.0,
                        theme::faint(),
                    )),
            )
            .child(div().flex().flex_col().gap(px(14.0)).children(panel))
            .into_any_element()
    }

    /// The reference's SegmentedControl: a 4px-padded track on the hover fill
    /// whose selected segment is a raised layer-1 pill.
    ///
    /// The catalog mode is disabled because bsh composes no provider directory,
    /// so it has nothing to list. `addCatalogExhausted` - "Every catalog
    /// provider is already configured." - is literally true here, and the
    /// reference puts a disabled mode's reason in its hover text.
    fn models_mode_switch(&self) -> AnyElement {
        let lang = self.state.lang;
        let mut track = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .p(px(4.0))
            .rounded(px(8.0))
            .bg(rgb(theme::hover()));
        let segments: [(&'static str, &'static str, bool); 2] = [
            ("catalog", "settings.addCatalog", false),
            ("custom", "settings.addCustom", true),
        ];
        for (id, label, selected) in segments {
            let base = div()
                .id(SharedString::from(format!("models-mode-{id}")))
                .debug_selector(move || format!("models-mode-{id}"))
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.0))
                .px(px(16.0))
                .rounded(px(6.0));
            let text = txt(
                tr(lang, label),
                theme::CHIP_SIZE,
                if selected {
                    theme::text()
                } else {
                    theme::muted()
                },
            );
            let seg: AnyElement = if selected {
                hairline_all(
                    base.bg(rgb(theme::page()))
                        .border_color(rgb(theme::border_soft())),
                )
                .child(text)
                .into_any_element()
            } else {
                let reason = SharedString::from(tr(lang, "settings.addCatalogExhausted"));
                base.opacity(0.4)
                    .tooltip(move |window, cx| Tooltip::new(reason.clone()).build(window, cx))
                    .child(text)
                    .into_any_element()
            };
            track = track.child(seg);
        }
        track.into_any_element()
    }

    /// Tools and skills, with a switch each.
    fn section_tools(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let mut rows: Vec<AnyElement> = vec![self.settings_note("settings.toolsHint", false)];
        if self.caps.is_empty() {
            rows.push(
                txt(tr(self.state.lang, "settings.empty"), 13.0, theme::muted())
                    .pt(px(10.0))
                    .into_any_element(),
            );
        }
        for (index, cap) in self.caps.iter().enumerate() {
            let name = cap.name.clone();
            let category = cap.category.clone();
            let description = cap.description.clone();
            let off = if category == "skill" {
                self.state
                    .draft()
                    .disabled_skills
                    .iter()
                    .any(|s| s == &name)
            } else {
                self.state.draft().disabled_tools.iter().any(|t| t == &name)
            };
            let is_skill = category == "skill";
            rows.push(
                div()
                    .id(SharedString::from(format!("cap-{index}")))
                    .py(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(rgb(theme::border_soft()))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(if off {
                                theme::border()
                            } else {
                                theme::accent()
                            }))
                            .bg(rgb(if off { theme::page() } else { theme::accent() }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if off {
                                div().into_any_element()
                            } else {
                                icon(IconName::Check, 11.0, theme::page()).into_any_element()
                            }),
                    )
                    .child(txt(name.clone(), 13.0, theme::text()).w(px(150.0)))
                    .child(
                        div()
                            .w(px(52.0))
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(theme::new_session_fill()))
                            .child(txt(category, 11.0, theme::muted())),
                    )
                    .child(txt(description, 12.0, theme::faint()).flex_1())
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        if is_skill {
                            this.state.toggle_skill_off(&name);
                        } else {
                            this.state.toggle_tool_off(&name);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        ("settings.tools", "settings.toolsHint", rows)
    }

    /// Select one catalog model as the active model, the row action.
    pub(crate) fn catalog_select(&mut self, model: &str) {
        self.state.draft_mut().model = model.to_string();
    }

    /// Add a typed model id to the active provider catalog.
    ///
    /// Refusals are ignored here on purpose: the pane derives its hint from
    /// the draft, so a blank or a repeat is explained by the list it draws.
    pub(crate) fn catalog_add(&mut self, typed: &str) {
        let provider = self.state.draft().active_provider.clone();
        let _ = self.state.draft_mut().add_catalog_model(&provider, typed);
    }

    /// Drop one model id from the active provider catalog.
    pub(crate) fn catalog_remove(&mut self, model: &str) {
        let provider = self.state.draft().active_provider.clone();
        self.state
            .draft_mut()
            .remove_catalog_model(&provider, model);
    }

    /// Toggle one declared input type on a catalog row.
    pub(crate) fn catalog_toggle_input(&mut self, model: &str, kind: &str, cx: &mut Context<Self>) {
        let provider = self.state.draft().active_provider.clone();
        self.state
            .draft_mut()
            .toggle_input_type(&provider, model, kind);
        cx.notify();
    }
    /// The active provider customized model catalog.
    ///
    /// The reference keeps this list in its provider editor: one row per model
    /// id the user added, each of which selects itself as the active model, a
    /// delete control beside it, and an add row that takes whatever the Model ID
    /// field holds. The hints are derived from the draft instead of remembered,
    /// so a refusal is explained without the pane holding any state of its own.
    /// The fetched-model picker: the reference Choose models to add flow.
    ///
    /// The list is what a real GET /models returned through the react vendor
    /// seam, so the heading only appears once a fetch has delivered one. The
    /// reference candidate search is drawn under its own measured wording
    /// (fetchSearch), and it narrows what the select-all pair acts on, which is
    /// what the reference does: its own button is disabled with no visible
    /// candidate and toggles exactly the visible ones.
    fn models_picker_rows(&self, cx: &mut Context<Shell>) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = vec![div()
            .debug_selector(|| "models-fetch".to_string())
            .id(SharedString::from("models-fetch"))
            .py(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(icon(IconName::ArrowUpDown, 12.0, theme::accent()))
            .child(txt(
                tr(self.state.lang, "settings.fetchModels"),
                13.0,
                theme::accent(),
            ))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.fetch_models(cx);
                cx.notify();
            }))
            .into_any_element()];
        if self.models_fetch_needs_base_url {
            rows.push(
                txt(
                    tr(self.state.lang, "settings.fetchNeedsBaseUrl"),
                    12.0,
                    theme::warn(),
                )
                .pb(px(6.0))
                .into_any_element(),
            );
        }
        if self.models_fetch_failed {
            rows.push(
                txt(
                    tr(self.state.lang, "settings.fetchFailed"),
                    12.0,
                    theme::error(),
                )
                .pb(px(6.0))
                .into_any_element(),
            );
        }
        let Some(models) = self.models_fetched.clone() else {
            return rows;
        };
        rows.push(
            txt(
                tr(self.state.lang, "settings.chooseModels"),
                13.0,
                theme::text(),
            )
            .pt(px(8.0))
            .pb(px(4.0))
            .into_any_element(),
        );
        let query = self.state.picker_query.clone();
        let visible: Vec<String> = models
            .iter()
            .filter(|model| crate::state::matches(&query, model))
            .cloned()
            .collect();
        if models.is_empty() {
            rows.push(
                txt(
                    tr(self.state.lang, "settings.fetchEmpty"),
                    12.0,
                    theme::faint(),
                )
                .pb(px(6.0))
                .into_any_element(),
            );
            return rows;
        }
        rows.push(
            div()
                .mb(px(6.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(rgb(theme::new_session_fill()))
                .border_1()
                .border_color(rgb(theme::border_soft()))
                .child(Input::new(&self.picker_search).appearance(false))
                .into_any_element(),
        );
        let all = visible.clone();
        let none = visible.clone();
        rows.push(
            div()
                .pb(px(4.0))
                .flex()
                .flex_row()
                .gap(px(10.0))
                .child(
                    txt(
                        tr(self.state.lang, "settings.selectAll"),
                        12.0,
                        theme::accent(),
                    )
                    .debug_selector(|| "fetched-select-all".to_string())
                    .id(SharedString::from("fetched-select-all"))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        for model in &all {
                            this.models_selected.insert(model.clone());
                        }
                        cx.notify();
                    })),
                )
                .child(
                    txt(
                        tr(self.state.lang, "settings.deselectAll"),
                        12.0,
                        theme::accent(),
                    )
                    .debug_selector(|| "fetched-deselect-all".to_string())
                    .id(SharedString::from("fetched-deselect-all"))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        for model in &none {
                            this.models_selected.remove(model);
                        }
                        cx.notify();
                    })),
                )
                .into_any_element(),
        );
        if visible.is_empty() {
            rows.push(
                div()
                    .debug_selector(|| String::from("fetched-none"))
                    .child(
                        txt(
                            tr(self.state.lang, "settings.noMatchingModels"),
                            12.0,
                            theme::faint(),
                        )
                        .pb(px(6.0)),
                    )
                    .into_any_element(),
            );
            return rows;
        }
        for (index, model) in visible.iter().enumerate() {
            let ticked = self.models_selected.contains(model);
            let id = model.clone();
            let label = model.clone();
            rows.push(
                div()
                    .debug_selector(move || format!("fetched-{index}"))
                    .id(SharedString::from(format!("fetched-{index}")))
                    .py(px(5.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(if ticked {
                                theme::accent()
                            } else {
                                theme::border()
                            }))
                            .bg(rgb(if ticked {
                                theme::accent()
                            } else {
                                theme::page()
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if ticked {
                                icon(IconName::Check, 11.0, theme::page()).into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    )
                    .child(txt(label, 13.0, theme::text()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_fetched_model(&id);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .debug_selector(|| "fetched-add".to_string())
                .id(SharedString::from("fetched-add"))
                .py(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(icon(IconName::Plus, 12.0, theme::accent()))
                .child(txt(
                    tr(self.state.lang, "settings.addSelected"),
                    13.0,
                    theme::accent(),
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.add_selected_models();
                    cx.notify();
                }))
                .into_any_element(),
        );
        rows
    }

    fn catalog_rows(&self, cx: &mut Context<Shell>) -> Vec<AnyElement> {
        let draft = self.state.draft();
        let models = draft.catalog(&draft.active_provider).to_vec();
        let typed = self
            .settings_inputs
            .model
            .read(cx)
            .value()
            .trim()
            .to_string();
        let mut rows: Vec<AnyElement> = vec![
            txt(
                tr(self.state.lang, "settings.modelCatalog"),
                13.0,
                theme::text(),
            )
            .pt(px(10.0))
            .pb(px(2.0))
            .into_any_element(),
            txt(
                tr(self.state.lang, "settings.modelCatalogHint"),
                12.0,
                theme::faint(),
            )
            .pb(px(6.0))
            .into_any_element(),
        ];
        if typed.is_empty() {
            rows.push(
                txt(
                    tr(self.state.lang, "settings.modelIdRequired"),
                    12.0,
                    theme::warn(),
                )
                .pb(px(6.0))
                .into_any_element(),
            );
        } else if models.iter().any(|model| model == &typed) {
            rows.push(
                txt(
                    tr(self.state.lang, "settings.modelIdUnique"),
                    12.0,
                    theme::warn(),
                )
                .pb(px(6.0))
                .into_any_element(),
            );
        }
        for (index, model) in models.iter().enumerate() {
            let selected = model == &draft.model;
            let select_name = model.clone();
            let remove_name = model.clone();
            let text = model.clone();
            rows.push(
                div()
                    .py(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .border_b_1()
                    .border_color(rgb(theme::border_soft()))
                    .child(
                        div()
                            .debug_selector(move || format!("catalog-{index}"))
                            .id(SharedString::from(format!("catalog-{index}")))
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_1()
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(5.0))
                            .when(selected, |element| element.bg(rgb(theme::selected_row())))
                            .child(txt(text, 13.0, theme::text()))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.catalog_select(&select_name);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("catalog-remove-{index}"))
                            .id(SharedString::from(format!("catalog-remove-{index}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(4.0))
                            .child(icon(IconName::X, 12.0, theme::muted()))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.catalog_remove(&remove_name);
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
            let chip_model = model.clone();
            let mut chips: Vec<AnyElement> = Vec::new();
            for (kind, key, id) in [
                ("text", "settings.inputText", "catalog-input-text"),
                ("image", "settings.inputImage", "catalog-input-image"),
            ] {
                let on = draft.shows_input_type(&draft.active_provider, model, kind);
                let kind = kind.to_string();
                let clicked = chip_model.clone();
                chips.push(
                    div()
                        .debug_selector(move || format!("{id}-{index}"))
                        .id(SharedString::from(format!("{id}-{index}")))
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(rgb(if on { theme::accent() } else { theme::border() }))
                        .when(on, |element| element.bg(rgb(theme::selected_row())))
                        .child(txt(
                            tr(self.state.lang, key),
                            12.0,
                            if on { theme::accent() } else { theme::muted() },
                        ))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.catalog_toggle_input(&clicked, &kind, cx);
                        }))
                        .into_any_element(),
                );
            }
            rows.push(
                div()
                    .pb(px(6.0))
                    .pl(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(txt(
                        tr(self.state.lang, "settings.inputTypes"),
                        12.0,
                        theme::faint(),
                    ))
                    .children(chips)
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .debug_selector(|| "catalog-add".to_string())
                .id(SharedString::from("catalog-add"))
                .py(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(icon(IconName::Plus, 12.0, theme::accent()))
                .child(txt(
                    tr(self.state.lang, "settings.addModel"),
                    13.0,
                    theme::accent(),
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    let typed = this
                        .settings_inputs
                        .model
                        .read(cx)
                        .value()
                        .trim()
                        .to_string();
                    this.catalog_add(&typed);
                    cx.notify();
                }))
                .into_any_element(),
        );
        rows
    }

    /// Agent loop: the step ceiling one turn may reach.
    fn section_agent_loop(
        &self,
        cx: &mut Context<Shell>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let inputs = &self.settings_inputs;
        let rows = vec![
            self.input_row(
                "settings.maxRecursionDepth",
                &inputs.max_steps,
                Field::MaxSteps,
                cx,
            ),
            {
                let on = self.state.draft().memory_enabled;
                // The reference's General pane has no memory row, so this one
                // carries the title alone: there is no measured description to
                // quote and inventing one would be our copy, not its.
                self.preference_row(
                    "settings.memoryTitle",
                    None,
                    self.switch("settings-memory", on, true, memory_action, cx),
                )
            },
            {
                let on = self.state.draft().parallel_tool_calls;
                div()
                    .debug_selector(|| "parallel-tool-calls".to_string())
                    .id(SharedString::from("parallel-tool-calls"))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .cursor_pointer()
                    .child(
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .bg(rgb(theme::page()))
                            .when(on, |element| {
                                element
                                    .bg(rgb(theme::accent()))
                                    .border_color(rgb(theme::accent()))
                                    .child(txt("✓", 12.0, theme::page()))
                            })
                            .into_any_element(),
                    )
                    .child(txt(
                        tr(self.state.lang, "settings.parallelToolCalls"),
                        13.0,
                        theme::text(),
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.set_parallel_tool_calls(!on);
                        cx.notify();
                    }))
                    .into_any_element()
            },
        ];
        ("settings.agent-loop", "settings.agentLoopHint", rows)
    }

    /// Shell: the two ceilings the shell tool obeys.
    fn section_shell(
        &self,
        cx: &mut Context<Shell>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let inputs = &self.settings_inputs;
        let rows = vec![
            self.input_row(
                "settings.shellTimeout",
                &inputs.shell_timeout,
                Field::ShellTimeout,
                cx,
            ),
            self.input_row(
                "settings.outputCap",
                &inputs.output_cap,
                Field::OutputCap,
                cx,
            ),
        ];
        ("settings.shell", "settings.shellHint", rows)
    }

    /// The plugin inventory: every plugin the agent was built from, with the
    /// tools it contributed.
    fn section_plugins(
        &self,
        cx: &mut Context<Shell>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let mut rows: Vec<AnyElement> = Vec::new();
        let query = self.state.plugin_query.clone();
        let visible = self
            .plugins
            .iter()
            .filter(|plugin| crate::state::matches(&query, &plugin.name))
            .count();
        rows.push(
            div()
                .mb(px(8.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(rgb(theme::new_session_fill()))
                .border_1()
                .border_color(rgb(theme::border_soft()))
                .child(Input::new(&self.plugin_search).appearance(false))
                .into_any_element(),
        );
        if self.plugins.is_empty() {
            rows.push(
                div()
                    .debug_selector(|| String::from("plugin-none"))
                    .child(txt(
                        tr(self.state.lang, "settings.noPlugins"),
                        13.0,
                        theme::muted(),
                    ))
                    .into_any_element(),
            );
        }
        if visible == 0 && !self.plugins.is_empty() {
            rows.push(
                div()
                    .debug_selector(|| String::from("plugin-empty"))
                    .child(txt(
                        tr(self.state.lang, "settings.noMatchingPlugins"),
                        13.0,
                        theme::muted(),
                    ))
                    .into_any_element(),
            );
        }
        for (index, plugin) in self
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, plugin)| crate::state::matches(&query, &plugin.name))
        {
            let enabled = plugin.enabled;
            let name = plugin.name.clone();
            let tools = if enabled {
                plugin.tools.join(", ")
            } else {
                tr(self.state.lang, "settings.pluginDisabled")
            };
            rows.push(
                div()
                    .py(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(rgb(theme::border_soft()))
                    .child(
                        div()
                            .debug_selector(|| format!("plugin-toggle-{index}"))
                            .id(SharedString::from(format!("plugin-toggle-{index}")))
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .bg(rgb(theme::page()))
                            .when(enabled, |element| {
                                element
                                    .bg(rgb(theme::accent()))
                                    .border_color(rgb(theme::accent()))
                                    .child(
                                        icon(IconName::Check, 11.0, theme::page())
                                            .into_any_element(),
                                    )
                            })
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.state.toggle_plugin_off(&name);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .debug_selector(|| format!("plugin-{index}"))
                            .child(txt(plugin.name.clone(), 13.0, theme::text())),
                    )
                    .child(txt(tools, 12.0, theme::muted()))
                    .into_any_element(),
            );
        }
        ("settings.plugin-inventory", "settings.pluginListHint", rows)
    }

    /// The reference own Built-in plugins pane.
    ///
    /// This is the plugin-level view: which plugins this deployment ships and
    /// whether each is on. bsh contributes no plugin views at all, which the
    /// reference answers with a sentence of its own, so that sentence is drawn
    /// instead of an invented empty state. The switches themselves live in the
    /// inventory pane, beside the tools each plugin contributed.
    /// The reference own Automation tasks page.
    ///
    /// The reference renders it as a sidebar PANEL whose id is `schedules`;
    /// bsh reaches it from the same sidebar row through its settings rail, the
    /// way its plugins page already does. The rows come from the DRAFT, so the
    /// pane own Save is what persists a task, and the shell scheduler is what
    /// runs one when it comes due.
    fn section_automation(
        &self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let lang = self.state.lang;
        let query = self.task_search.read(cx).value().to_string();
        let filter = self.task_filter;
        let now = crate::settings::unix_now();
        let draft = self.state.draft();
        let total = draft.schedules.len();
        let tasks: Vec<(String, String, String, bool, String)> = draft
            .schedules
            .iter()
            .filter(|task| crate::state::matches(&query, &task.name))
            .filter(|task| match filter {
                1 => task.enabled,
                2 => !task.enabled,
                _ => true,
            })
            .map(|task| {
                (
                    task.id.clone(),
                    task.name.clone(),
                    self.cadence_text(task.every_seconds),
                    task.enabled,
                    draft
                        .next_run(task, now)
                        .map(|at| self.countdown_text(at, now))
                        .unwrap_or_default(),
                )
            })
            .collect();

        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .mb(px(8.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(rgb(theme::new_session_fill()))
                .border_1()
                .border_color(rgb(theme::border_soft()))
                .child(Input::new(&self.task_search).appearance(false))
                .into_any_element(),
        );

        let mut filters = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .mb(px(10.0));
        for (value, key, id) in [
            (0usize, "task.statusAll", "task-filter-all"),
            (1usize, "task.statusEnabled", "task-filter-enabled"),
            (2usize, "task.statusInactive", "task-filter-inactive"),
        ] {
            let active = filter == value;
            filters = filters.child(
                div()
                    .debug_selector(move || id.to_string())
                    .id(SharedString::from(id))
                    .px(px(10.0))
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(if active {
                        theme::accent()
                    } else {
                        theme::border_soft()
                    }))
                    .cursor_pointer()
                    .child(txt(
                        tr(lang, key),
                        12.0,
                        if active {
                            theme::accent()
                        } else {
                            theme::muted()
                        },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.task_filter = value;
                        cx.notify();
                    })),
            );
        }
        rows.push(filters.into_any_element());
        rows.push(
            div()
                .mb(px(4.0))
                .child(txt(tr(lang, "task.list"), 12.0, theme::muted()))
                .into_any_element(),
        );

        if total == 0 {
            rows.push(
                div()
                    .debug_selector(|| String::from("task-none"))
                    .child(txt(tr(lang, "task.empty"), 13.0, theme::muted()))
                    .into_any_element(),
            );
        } else if tasks.is_empty() {
            rows.push(
                div()
                    .debug_selector(|| String::from("task-no-matches"))
                    .child(txt(tr(lang, "task.noMatches"), 13.0, theme::muted()))
                    .into_any_element(),
            );
        }

        for (index, (id, name, cadence, enabled, next)) in tasks.iter().enumerate() {
            let toggle_id = id.clone();
            let remove_id = id.clone();
            let detail = if next.is_empty() {
                cadence.clone()
            } else {
                format!("{} · {}{}", cadence, tr(lang, "task.nextPrefix"), next)
            };
            rows.push(
                div()
                    .debug_selector(move || format!("task-{index}"))
                    .mb(px(6.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(txt(name.clone(), 13.0, theme::text()))
                    .child(txt(detail, 12.0, theme::muted()))
                    .child(
                        div()
                            .debug_selector({
                                let debug_id = toggle_id.clone();
                                move || format!("task-toggle-{debug_id}")
                            })
                            .id(SharedString::from(format!("task-toggle-{toggle_id}")))
                            .px(px(8.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(if *enabled {
                                theme::accent()
                            } else {
                                theme::border_soft()
                            }))
                            .cursor_pointer()
                            .child(txt(
                                tr(
                                    lang,
                                    if *enabled {
                                        "task.statusEnabled"
                                    } else {
                                        "task.statusInactive"
                                    },
                                ),
                                12.0,
                                if *enabled {
                                    theme::accent()
                                } else {
                                    theme::muted()
                                },
                            ))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.task_toggle(&toggle_id, cx);
                            })),
                    )
                    .child(
                        div()
                            .debug_selector({
                                let debug_id = remove_id.clone();
                                move || format!("task-delete-{debug_id}")
                            })
                            .id(SharedString::from(format!("task-delete-{remove_id}")))
                            .px(px(8.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(theme::border_soft()))
                            .cursor_pointer()
                            .child(txt(tr(lang, "task.deleteAction"), 12.0, theme::muted()))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.task_request_delete(&remove_id, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }

        rows.push(
            div()
                .mt(px(14.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(txt(tr(lang, "task.new"), 13.0, theme::text()))
                .child(txt(tr(lang, "task.nameLabel"), 12.0, theme::muted()))
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::new_session_fill()))
                        .border_1()
                        .border_color(rgb(theme::border_soft()))
                        .child(Input::new(&self.task_name).appearance(false)),
                )
                .child(txt(tr(lang, "task.instructionLabel"), 12.0, theme::muted()))
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::new_session_fill()))
                        .border_1()
                        .border_color(rgb(theme::border_soft()))
                        .child(Input::new(&self.task_prompt).appearance(false)),
                )
                .child(txt(tr(lang, "task.intervalLabel"), 12.0, theme::muted()))
                .child(
                    div()
                        .w(px(120.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::new_session_fill()))
                        .border_1()
                        .border_color(rgb(theme::border_soft()))
                        .child(Input::new(&self.task_every).appearance(false)),
                )
                .child(
                    div()
                        .debug_selector(|| String::from("task-create"))
                        .id("task-create")
                        .px(px(10.0))
                        .h(px(26.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .bg(rgb(theme::accent()))
                        .cursor_pointer()
                        .child(txt(tr(lang, "task.new"), 12.0, theme::page()))
                        .on_click(cx.listener(|this, _event, _window, cx| this.task_add(cx))),
                )
                .into_any_element(),
        );
        ("task.title", "", rows)
    }

    fn cadence_text(&self, every_seconds: u64) -> String {
        let lang = self.state.lang;
        if every_seconds == 0 {
            return tr(lang, "task.frequencyOnce");
        }
        let (value, unit) = self.amount_text(every_seconds);
        tr(lang, "task.frequencyEvery")
            .replacen("{value}", &value.to_string(), 1)
            .replacen("{unit}", &unit, 1)
    }

    fn countdown_text(&self, at: u64, now: u64) -> String {
        let lang = self.state.lang;
        if at <= now {
            return tr(lang, "task.dueNow");
        }
        let (value, unit) = self.amount_text(at - now);
        tr(lang, "task.inFuture")
            .replacen("{value}", &value.to_string(), 1)
            .replacen("{unit}", &unit, 1)
    }

    fn amount_text(&self, seconds: u64) -> (u64, String) {
        let lang = self.state.lang;
        let (value, one, other) = if seconds < 60 {
            (seconds, "task.unitSecondOne", "task.unitSecondOther")
        } else if seconds < 3_600 {
            (seconds / 60, "task.unitMinuteOne", "task.unitMinuteOther")
        } else {
            (seconds / 3_600, "task.unitHourOne", "task.unitHourOther")
        };
        let key = if value == 1 { one } else { other };
        (value, tr(lang, key))
    }

    /// Create a task from the pane own fields.
    pub(crate) fn task_add(&mut self, cx: &mut Context<Self>) {
        let lang = self.state.lang;
        let name = self.task_name.read(cx).value().to_string();
        let prompt = self.task_prompt.read(cx).value().to_string();
        let minutes: u64 = self
            .task_every
            .read(cx)
            .value()
            .trim()
            .parse()
            .unwrap_or(30);
        let session = self.state.settings.active_session.clone();
        let refusal =
            self.state
                .draft_mut()
                .add_schedule(&name, &prompt, &session, minutes.max(1) * 60);
        if let Err(refusal) = refusal {
            let key = match refusal {
                crate::settings::ScheduleRefusal::Name => "task.invalidTitle",
                crate::settings::ScheduleRefusal::Prompt => "task.invalidPrompt",
            };
            let text = tr(lang, key).to_string();
            self.state.notice(text);
        }
        cx.notify();
    }

    /// Flip one task between enabled and inactive.
    pub(crate) fn task_toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        self.state.draft_mut().toggle_schedule(id);
        cx.notify();
    }

    /// Delete one task, under the reference own confirmation sentence.
    /// Ask before deleting, the way the reference does: the row control opens a
    /// confirmation instead of removing the task.
    pub(crate) fn task_request_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.task_delete = Some(id.to_string());
        cx.notify();
    }

    /// The confirmation own button. A refusal reports the reference own failure
    /// sentence rather than looking like it worked.
    pub(crate) fn task_confirm_delete(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.task_delete.take() {
            let removed = self.state.draft_mut().remove_schedule(&id);
            let key = if removed {
                "task.deleted"
            } else {
                "task.deleteFailed"
            };
            let text = tr(self.state.lang, key).to_string();
            self.state.notice(text);
        }
        cx.notify();
    }

    /// Backing out leaves the task in place.
    pub(crate) fn task_cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.task_delete = None;
        cx.notify();
    }
    /// Flip the risk gate's acknowledgement box.
    ///
    /// Measured (`RiskConfirmation`): the box starts unticked every time the
    /// gate opens, so acknowledging one mode never acknowledges the next one.
    pub(crate) fn permission_toggle_acknowledged(&mut self, cx: &mut Context<Self>) {
        self.permission_acknowledged = !self.permission_acknowledged;
        cx.notify();
    }

    /// Confirm the gate: the mode is written only now, and only once the box is
    /// ticked - which is what keeps the gate from being a second menu.
    pub(crate) fn permission_confirm_enable(&mut self, cx: &mut Context<Self>) {
        if !self.permission_acknowledged {
            return;
        }
        if let Some(permission) = self.permission_confirm.take() {
            self.permission_acknowledged = false;
            self.set_permission(&permission, cx);
        }
        cx.notify();
    }

    /// Backing out leaves the current mode alone.
    pub(crate) fn permission_cancel_confirm(&mut self, cx: &mut Context<Self>) {
        self.permission_confirm = None;
        self.permission_acknowledged = false;
        cx.notify();
    }

    /// The full-access risk gate, drawn over the settings dialog.
    ///
    /// Measured (`RiskConfirmation`): the confirmation is `min(440px, 100%)`, its
    /// warning block is a 10px-gap row whose 14px error-coloured mark sits 2px
    /// down against a 14/22 secondary sentence, the acknowledgement is another
    /// 10px-gap row 20px under it - a 16x16 box on a 14/22 primary label - and
    /// the two actions are at least 72 and 136 wide. The enable action is drawn
    /// disabled until the box is ticked, which is the reference's own
    /// `disabled={!acknowledged}`, and the gate is the only path that writes
    /// full access: the pane's row and the composer chip both route through
    /// `request_permission`.
    pub(crate) fn render_permission_confirm(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let acknowledged = self.permission_acknowledged;
        let acknowledgement = div()
            .id("permission-acknowledge")
            .debug_selector(|| String::from("permission-acknowledge"))
            .mt(px(20.0))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(10.0))
            .cursor_pointer()
            .child(
                div()
                    .flex_none()
                    .w(px(16.0))
                    .h(px(16.0))
                    .mt(px(3.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if acknowledged {
                        theme::accent()
                    } else {
                        theme::border_strong()
                    }))
                    .bg(rgb(if acknowledged {
                        theme::accent()
                    } else {
                        theme::page()
                    }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(acknowledged, |box_| {
                        box_.child(icon(IconName::Check, 11.0, theme::page()))
                    }),
            )
            .child(
                txt(
                    tr(lang, "permission.confirmAcknowledge"),
                    14.0,
                    theme::text(),
                )
                .flex_1()
                .min_w_0(),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.permission_toggle_acknowledged(cx);
            }));
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgba(0x0F1115CC))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .debug_selector(|| String::from("permission-confirm-dialog"))
                    .w(px(440.0))
                    .p(px(18.0))
                    .rounded(px(12.0))
                    .bg(rgb(theme::page()))
                    .border_1()
                    .border_color(rgb(theme::border_strong()))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(txt(
                        tr(lang, "permission.confirmTitle"),
                        15.0,
                        theme::text(),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(10.0))
                            .child(div().flex_none().mt(px(2.0)).child(icon(
                                IconName::TriangleAlert,
                                14.0,
                                theme::error(),
                            )))
                            .child(
                                txt(
                                    tr(lang, "permission.confirmDescription"),
                                    14.0,
                                    theme::muted(),
                                )
                                .flex_1()
                                .min_w_0(),
                            ),
                    )
                    .child(acknowledgement)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(self.gate_button(
                                GateButton {
                                    id: "permission-confirm-cancel",
                                    label: tr(lang, "permission.confirmCancel").into(),
                                    primary: false,
                                    min_w: 72.0,
                                    enabled: true,
                                    action: |shell, cx| shell.permission_cancel_confirm(cx),
                                },
                                cx,
                            ))
                            .child(self.gate_button(
                                GateButton {
                                    id: "permission-confirm-enable",
                                    label: tr(lang, "permission.confirmEnable").into(),
                                    primary: true,
                                    min_w: 136.0,
                                    enabled: acknowledged,
                                    action: |shell, cx| shell.permission_confirm_enable(cx),
                                },
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    /// One button of the risk gate, at its measured minimum width and disabled
    /// until the reference's own acknowledgement has been ticked.
    ///
    /// The reference draws its two gate buttons from one component with two
    /// measured minimum widths (72 for cancel, 136 for the confirm), so the
    /// width travels with the label rather than as its own argument.
    fn gate_button(&self, gate: GateButton, cx: &mut Context<Self>) -> AnyElement {
        let GateButton {
            id,
            label,
            primary,
            min_w,
            enabled,
            action,
        } = gate;
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .when(!enabled, |button| button.opacity(0.5).cursor_default())
            .when(enabled, |button| button.cursor_pointer())
            .min_w(px(min_w))
            .px(px(16.0))
            .h(px(30.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(rgb(if primary {
                theme::accent()
            } else {
                theme::border()
            }))
            .bg(rgb(if primary {
                theme::accent()
            } else {
                theme::page()
            }))
            .when(enabled, |button| {
                button.hover(|s| {
                    s.bg(rgb(if primary {
                        theme::accent()
                    } else {
                        theme::hover()
                    }))
                })
            })
            .child(txt(
                label,
                13.0,
                if primary {
                    theme::page()
                } else {
                    theme::text()
                },
            ))
            .on_click(cx.listener(move |this, _event, _window, cx| action(this, cx)))
            .into_any_element()
    }

    /// The delete confirmation, drawn over the settings dialog.
    pub(crate) fn render_task_delete(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgba(0x0F1115CC))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .debug_selector(|| String::from("task-delete-dialog"))
                    .w(px(460.0))
                    .p(px(18.0))
                    .rounded(px(12.0))
                    .bg(rgb(theme::page()))
                    .border_1()
                    .border_color(rgb(theme::border_strong()))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(txt(tr(lang, "task.deleteTitle"), 15.0, theme::text()))
                    .child(txt(
                        tr(lang, "task.deleteDescription"),
                        12.5,
                        theme::muted(),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(self.delete_button(
                                "task-delete-cancel",
                                tr(lang, "task.deleteCancel"),
                                false,
                                cx,
                            ))
                            .child(self.delete_button(
                                "task-delete-confirm",
                                tr(lang, "task.deleteConfirm"),
                                true,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    /// The provider delete confirmation, drawn as a dialog over the settings
    /// panel the way the reference draws its own delete modal. The card it
    /// guards stays where it is; only the dialog is new.
    pub(crate) fn render_provider_delete(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let name = self.state.removing_provider.clone().unwrap_or_default();
        let has_credential = self
            .state
            .draft()
            .providers
            .iter()
            .any(|profile| profile.name == name && !profile.api_key.trim().is_empty());
        // The reference picks the description by whether a credential is
        // configured, and labels the confirm with the provider's own name.
        let title = crate::i18n::tr_with(lang, "settings.deleteTitle", &name);
        let description = if has_credential {
            crate::i18n::tr_with(lang, "settings.deleteDescriptionWithCredential", &name)
        } else {
            crate::i18n::tr_with(lang, "settings.deleteDescription", &name)
        };
        let confirm_label = crate::i18n::tr_with(lang, "settings.deleteConfirm", &name);
        let confirmed = name.clone();
        let confirm = div()
            .id("provider-delete-confirm")
            .debug_selector(|| String::from("provider-delete-confirm"))
            .px(px(16.0))
            .h(px(30.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(theme::accent()))
            .bg(rgb(theme::accent()))
            .hover(|s| s.opacity(0.9))
            .child(txt(confirm_label, 13.0, theme::page()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.state.remove_provider(&confirmed);
                cx.notify();
            }));
        let cancel = div()
            .id("provider-delete-cancel")
            .debug_selector(|| String::from("provider-delete-cancel"))
            .px(px(16.0))
            .h(px(30.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(theme::border()))
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(txt(tr(lang, "settings.cancel"), 13.0, theme::text()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.state.cancel_provider_remove();
                cx.notify();
            }));
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgba(0x0F1115CC))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .debug_selector(|| String::from("provider-delete-dialog"))
                    .w(px(460.0))
                    .p(px(18.0))
                    .rounded(px(12.0))
                    .bg(rgb(theme::page()))
                    .border_1()
                    .border_color(rgb(theme::border_strong()))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(txt(title, 15.0, theme::text()))
                    .child(txt(description, 12.5, theme::muted()))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(cancel)
                            .child(confirm),
                    ),
            )
            .into_any_element()
    }

    /// One button of the confirmation, carrying its own click.
    fn delete_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .px(px(16.0))
            .h(px(30.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(if primary {
                theme::accent()
            } else {
                theme::border()
            }))
            .bg(rgb(if primary {
                theme::accent()
            } else {
                theme::page()
            }))
            .hover(|s| {
                s.bg(rgb(if primary {
                    theme::accent()
                } else {
                    theme::hover()
                }))
            })
            .child(txt(
                label,
                13.0,
                if primary {
                    theme::page()
                } else {
                    theme::text()
                },
            ))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if primary {
                    this.task_confirm_delete(cx);
                } else {
                    this.task_cancel_delete(cx);
                }
            }))
            .into_any_element()
    }
    fn section_builtin_plugins(
        &self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let mut rows: Vec<AnyElement> = Vec::new();
        if self.plugins.is_empty() {
            rows.push(
                txt(tr(self.state.lang, "settings.empty"), 13.0, theme::muted()).into_any_element(),
            );
        }
        for (index, plugin) in self.plugins.iter().enumerate() {
            let name = plugin.name.clone();
            let status = self.plugin_status_key(&plugin.name);
            rows.push(
                div()
                    .debug_selector(move || format!("builtin-plugin-{index}"))
                    .id(SharedString::from(format!("builtin-plugin-{index}")))
                    .py(px(7.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(rgb(theme::border_soft()))
                    .child(txt(name, 13.0, theme::text()).flex_1())
                    .child(txt(tr(self.state.lang, status), 12.0, theme::muted()))
                    .into_any_element(),
            );
        }
        rows.push(
            txt(
                tr(self.state.lang, "settings.pluginViews"),
                13.0,
                theme::text(),
            )
            .pt(px(12.0))
            .pb(px(2.0))
            .into_any_element(),
        );
        for id in VIEWS {
            rows.push(self.builtin_view_row(id, cx));
        }
        ("settings.plugins", "settings.builtinPluginsHint", rows)
    }

    /// One row of the Built-in plugins pane's view list: a real control that
    /// moves the dialog to the pane it names, labelled with that pane's own
    /// title key, so the list cannot drift from the panes it opens.
    fn builtin_view_row(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let index = section_index(id).unwrap_or(0);
        let lang = self.state.lang;
        div()
            .id(SharedString::from(format!("builtin-view-{id}")))
            .debug_selector(move || format!("builtin-view-{id}"))
            .py(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::hover())))
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .child(
                txt(tr(lang, SECTIONS[index]), 13.0, theme::text())
                    .flex_1()
                    .min_w_0(),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.state.settings_section = index;
                cx.notify();
            }))
            .into_any_element()
    }

    /// The locale key for one plugin switch in the draft.
    ///
    /// The pane reads the draft rather than the installed inventory, so a
    /// plugin switched off elsewhere shows as off here with no state of its
    /// own to keep in sync.
    pub(crate) fn plugin_status_key(&self, name: &str) -> &'static str {
        if self
            .state
            .draft()
            .disabled_plugins
            .iter()
            .any(|plugin| plugin == name)
        {
            "settings.pluginDisabled"
        } else {
            "settings.pluginEnabled"
        }
    }

    /// MCP servers.
    fn section_mcp(&self) -> (&'static str, &'static str, Vec<AnyElement>) {
        let mut rows: Vec<AnyElement> = Vec::new();
        let servers = &self.state.draft().mcp_servers;
        if servers.is_empty() {
            rows.push(
                txt(
                    tr(self.state.lang, "settings.mcpEmpty"),
                    13.0,
                    theme::muted(),
                )
                .into_any_element(),
            );
        }
        for (index, server) in servers.iter().enumerate() {
            let name = server
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("unnamed")
                .to_string();
            rows.push(self.settings_row("settings.mcp", format!("{index}. {name}")));
        }
        rows.push(self.settings_note("settings.mcpPending", true));
        ("settings.mcp", "settings.mcpPending", rows)
    }

    /// Archived sessions, restored in place.
    fn section_archived(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let archived: Vec<(String, String, String)> = self
            .state
            .store
            .archived()
            .into_iter()
            .map(|session| {
                (
                    session.id.clone(),
                    session.title.clone(),
                    crate::state::age(session.updated_at),
                )
            })
            .collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        if archived.is_empty() {
            rows.push(
                txt(
                    tr(self.state.lang, "settings.archivedEmpty"),
                    13.0,
                    theme::muted(),
                )
                .into_any_element(),
            );
        }
        for (index, (id, title, when)) in archived.into_iter().enumerate() {
            let restore = id.clone();
            let control = div()
                .id(SharedString::from(format!("restore-row-{index}")))
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(theme::border()))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::hover())))
                .child(txt(
                    tr(self.state.lang, "settings.restore"),
                    theme::CHIP_SIZE,
                    theme::text(),
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.state.unarchive_session(&restore);
                    cx.notify();
                }));
            rows.push(
                self.settings_control_row(
                    "settings.archived",
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .child(txt(title, 13.0, theme::text()).flex_1())
                        .child(txt(when, theme::CHIP_SIZE, theme::faint()))
                        .child(control),
                ),
            );
        }
        ("settings.archived-sessions", "settings.archivedEmpty", rows)
    }

    /// Workspaces: which one roots this window's tools.
    fn section_workspaces(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let active = self.state.draft().active_workspace.clone();
        let workspaces: Vec<(String, String, String)> = self
            .state
            .draft()
            .workspaces
            .iter()
            .map(|workspace| {
                (
                    workspace.id.clone(),
                    workspace.name.clone(),
                    workspace.root.clone(),
                )
            })
            .collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, (id, name, root)) in workspaces.into_iter().enumerate() {
            let selected = id == active;
            let pick = id.clone();
            let control = div()
                .id(SharedString::from(format!("ws-pick-{index}")))
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(if selected {
                    theme::accent()
                } else {
                    theme::border()
                }))
                .bg(rgb(if selected {
                    theme::accent()
                } else {
                    theme::page()
                }))
                .cursor_pointer()
                .hover(|s| {
                    s.bg(rgb(if selected {
                        theme::accent()
                    } else {
                        theme::hover()
                    }))
                })
                .child(txt(
                    if selected {
                        tr(self.state.lang, "settings.active")
                    } else {
                        tr(self.state.lang, "settings.use")
                    },
                    theme::CHIP_SIZE,
                    if selected {
                        theme::page()
                    } else {
                        theme::text()
                    },
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.state.draft_mut().active_workspace = pick.clone();
                    cx.notify();
                }));
            rows.push(
                self.settings_control_row(
                    "settings.workspace",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .child(txt(name, 13.0, theme::text()).flex_1())
                                .child(control),
                        )
                        .child(txt(root, 12.0, theme::faint())),
                ),
            );
        }
        rows.push(self.settings_note("settings.workspacesHint", false));
        ("settings.workspaces", "settings.workspacesHint", rows)
    }

    /// Context: the send-side ceiling.
    fn section_context(
        &self,
        cx: &mut Context<Shell>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let rows = vec![
            self.input_row(
                "settings.budget",
                &self.settings_inputs.budget,
                Field::Budget,
                cx,
            ),
            self.input_row(
                "settings.maxOutputTokens",
                &self.settings_inputs.max_tokens,
                Field::MaxTokens,
                cx,
            ),
        ];
        ("settings.context", "settings.contextHint", rows)
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::parse_capacity;

    /// The reference documents a capacity as a number with an optional K or M
    /// suffix, so the field accepts both spellings of the same window.
    #[test]
    fn a_capacity_takes_a_k_or_m_suffix() {
        assert_eq!(parse_capacity("131072"), Some(131_072));
        assert_eq!(parse_capacity("128K"), Some(131_072));
        assert_eq!(parse_capacity("128k"), Some(131_072));
        assert_eq!(parse_capacity("1M"), Some(1_048_576));
        assert_eq!(parse_capacity(" 32 K "), Some(32_768));
    }

    /// Junk and empty input keep the setting rather than writing a zero window,
    /// which would make the context share meaningless.
    #[test]
    fn a_bad_capacity_is_refused() {
        assert_eq!(parse_capacity(""), None);
        assert_eq!(parse_capacity("many"), None);
        assert_eq!(parse_capacity("K"), None);
    }
}

/// The model and context labels are the strings measured off the reference own
/// models package, in both locales; the Chinese keeps the protocol names.
#[cfg(test)]
mod label_tests {
    fn tr(locale: &str, key: &'static str) -> String {
        crate::i18n::tr(crate::i18n::Lang::from_locale(locale), key)
    }

    #[test]
    fn the_measured_labels_are_the_ones_drawn() {
        assert_eq!(tr("en", "settings.apiMode"), "API protocol");
        assert_eq!(tr("en", "settings.apiChat"), "OpenAI Chat Completions");
        assert_eq!(tr("en", "settings.apiResponses"), "OpenAI Responses");
        assert_eq!(tr("en", "settings.apiAnthropic"), "Anthropic Messages");
        assert_eq!(tr("zh", "settings.apiAnthropic"), "Anthropic Messages");
        assert_eq!(tr("en", "settings.budget"), "Context window");
        assert_eq!(tr("zh", "settings.budget"), "上下文窗口");
        assert_eq!(tr("zh", "settings.apiAuto"), "提供商默认");
        assert_eq!(tr("zh", "settings.apiChat"), "OpenAI Chat Completions");
    }
}
