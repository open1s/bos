//! The settings dialog.
//!
//! Shaped after the DeepSeek Harness reference: a vertical rail of sections, a
//! scrolling pane whose header states what the section covers, and one
//! Cancel/Save pair at the foot. Everything edits a draft, so nothing reaches
//! the agent until Save; Cancel drops the draft.

use gpui_kit::component::input::{Input, InputContentType, InputState};
use gpui_kit::*;

use super::shell::Shell;
use super::{hairline_all, hairline_bottom, icon, txt, DivExt, IconName};
use crate::i18n::{tr, Lang};
use crate::settings::Settings;
use crate::theme;

/// The rail, in display order; the index is the section number narrowest the
/// state stores. Public within the crate so the command line can deep-link.
pub(crate) const SECTIONS: [&str; 15] = [
    "settings.account",
    "settings.general",
    "settings.models",
    "settings.tools",
    "settings.plugin-inventory",
    "settings.shell",
    "settings.agent-loop",
    "settings.mcp",
    "settings.archived-sessions",
    "settings.presentation",
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
pub(crate) const VIEWS: [&str; 10] = [
    "plugin-inventory",
    "shell",
    "agent-loop",
    "tools",
    "mcp",
    "archived-sessions",
    "presentation",
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

/// Grant or withhold the shell and write tools.
fn permission_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    let write = key == "write";
    let draft = shell.state.draft_mut();
    draft.bash_enabled = write;
    draft.file_tools_enabled = write;
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
fn memory_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    shell.state.draft_mut().memory_enabled = key == "on";
    cx.notify();
}

/// Reveal the settings file in the platform's file browser.
fn open_config_action(shell: &mut Shell, cx: &mut Context<Shell>) {
    let lang = shell.state.lang;
    match crate::settings::reveal_config() {
        Ok(()) => shell
            .state
            .notice(tr(lang, "settings.openedConfig").to_string()),
        Err(error) => shell
            .state
            .notice(format!("{} {error}", tr(lang, "settings.revealFailed"))),
    }
    cx.notify();
}

/// The editable fields whose shipped default the dialog can restore.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The model id.
    Model,
    /// The API base URL.
    BaseUrl,
    /// The API key.
    ApiKey,
    /// The system prompt.
    SystemPrompt,
    /// The shell command timeout.
    ShellTimeout,
    /// The per-stream output cap.
    OutputCap,
    /// The ReAct step ceiling.
    MaxSteps,
    /// The name a new provider is saved under.
    ProviderName,
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
            Field::Model => default.model,
            Field::BaseUrl => default.base_url,
            Field::ApiKey => default.api_key,
            Field::SystemPrompt => default.system_prompt,
            Field::ShellTimeout => default.shell_timeout_secs.to_string(),
            Field::OutputCap => default.tool_output_cap.to_string(),
            Field::MaxSteps => default.max_steps.to_string(),
            Field::ProviderName => String::new(),
            Field::Budget => default.context_budget.to_string(),
            Field::MaxTokens => default
                .max_tokens
                .map(|tokens| tokens.to_string())
                .unwrap_or_default(),
        }
    }
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
            0 => self.section_account(cx),
            1 => self.section_general(cx),
            2 => self.section_models(cx),
            3 => self.section_tools(cx),
            4 => self.section_plugins(cx),
            5 => self.section_shell(cx),
            6 => self.section_agent_loop(cx),
            7 => self.section_mcp(),
            8 => self.section_archived(cx),
            9 => self.section_presentation(cx),
            10 => self.section_workspaces(cx),
            11 => self.section_context(cx),
            12 => self.section_agent_presets(cx),
            13 => self.section_builtin_plugins(cx),
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
                .child(
                    txt(tr(self.state.lang, label), 13.0, theme::text())
                        .font_weight(FontWeight::MEDIUM)
                        .flex_1()
                        .min_w_0(),
                )
                .child(control),
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

    /// A segmented choice.
    fn choice_row(
        &self,
        id: &'static str,
        label: &'static str,
        current: &str,
        options: &[(&'static str, &str)],
        action: fn(&mut Shell, &str, &mut Context<Shell>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut group = div().flex().flex_row().gap(px(6.0));
        for (key, text) in options.iter().copied() {
            let selected = key == current;
            group = group.child(
                div()
                    .id(SharedString::from(format!("{id}-{key}")))
                    .debug_selector(move || format!("{id}-{key}"))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    }))
                    .bg(if selected {
                        rgb(theme::accent())
                    } else {
                        rgb(theme::page())
                    })
                    .cursor_pointer()
                    .hover(|s| {
                        s.bg(rgb(if selected {
                            theme::accent()
                        } else {
                            theme::hover()
                        }))
                    })
                    .child(txt(
                        text,
                        theme::CHIP_SIZE,
                        if selected {
                            theme::page()
                        } else {
                            theme::muted()
                        },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| action(this, key, cx))),
            );
        }
        self.settings_control_row(label, group)
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

    /// An editable field whose contents are masked on screen.
    fn secret_row(
        &self,
        label: &'static str,
        input: &Entity<InputState>,
        field: Field,
        cx: &mut Context<Shell>,
    ) -> AnyElement {
        self.field_row(label, input, true, field, cx)
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

    /// A button under the rows, for actions that are not themselves settings.
    fn action_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: fn(&mut Shell, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .mt(px(12.0))
            .child(
                div()
                    .id(id)
                    .px(px(12.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(txt(
                        tr(self.state.lang, label),
                        theme::CHIP_SIZE,
                        theme::text(),
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| action(this, cx))),
            )
            .into_any_element()
    }

    /// Account: identity, paths and the way to the configuration file.
    fn section_account(
        &self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
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
            self.action_button("open-config", "settings.openConfig", open_config_action, cx),
        ];
        ("settings.account", "settings.accountHint", rows)
    }

    /// General: language, permission and memory.
    fn section_general(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let lang = self.state.lang;
        let write = self.state.draft().bash_enabled;
        let memory = self.state.draft().memory_enabled;
        let on = if memory { "on" } else { "off" };
        let skills_dir = self.state.draft().skills_dir.clone();
        let mut rows = vec![
            self.appearance_row(cx),
            self.font_size_row(cx),
            self.choice_row(
                "set-perm",
                "settings.permission",
                if write { "write" } else { "read" },
                &[
                    ("write", &tr(lang, "settings.permWrite")),
                    ("read", &tr(lang, "settings.permRead")),
                ],
                permission_action,
                cx,
            ),
            self.choice_row(
                "set-memory",
                "settings.memoryToggle",
                on,
                &[
                    ("on", &tr(lang, "settings.on")),
                    ("off", &tr(lang, "settings.off")),
                ],
                memory_action,
                cx,
            ),
            self.settings_row("settings.skillsDir", skills_dir),
            self.settings_row(
                "settings.approvalOn",
                if self.state.draft().require_approval {
                    tr(lang, "settings.on").to_string()
                } else {
                    tr(lang, "settings.off").to_string()
                },
            ),
            self.settings_note("settings.pendingGate", true),
            self.settings_note("settings.skillsNote", false),
        ];
        rows.push(
            div()
                .debug_selector(|| "general-item-current-version".to_string())
                .flex()
                .flex_col()
                .gap(px(12.0))
                .pt(px(12.0))
                .child(txt(tr(lang, "settings.about"), 13.0, theme::text()))
                .children(self.about_rows())
                .child(self.settings_note("settings.aboutHint", false))
                .into_any_element(),
        );
        ("settings.general", "settings.generalHint", rows)
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

    fn section_models(
        &self,
        cx: &mut Context<Shell>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let inputs = &self.settings_inputs;
        let draft = self.state.draft();
        let active = draft.active_provider.clone();
        // Discovered profiles first, then saved ones; only the saved ones are
        // bsh own rows to remove, because the rest are rewritten from the BOS
        // config on every load.
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
        let mut rows: Vec<AnyElement> = vec![
            self.input_row("settings.modelId", &inputs.model, Field::Model, cx),
            self.input_row("settings.baseUrl", &inputs.base_url, Field::BaseUrl, cx),
            self.secret_row("settings.apiKey", &inputs.api_key, Field::ApiKey, cx),
            self.input_row(
                "settings.systemPrompt",
                &inputs.system_prompt,
                Field::SystemPrompt,
                cx,
            ),
            self.choice_row(
                "set-api-mode",
                "settings.apiMode",
                if draft.api_mode.is_empty() {
                    "auto"
                } else {
                    draft.api_mode.as_str()
                },
                &[
                    (
                        "auto",
                        &crate::i18n::tr(self.state.lang, "settings.apiAuto"),
                    ),
                    (
                        "chat",
                        &crate::i18n::tr(self.state.lang, "settings.apiChat"),
                    ),
                    (
                        "responses",
                        &crate::i18n::tr(self.state.lang, "settings.apiResponses"),
                    ),
                    (
                        "anthropic",
                        &crate::i18n::tr(self.state.lang, "settings.apiAnthropic"),
                    ),
                ],
                api_mode_action,
                cx,
            ),
        ];
        for (index, (profile, removable)) in listed.into_iter().enumerate() {
            let selected = profile.name == active;
            let name = profile.name.clone();
            let endpoint = format!("{}  {}", profile.model, profile.base_url);
            let mut row = div()
                .py(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .border_b_1()
                .border_color(rgb(theme::border_soft()));
            row = row.child(
                div()
                    .debug_selector(move || format!("provider-{index}"))
                    .id(SharedString::from(format!("provider-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .flex_1()
                    .px(px(6.0))
                    .py(px(4.0))
                    .rounded(px(5.0))
                    .when(selected, |element| element.bg(rgb(theme::selected_row())))
                    .child(txt(profile.name.clone(), 13.0, theme::text()))
                    .child(txt(endpoint, 12.0, theme::muted()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.select_provider(&name);
                        cx.notify();
                    })),
            );
            let edit_profile = profile.clone();
            let edit_label = tr(self.state.lang, "settings.editProvider");
            row = row.child(
                div()
                    .debug_selector(move || format!("provider-edit-{index}"))
                    .id(SharedString::from(format!("provider-edit-{index}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(20.0))
                    .px(px(4.0))
                    .rounded(px(4.0))
                    .child(txt(edit_label, 12.0, theme::muted()))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        load_provider_into_form(this, &edit_profile, window, cx);
                        cx.notify();
                    })),
            );
            if removable {
                let name = profile.name.clone();
                row = row.child(
                    div()
                        .debug_selector(move || format!("provider-remove-{index}"))
                        .id(SharedString::from(format!("provider-remove-{index}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(20.0))
                        .h(px(20.0))
                        .rounded(px(4.0))
                        .child(icon(IconName::X, 12.0, theme::muted()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.state.begin_provider_remove(&name);
                            cx.notify();
                        })),
                );
            }
            if self.state.removing_provider.as_deref() == Some(profile.name.as_str()) {
                let lang = self.state.lang;
                let has_key = !profile.api_key.is_empty();
                let title = crate::i18n::tr_with(lang, "settings.deleteTitle", &profile.name);
                let description = if has_key {
                    crate::i18n::tr_with(
                        lang,
                        "settings.deleteDescriptionWithCredential",
                        &profile.name,
                    )
                } else {
                    crate::i18n::tr_with(lang, "settings.deleteDescription", &profile.name)
                };
                let confirm_label =
                    crate::i18n::tr_with(lang, "settings.deleteConfirm", &profile.name);
                let cancel_label = crate::i18n::tr(lang, "settings.cancel");
                let confirmed = profile.name.clone();
                let confirm_button = div()
                    .debug_selector(move || format!("provider-delete-confirm-{index}"))
                    .id(SharedString::from(format!(
                        "provider-delete-confirm-{index}"
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(20.0))
                    .px(px(6.0))
                    .rounded(px(4.0))
                    .child(txt(confirm_label, 12.0, theme::error()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.remove_provider(&confirmed);
                        cx.notify();
                    }));
                let cancel_button = div()
                    .debug_selector(move || format!("provider-delete-cancel-{index}"))
                    .id(SharedString::from(format!(
                        "provider-delete-cancel-{index}"
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(20.0))
                    .px(px(6.0))
                    .rounded(px(4.0))
                    .child(txt(cancel_label, 12.0, theme::muted()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.cancel_provider_remove();
                        cx.notify();
                    }));
                rows.push(
                    div()
                        .debug_selector(move || format!("provider-delete-{index}"))
                        .id(SharedString::from(format!("provider-delete-{index}")))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::hover()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(txt(title, 12.0, theme::text()))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(confirm_button)
                                        .child(cancel_button),
                                ),
                        )
                        .child(txt(description, 11.0, theme::muted()))
                        .into_any_element(),
                );
            } else {
                rows.push(row.into_any_element());
            }
        }
        rows.push(self.input_row(
            "settings.providerDisplayName",
            &inputs.provider_name,
            Field::ProviderName,
            cx,
        ));
        rows.push(
            div()
                .debug_selector(|| "provider-add".to_string())
                .id(SharedString::from("provider-add"))
                .py(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(icon(IconName::Plus, 12.0, theme::accent()))
                .child(txt(
                    tr(
                        self.state.lang,
                        if self.state.editing_provider.is_some() {
                            "settings.saveProvider"
                        } else {
                            "settings.addModelProvider"
                        },
                    ),
                    13.0,
                    theme::accent(),
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    let typed = this
                        .settings_inputs
                        .provider_name
                        .read(cx)
                        .value()
                        .trim()
                        .to_string();
                    let outcome = match this.state.editing_provider.clone() {
                        Some(original) => this.state.update_provider(&original, &typed),
                        None => this.state.add_provider(&typed),
                    };
                    this.state.provider_refusal = outcome.err().map(|refusal| refusal.key());
                    if this.state.provider_refusal.is_none() {
                        this.state.editing_provider = None;
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        if self.state.editing_provider.is_some() {
            rows.push(
                div()
                    .debug_selector(|| "provider-cancel-edit".to_string())
                    .id(SharedString::from("provider-cancel-edit"))
                    .py(px(4.0))
                    .child(txt(
                        tr(self.state.lang, "settings.cancelEdit"),
                        12.0,
                        theme::muted(),
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.editing_provider = None;
                        this.state.provider_refusal = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if let Some(key) = self.state.provider_refusal {
            rows.push(self.settings_note(key, true));
        }
        rows.extend(self.catalog_rows(cx));
        rows.extend(self.models_picker_rows(cx));
        ("settings.models", "settings.modelsHint", rows)
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

    /// About: build facts and discovered endpoints.
    /// How the interface looks and speaks.
    fn section_presentation(
        &self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let rows = vec![self.choice_row(
            "set-lang",
            "settings.language",
            &self.state.settings.lang.clone(),
            &[("en", "English"), ("zh", "中文")],
            language_action,
            cx,
        )];
        ("settings.presentation", "settings.presentationHint", rows)
    }

    /// The About block: the reference draws it as General's own `current-version`
    /// row group (`settings.general.item`, order 100), not as a rail cell.
    fn about_rows(&self) -> Vec<AnyElement> {
        let draft = self.state.draft();
        let mut rows = vec![
            self.settings_row("settings.version", env!("CARGO_PKG_VERSION").to_string()),
            self.settings_row(
                "settings.dataDir",
                crate::settings::dir().display().to_string(),
            ),
            self.settings_row(
                "settings.sessions",
                self.state.store.sessions.len().to_string(),
            ),
        ];
        if draft.profiles.is_empty() {
            rows.push(self.settings_row("settings.profile", "-".to_string()));
        }
        for profile in &draft.profiles {
            rows.push(self.settings_row(
                "settings.profile",
                format!("{}  -  {}", profile.name, profile.model),
            ));
        }
        rows
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
