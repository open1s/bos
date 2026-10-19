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

use super::shell::{ModelRowEditor, ModelsAddRoute, Shell};
use super::{hairline_all, hairline_bottom, hairline_top, icon, txt, DivExt, IconName};
use crate::i18n::{tr, tr_args, Lang};
use crate::settings::mcp::{McpScope, McpServer, McpTransport};
use crate::settings::Settings;
use crate::theme;

/// The rail, in display order; the index is the section number narrowest the
/// state stores. Public within the crate so the command line can deep-link.
pub(crate) const SECTIONS: [&str; 15] = [
    "settings.account",
    "settings.vault",
    "settings.general",
    "settings.models",
    "settings.tools",
    "settings.plugin-inventory",
    "settings.shell",
    "settings.agent-loop",
    "settings.mcp",
    "settings.archived-sessions",
    "settings.global.workspaces",
    "settings.context",
    "settings.agent-presets",
    "settings.plugins",
    "settings.automation",
];

/// The rail's cells: the reference's five, plus the Vault.
///
/// DSH registers five `settings.section`s and sorts them by `order`: account
/// (-10), general (0), models (10), plugins (15, "Extensions") and
/// agent-presets (20). The Vault is a BSH addition and sits after Account, the
/// surface it is administered from. Everything else this crate's `SECTIONS`
/// carries is a card, a tab or a sidebar panel in the reference, so it is not a
/// rail cell here either. See `docs/ui-mirror/reference/dsh-settings-rail.md`.
pub(crate) const NAV: [&str; 6] = [
    "account",
    "vault",
    "general",
    "models",
    "plugins",
    "agent-presets",
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
    SECTIONS.iter().position(|key| {
        // A pane the rail does not carry is reached by its short id, so the
        // global-level prefix is part of the id and not of the name: the
        // workspaces pane is `settings.global.workspaces` in the file and
        // `workspaces` to the cell that opens it.
        let trimmed = key
            .strip_prefix("settings.global.")
            .or_else(|| key.strip_prefix("settings."))
            .unwrap_or(key);
        trimmed == id
    })
}

/// The text fields the panes edit.
///
/// The connection fields are not here any more: an endpoint belongs to a
/// provider node in the tree, and the provider form carries its own fields on
/// `Shell`, because one provider form is open at a time.
pub(crate) struct SettingsInputs {
    system_prompt: Entity<InputState>,
    budget: Entity<InputState>,
    shell_timeout: Entity<InputState>,
    output_cap: Entity<InputState>,
    pub(crate) max_steps: Entity<InputState>,
    max_tokens: Entity<InputState>,
}

impl SettingsInputs {
    /// Build the fields.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            system_prompt: cx.new(|cx| InputState::new(window, cx)),
            budget: cx.new(|cx| InputState::new(window, cx)),
            shell_timeout: cx.new(|cx| InputState::new(window, cx)),
            output_cap: cx.new(|cx| InputState::new(window, cx)),
            max_steps: cx.new(|cx| InputState::new(window, cx)),
            max_tokens: cx.new(|cx| InputState::new(window, cx)),
        }
    }

    /// Load the fields from the settings the dialog opened on.
    pub(crate) fn sync(&self, settings: &Settings, window: &mut Window, cx: &mut Context<Shell>) {
        let effective = settings.effective();
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
        settings.workspace.agent.system_prompt = self.system_prompt.read(cx).value().to_string();
        let budget = self.budget.read(cx).value().trim().to_string();
        if let Some(parsed) = parse_capacity(&budget) {
            settings.workspace.agent.context_budget = parsed;
        }
        let timeout = self.shell_timeout.read(cx).value().trim().to_string();
        if let Ok(parsed) = timeout.parse::<u64>() {
            settings.workspace.agent.shell_timeout_secs = parsed.max(1);
        }
        let cap = self.output_cap.read(cx).value().trim().to_string();
        if let Ok(parsed) = cap.parse::<usize>() {
            settings.workspace.agent.tool_output_cap = parsed.max(256);
        }
        let steps = self.max_steps.read(cx).value().trim().to_string();
        if let Ok(parsed) = steps.parse::<usize>() {
            settings.workspace.agent.max_steps = parsed.max(1);
        }
        let tokens = self.max_tokens.read(cx).value().trim().to_string();
        if tokens.is_empty() {
            settings.workspace.agent.max_tokens = None;
        } else if let Some(parsed) = parse_capacity(&tokens) {
            settings.workspace.agent.max_tokens = Some(parsed.min(u32::MAX as usize) as u32);
        }
    }
}

/// The Vault pane's text fields.
///
/// The passphrase and the entry value are masked; the confirm field exists so a
/// create is typed twice, because a vault sealed under a typo has no recovery.
pub(crate) struct VaultInputs {
    /// The admin passphrase, masked.
    pub(crate) passphrase: Entity<InputState>,
    /// The second copy a create asks for, masked.
    pub(crate) confirm: Entity<InputState>,
    /// The name of the entry being added.
    pub(crate) name: Entity<InputState>,
    /// The value of the entry being added, masked.
    pub(crate) value: Entity<InputState>,
}

impl VaultInputs {
    /// Build the fields, with the pane's own placeholders.
    pub(crate) fn new(window: &mut Window, lang: Lang, cx: &mut Context<Shell>) -> Self {
        let passphrase = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(tr(lang, "settings.vaultPassphrasePlaceholder"))
        });
        let confirm = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(tr(lang, "settings.vaultConfirmPlaceholder"))
        });
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr(lang, "settings.vaultNamePlaceholder"))
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(tr(lang, "settings.vaultValuePlaceholder"))
        });
        Self {
            passphrase,
            confirm,
            name,
            value,
        }
    }
}

/// The name a provider is shown under: the name the tree carries, else the
/// shipped family's own name, else its id.
///
/// Never the base URL. `built_in` answers `(name, base URL, protocol)`, and two
/// sites here took its SECOND element as the label, so the Models card printed
/// `https://integrate.api.nvidia.com/v1` where the provider's name belongs and
/// the display-name field opened pre-filled with the endpoint.
pub(crate) fn provider_label(id: &str, name: Option<&str>) -> String {
    if let Some(named) = name.map(str::trim).filter(|named| !named.is_empty()) {
        return named.to_string();
    }
    crate::settings::global::built_in(id)
        .map(|(label, _, _)| label.to_string())
        .unwrap_or_else(|| id.to_string())
}

/// The label of the provider's API-key source control.
///
/// A chosen entry names itself, so the row states where the credential comes
/// from; with no choice the control is the affordance that opens the vault.
pub(crate) fn vault_source_label(lang: Lang, entry: Option<&str>) -> String {
    match entry.map(str::trim).filter(|entry| !entry.is_empty()) {
        Some(entry) => tr_args(lang, "settings.vaultSource", &[("name", entry.to_string())]),
        None => tr(lang, "settings.vaultChoose"),
    }
}

/// Load a provider into the open editor: the tree carries the values, and the
/// text engines are what the reader sees.
///
/// The provider id is fixed, because it is the key the tree is filed under and
/// the prefix the engine registers the vendor with; the display name is the
/// part a reader may edit.
pub(crate) fn load_provider_into_form(
    shell: &mut Shell,
    id: &str,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    // The draft is the document the panes edit, so a provider seeded into
    // it is the one the form opens, and the file it is saved to never has to
    // be read back for a value the form already holds.
    let Some(provider) = shell.state.draft().global.providers.get(id).cloned() else {
        return;
    };
    let label = provider_label(id, provider.name.as_deref());
    set_value(&shell.add_route, id.to_string(), window, cx);
    set_value(&shell.add_display, label, window, cx);
    set_value(&shell.add_base_url, provider.base_url.clone(), window, cx);
    // One editor per leaf, so the reference's per-row inputs are per-row
    // engines rather than one shared field that collapses every row into one.
    shell.model_rows = provider
        .models
        .iter()
        .map(|(key, entry)| ModelRowEditor {
            key: key.clone(),
            entry: entry.clone(),
            id: cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                state.set_value(entry.model.clone(), window, cx);
                state
            }),
            name: cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                state.set_value(entry.name.clone().unwrap_or_default(), window, cx);
                state
            }),
            open: false,
        })
        .collect();
    set_value(&shell.new_row_id, String::new(), window, cx);
    set_value(&shell.new_row_name, String::new(), window, cx);
    set_value(&shell.new_model_id, String::new(), window, cx);
    set_value(&shell.new_model_name, String::new(), window, cx);
    shell.catalogue_choice = None;
    shell.catalogue_open = false;
    shell.state.begin_provider_edit(id);
}

/// Replace one text engine's value, without firing a change event.
fn set_value(
    input: &Entity<InputState>,
    value: String,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    input.update(cx, |state, cx| state.set_value(value, window, cx));
}
/// The alias the pane uses for a text engine replacement.
fn set(input: &Entity<InputState>, value: String, window: &mut Window, cx: &mut Context<Shell>) {
    set_value(input, value, window, cx);
}

/// Switch the interface language.
fn language_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    let lang = if key == "zh" { Lang::Zh } else { Lang::En };
    // The preview follows the click; the file follows Save. Writing the
    // committed tree and saving it here would persist every OTHER field from
    // the copy the reader has not applied yet, which is how a pending
    // permission change got overwritten by a language pick.
    shell.state.lang = lang;
    shell.state.draft_mut().global.general.lang = lang.locale().to_string();
    cx.notify();
}

/// Switch the colour mode and remember it.
fn theme_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    shell.state.draft_mut().global.general.theme_mode = key.to_string();
    crate::theme::apply_here(key, cx);
    cx.notify();
}

/// Step the conversation font size and remember it.
///
/// The reference stepper accepts 10..22 px and moves the conversation text by
/// the step, so the shell re-renders instead of only storing the number.
fn font_size_action(shell: &mut Shell, delta: i32, cx: &mut Context<Shell>) {
    let current = shell.state.draft().global.general.content_font_size as i32;
    let next = (current + delta).clamp(
        crate::theme::CONTENT_MIN as i32,
        crate::theme::CONTENT_MAX as i32,
    ) as u32;
    if next == shell.state.draft().global.general.content_font_size {
        return;
    }
    shell.state.draft_mut().global.general.content_font_size = next;
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
    // Draft only. Saving here persisted the committed tree, so the toggle the
    // reader had just flipped was the one field the write did NOT carry - the
    // file kept its old value while the pane showed the new one.
    shell.state.draft_mut().global.general.developer_tools = on;
    shell.developer_tools_failed = false;
    cx.notify();
}

/// Offer or withdraw the hosted web-search tool.
///
/// Nothing local executes it - the provider runs the search - so the switch
/// writes the setting and the engine decides whether the protocol can carry it.
fn web_tools_action(shell: &mut Shell, on: bool, cx: &mut Context<Shell>) {
    shell.state.draft_mut().workspace.agent.web_tools_enabled = on;
    cx.notify();
}

/// Attach or detach the memory store.
fn memory_action(shell: &mut Shell, on: bool, cx: &mut Context<Shell>) {
    shell.state.draft_mut().workspace.agent.memory_enabled = on;
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
    preference_action(
        shell,
        |settings| &mut settings.global.general.transcript_view,
        key,
        cx,
    );
}

/// Where a link in the transcript opens.
fn link_opening_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    preference_action(
        shell,
        |settings| &mut settings.global.general.link_opening,
        key,
        cx,
    );
}

/// What Enter does while a turn streams.
fn composer_enter_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    preference_action(
        shell,
        |settings| &mut settings.global.general.composer_enter,
        key,
        cx,
    );
}

/// How much performance and usage detail the footer shows.
fn performance_usage_action(shell: &mut Shell, key: &str, cx: &mut Context<Shell>) {
    preference_action(
        shell,
        |settings| &mut settings.global.general.performance_usage,
        key,
        cx,
    );
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
            Field::ShellTimeout => default.workspace.agent.shell_timeout_secs.to_string(),
            Field::OutputCap => default.workspace.agent.tool_output_cap.to_string(),
            Field::MaxSteps => default.workspace.agent.max_steps.to_string(),
            Field::Budget => default.workspace.agent.context_budget.to_string(),
            Field::MaxTokens => default
                .workspace
                .agent
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

/// Which of the Skills pane's two disclosure controls a press flips.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SkillsControl {
    /// The browse-all list over every discovered skill.
    Browse,
    /// The executor-dirs list over the sources themselves.
    Dirs,
}

/// The tint a source card's initial is drawn in, derived from the source name.
///
/// Six executor cards have to be distinguishable at a glance, and a colour
/// chosen per name - rather than a seventh table mapping names to colours - is
/// the one form that cannot fall out of step when a custom directory is added:
/// any name at all gets a stable tint.
fn source_tint(name: &str) -> u32 {
    // A small, fixed palette rather than a hue wheel: the cards sit on a white
    // pane and a saturated random hue would fight the accent the pane already
    // uses for "on".
    const TINTS: [u32; 6] = [
        0x4176E6, // the accent blue
        0x2E9E6B, // green
        0xB8791E, // amber
        0x8B5CF6, // violet
        0xD9534F, // red
        0x2F7D8C, // teal
    ];
    let sum: usize = name.bytes().map(|byte| byte as usize).sum();
    TINTS[sum % TINTS.len()]
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
            // The dialog previews the WORKSPACE configuration it is editing, so
            // it resolves without a session choice: the pane's job is to show
            // what the draft would build for a session that inherits.
            let built = crate::caps::build_agent(
                self.state.draft(),
                &self.state.vault,
                &crate::settings::resolve::SessionChoice::default(),
                &self.state.mcp,
            );
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
            1 => self.section_vault(cx),
            2 => self.section_general(cx),
            3 => self.section_models(cx),
            4 => self.section_tools(cx),
            5 => self.section_plugins(cx),
            6 => self.section_shell(cx),
            7 => self.section_agent_loop(cx),
            8 => self.section_mcp(cx),
            9 => self.section_archived(cx),
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
                    .children(content),
            )
            // The save bar is pinned under the scrolling box, not carried in it.
            // Carried in it, the dialog's ONE primary action scrolled away with
            // the section: in General at the reference window size it laid out at
            // y=1214 in a 900-tall window - off screen - so the reader had to
            // scroll to find the only control that commits, and nothing reachable
            // by keyboard could replace it.
            .child(div().flex_none().px(px(24.0)).pb(px(24.0)).child(footer));

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
        let size = self.state.draft().global.general.content_font_size;
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
        let size = self.state.draft().global.general.content_font_size as i32;
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
        let current = self.state.draft().global.general.theme_mode.clone();
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
        let permission = draft.workspace.agent.permission.clone();
        let language = if self.state.lang == Lang::Zh {
            "zh"
        } else {
            "en"
        };
        let transcript = draft.global.general.transcript_view.clone();
        let links = draft.global.general.link_opening.clone();
        let enter = draft.global.general.composer_enter.clone();
        let performance = draft.global.general.performance_usage.clone();
        let developer_tools = draft.global.general.developer_tools;
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
        let active = draft.global.default_preset.clone();
        let listed: Vec<String> = draft.global.preset_ids();
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
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let lang = self.state.lang;
        let cards: Vec<AnyElement> = self
            .state
            .draft()
            .global
            .providers
            .iter()
            .enumerate()
            .map(|(index, (id, provider))| self.provider_card(id, provider, index, cx))
            .collect();
        let listed = if cards.is_empty() {
            vec![self.model_empty_note(lang)]
        } else {
            cards
        };
        let mut rows: Vec<AnyElement> = Vec::with_capacity(listed.len() + 1);
        rows.extend(listed);
        rows.push(self.add_provider_block(cx));
        ("settings.models", "settings.modelsHint", rows)
    }

    /// The empty catalogue, drawn where the reference draws its `modelEmpty`.
    fn model_empty_note(&self, lang: Lang) -> AnyElement {
        hairline_all(
            div()
                .flex()
                .items_center()
                .justify_center()
                .py(px(12.0))
                .px(px(14.0))
                .rounded(px(16.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border_soft()))
                .child(txt(lang.tr("settings.modelsEmpty"), 12.0, theme::faint())),
        )
        .into_any_element()
    }

    /// One provider card: the measured row head, and the editor under it while
    /// this provider is the one being edited.
    fn provider_card(
        &self,
        id: &str,
        provider: &crate::settings::global::Provider,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let open = self.state.editing_provider.as_deref() == Some(id);
        let name = provider_label(id, provider.name.as_deref());
        let mut identity = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .child(txt(name.clone(), 14.0, theme::text()).font_weight(FontWeight::MEDIUM));
        if provider.declared {
            identity = identity.child(
                hairline_all(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .border_color(rgb(theme::border()))
                        .child(txt(lang.tr("settings.customTag"), 11.0, theme::muted())),
                )
                .into_any_element(),
            );
        }
        // The dot answers "is a credential configured", which now means a
        // vault source: a named entry, or the conventional entry a migration
        // filed. A locked vault is not a missing key, so a named choice counts
        // regardless of the lock state.
        let in_vault = self.state.vault.contains(&crate::vault::provider_key(id))
            || provider.api_key_vault.is_some();
        if in_vault {
            identity = identity.child(
                div()
                    .debug_selector(|| format!("provider-key-{index}"))
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(rgb(theme::success()))
                    .into_any_element(),
            );
        }
        let owned = id.to_string();
        let head = identity.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .ml_auto()
                .child(
                    div()
                        .id(format!("provider-edit-{index}"))
                        .debug_selector(move || format!("provider-edit-{index}"))
                        .cursor_pointer()
                        .child(txt(lang.tr("settings.editProvider"), 12.0, theme::muted()))
                        .on_click(cx.listener({
                            let owned = owned.clone();
                            move |this, _event, window, cx| {
                                load_provider_into_form(this, &owned, window, cx);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    div()
                        .id(format!("provider-remove-{index}"))
                        .debug_selector(move || format!("provider-remove-{index}"))
                        .cursor_pointer()
                        .child(txt(lang.tr("settings.modelsRemove"), 12.0, theme::muted()))
                        .on_click(cx.listener({
                            let owned = owned.clone();
                            move |this, _event, _window, cx| {
                                this.state.begin_provider_remove(&owned);
                                cx.notify();
                            }
                        })),
                ),
        );
        let mut card = hairline_all(
            div()
                .id(SharedString::from(format!("provider-{index}")))
                .debug_selector(move || format!("provider-{index}"))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .py(px(12.0))
                .px(px(14.0))
                .rounded(px(16.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border_soft()))
                .child(head),
        );
        if open {
            card = card.child(self.provider_editor(id, cx));
        }
        card.into_any_element()
    }
    /// The provider editor: the measured card head and the model list under
    /// it. One text engine per model row, so a refusal repaints its own row.
    fn provider_editor(&self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let mut body: Vec<AnyElement> = Vec::with_capacity(10);
        // The measured editor field order: the credential, then the fold that
        // holds the endpoint, the protocol and the display name.
        body.push(self.provider_api_key_field(id, cx));
        let open = self.models_advanced_open;
        let fold = div()
            .id("models-customized")
            .debug_selector(|| "models-customized".to_string())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .child(if open {
                icon(IconName::ChevronDown, 12.0, theme::muted())
            } else {
                icon(IconName::ChevronRight, 12.0, theme::muted())
            })
            .child(txt(lang.tr("settings.customized"), 12.0, theme::muted()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.models_advanced_open = !open;
                cx.notify();
            }))
            .into_any_element();
        body.push(fold);
        if open {
            body.push(self.editor_field(
                "settings.customBaseUrl",
                self.editor_input(&self.add_base_url, "field-editor-base-url".to_string()),
            ));
            body.push(self.editor_field(
                "settings.displayName",
                self.editor_input(&self.add_display, "field-editor-display".to_string()),
            ));
            body.push(self.editor_field("settings.apiMode", self.api_mode_choices(cx)));
            body.push(
                txt(lang.tr("settings.advancedHint"), 12.0, theme::faint()).into_any_element(),
            );
        }
        body.extend(self.model_list_rows(id, cx));
        // A refusal is drawn, not only computed: the backend already named what
        // is wrong, and a pane that hides it leaves the reader guessing at why
        // the apply did nothing.
        if let Some(key) = self.state.provider_refusal {
            body.push(
                div()
                    .debug_selector(|| "provider-refusal".to_string())
                    .child(txt(lang.tr(key), 12.0, theme::error()))
                    .into_any_element(),
            );
        }
        let owned = id.to_string();
        let apply = div()
            .id(SharedString::from("models-apply"))
            .debug_selector(|| "models-apply".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .bg(rgb(theme::text()))
            .cursor_pointer()
            .child(txt(lang.tr("settings.apply"), 14.0, theme::page()))
            .on_click(cx.listener({
                let owned = owned.clone();
                move |this, _event, window, cx| {
                    this.apply_provider_edit(&owned, window, cx);
                }
            }))
            .into_any_element();
        let cancel = div()
            .id(SharedString::from("models-editor-cancel"))
            .debug_selector(|| "models-editor-cancel".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme::border()))
            .cursor_pointer()
            .child(txt(lang.tr("settings.cancel"), 14.0, theme::text()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.state.editing_provider = None;
                this.model_rows.clear();
                this.models_advanced_open = false;
                cx.notify();
            }))
            .into_any_element();
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(apply)
            .child(cancel);
        div()
            .id(SharedString::from("provider-editor"))
            .debug_selector(|| "provider-editor".to_string())
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .pt(px(12.0))
                    .children(body),
            )
            .child(actions)
            .into_any_element()
    }
    /// The API key row, including the vault chooser that names the source.
    ///
    /// The plaintext field is the source whenever it is non-empty, matching the
    /// resolver's order. When the vault is unlocked the row offers its entries;
    /// a chosen entry `disables` the plaintext field and names itself, while a
    /// locked vault keeps the field editable, names any saved choice, and hints
    /// that unlocking would offer the menu.
    fn provider_api_key_field(&self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let selected = self
            .state
            .draft()
            .global
            .providers
            .get(id)
            .and_then(|provider| provider.api_key_vault.as_deref())
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string);
        let unlocked = self.state.vault.is_app_available();
        // No plaintext field exists: the vault entry is the only source. The
        // app access key keeps the chooser available even while the Vault pane
        // is locked; without that key the row explains how to grant access.
        let mut column = div().flex().flex_col().gap(px(6.0));
        if unlocked {
            column = column.child(self.provider_vault_chooser(id, selected.as_deref(), cx));
        } else {
            if selected.is_some() {
                column = column.child(
                    div()
                        .debug_selector(|| "provider-vault-source-locked".to_string())
                        .child(txt(
                            vault_source_label(lang, selected.as_deref()),
                            12.0,
                            theme::faint(),
                        )),
                );
            }
            let hint = if self.state.vault.exists() {
                "settings.vaultUnlockToChoose"
            } else {
                "settings.vaultCreateToChoose"
            };
            column = column.child(
                div()
                    .debug_selector(|| "provider-vault-hint".to_string())
                    .child(txt(lang.tr(hint), 12.0, theme::faint())),
            );
        }
        if unlocked && selected.is_some() {
            column = column.child(
                div()
                    .debug_selector(|| "provider-vault-note".to_string())
                    .child(txt(
                        lang.tr("settings.vaultKeyFromVault"),
                        12.0,
                        theme::faint(),
                    )),
            );
        }
        self.editor_field("settings.apiKey", column.into_any_element())
    }

    /// The vault source control: one button naming the current source, opening
    /// the unlocked vault's entry ids and a Clear row.
    fn provider_vault_chooser(
        &self,
        id: &str,
        selected: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let source = selected.map(str::to_string);
        let entries: Vec<String> = self
            .state
            .vault
            .entries()
            .map(|entries| entries.into_iter().map(|entry| entry.name).collect())
            .unwrap_or_default();
        let shell = cx.entity();
        let provider = id.to_string();
        let label = vault_source_label(lang, selected);
        let debug = if selected.is_some() {
            "provider-vault-source"
        } else {
            "provider-vault-choose"
        };
        div()
            .debug_selector(move || debug.to_string())
            .child(
                Button::new("provider-api-key-vault")
                    .ghost()
                    .h(px(32.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::selected_row()))
                    .cursor_pointer()
                    .label(label)
                    .icon(IconName::ChevronDown)
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                        let mut menu = menu.min_w(px(260.0));
                        if entries.is_empty() && source.is_none() {
                            return menu.item(
                                PopupMenuItem::new(lang.tr("settings.vaultNoEntries"))
                                    .disabled(true),
                            );
                        }
                        for entry in entries.iter() {
                            let picked = entry.clone();
                            let provider = provider.clone();
                            let pick = shell.clone();
                            let active = source.as_deref() == Some(entry.as_str());
                            menu = menu.item(
                                PopupMenuItem::new(entry.clone()).checked(active).on_click(
                                    move |_, window, cx| {
                                        pick.update(cx, |this, cx| {
                                            this.choose_provider_vault_key(
                                                &provider,
                                                Some(&picked),
                                                window,
                                                cx,
                                            );
                                        });
                                    },
                                ),
                            );
                        }
                        if source.is_some() {
                            let provider = provider.clone();
                            let pick = shell.clone();
                            menu = menu.item(PopupMenuItem::separator()).item(
                                PopupMenuItem::new(lang.tr("settings.vaultClear")).on_click(
                                    move |_, window, cx| {
                                        pick.update(cx, |this, cx| {
                                            this.choose_provider_vault_key(
                                                &provider, None, window, cx,
                                            );
                                        });
                                    },
                                ),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }
    /// The add card's API key row, with the same vault chooser as the editor.
    ///
    /// The provider this card creates does not exist yet, so the choice is
    /// pending on the shell until Create lands it on the new provider. The
    /// chooser is offered only while the vault is unlocked, exactly as in the
    /// editor; with the vault unlocked a chosen entry disables the plaintext
    /// field, while a locked vault keeps it editable.
    fn add_api_key_field(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let selected = self
            .add_key_vault
            .as_deref()
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string);
        let unlocked = self.state.vault.is_app_available();
        // Same rule as the editor: app access keeps the chooser available even
        // while the admin view is locked.
        let mut column = div().flex().flex_col().gap(px(6.0));
        if unlocked {
            column = column.child(self.add_vault_chooser(selected.as_deref(), cx));
        } else {
            if selected.is_some() {
                column = column.child(
                    div()
                        .debug_selector(|| "add-vault-source-locked".to_string())
                        .child(txt(
                            vault_source_label(lang, selected.as_deref()),
                            12.0,
                            theme::faint(),
                        )),
                );
            }
            let hint = if self.state.vault.exists() {
                "settings.vaultUnlockToChoose"
            } else {
                "settings.vaultCreateToChoose"
            };
            column = column.child(
                div()
                    .debug_selector(|| "add-vault-hint".to_string())
                    .child(txt(lang.tr(hint), 12.0, theme::faint())),
            );
        }
        if unlocked && selected.is_some() {
            column = column.child(div().debug_selector(|| "add-vault-note".to_string()).child(
                txt(lang.tr("settings.vaultKeyFromVault"), 12.0, theme::faint()),
            ));
        }
        self.editor_field("settings.apiKey", column.into_any_element())
    }

    /// The add card's vault source control, matching the editor's chooser.
    fn add_vault_chooser(&self, selected: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let source = selected.map(str::to_string);
        let entries: Vec<String> = self
            .state
            .vault
            .entries()
            .map(|entries| entries.into_iter().map(|entry| entry.name).collect())
            .unwrap_or_default();
        let shell = cx.entity();
        let label = vault_source_label(lang, selected);
        let debug = if selected.is_some() {
            "add-vault-source"
        } else {
            "add-vault-choose"
        };
        div()
            .debug_selector(move || debug.to_string())
            .child(
                Button::new("add-api-key-vault")
                    .ghost()
                    .h(px(32.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::selected_row()))
                    .cursor_pointer()
                    .label(label)
                    .icon(IconName::ChevronDown)
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                        let mut menu = menu.min_w(px(260.0));
                        if entries.is_empty() && source.is_none() {
                            return menu.item(
                                PopupMenuItem::new(lang.tr("settings.vaultNoEntries"))
                                    .disabled(true),
                            );
                        }
                        for entry in entries.iter() {
                            let picked = entry.clone();
                            let pick = shell.clone();
                            let active = source.as_deref() == Some(entry.as_str());
                            menu = menu.item(
                                PopupMenuItem::new(entry.clone()).checked(active).on_click(
                                    move |_, window, cx| {
                                        pick.update(cx, |this, cx| {
                                            this.choose_add_vault_key(Some(&picked), window, cx);
                                        });
                                    },
                                ),
                            );
                        }
                        if source.is_some() {
                            let pick = shell.clone();
                            menu = menu.item(PopupMenuItem::separator()).item(
                                PopupMenuItem::new(lang.tr("settings.vaultClear")).on_click(
                                    move |_, window, cx| {
                                        pick.update(cx, |this, cx| {
                                            this.choose_add_vault_key(None, window, cx);
                                        });
                                    },
                                ),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }
    /// Commit the editor: the connection it holds, then every model leaf.
    ///
    /// The rows are read on the click, not on a keystroke, so a half-typed
    /// row never reaches the tree; and the provider id is not editable,
    /// because it is the registry key the wire model carries.
    fn apply_provider_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let base_url = self.add_base_url.read(cx).value().trim().to_string();
        let name = self.add_display.read(cx).value().trim().to_string();
        if base_url.is_empty() {
            self.state.provider_refusal =
                Some(crate::settings::provider::ProviderRefusal::NeedsBaseUrl.key());
            cx.notify();
            return;
        }
        if let Some(refusal) = crate::settings::provider::base_url_refusal(&base_url) {
            self.state.provider_refusal = Some(refusal.key());
            cx.notify();
            return;
        }
        let rows: Vec<(String, crate::settings::global::ModelEntry)> = self
            .model_rows
            .iter()
            .map(|row| {
                let display = row.name.read(cx).value().trim().to_string();
                let mut leaf = row.entry.clone();
                leaf.model = row.id.read(cx).value().trim().to_string();
                leaf.name = (!display.is_empty()).then_some(display);
                (row.key.clone(), leaf)
            })
            .collect();
        self.state.provider_refusal = None;
        for (key, leaf) in rows {
            if let Err(refusal) = self.state.update_model(id, &key, &leaf) {
                self.state.provider_refusal = Some(refusal.key());
            }
        }
        let owned = id.to_string();
        self.state.set_provider_connection(&owned, &base_url);
        self.state.set_provider_name(&owned, &name);
        self.state.editing_provider = None;
        self.model_rows.clear();
        self.models_advanced_open = false;
        self.reload_model_rows(&owned, window, cx);
    }
    /// The model list: the measured head, one entry per leaf and the add row.
    fn model_list_rows(&self, id: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let lang = self.state.lang;
        let mut rows: Vec<AnyElement> = Vec::with_capacity(self.model_rows.len() + 3);
        let customized = !self.model_rows.is_empty();
        // The head carries the inherited/customized meta and the reset link,
        // which drops the whole catalog override in one press.
        let owned = id.to_string();
        let reset = div()
            .id(SharedString::from("models-reset"))
            .debug_selector(|| "models-reset".to_string())
            .cursor_pointer()
            .child(txt(lang.tr("settings.resetModels"), 12.0, theme::muted()))
            .on_click(cx.listener({
                let owned = owned.clone();
                move |this, _event, _window, cx| {
                    this.state.remove_provider(&owned);
                    this.model_rows.clear();
                    this.state.editing_provider = None;
                    this.models_advanced_open = false;
                    // A provider that is gone cannot be asked, so the
                    // trigger stops naming an ask that will never land.
                    this.models_fetching = false;
                    cx.notify();
                }
            }))
            .into_any_element();
        // While the ask is in flight the trigger names the ask, so a
        // second press does not start a second request.
        let fetch_label = if self.models_fetching {
            lang.tr("settings.fetching")
        } else {
            lang.tr("settings.models.fetchModels")
        };
        let fetch = div()
            .id(SharedString::from("models-fetch"))
            .debug_selector(|| "models-fetch".to_string())
            .cursor_pointer()
            .child(txt(fetch_label, 12.0, theme::muted()))
            .on_click(cx.listener({
                let owned = owned.clone();
                move |this, _event, _window, cx| {
                    this.fetch_models(&owned, cx);
                }
            }))
            .into_any_element();
        // The measured catalogue heading: the title above the meta line,
        // in one column, with the link buttons parked at its top right.
        let heading = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                txt(lang.tr("settings.models"), 12.0, theme::muted())
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(txt(
                if customized {
                    lang.tr("settings.modelsCustomized")
                } else {
                    lang.tr("settings.modelsInherited")
                },
                12.0,
                theme::faint(),
            ));
        // Restore defaults leads the toolbar, and the fetch follows it, which is
        // the order the reference's own catalog toolbar names.
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(reset)
            .child(fetch);
        let head = div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(px(12.0))
            .child(heading)
            .child(actions);
        rows.push(head.into_any_element());
        if !customized {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py(px(12.0))
                    .rounded(px(16.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .child(txt(lang.tr("settings.modelsEmpty"), 12.0, theme::faint()))
                    .into_any_element(),
            );
        }
        for index in 0..self.model_rows.len() {
            rows.push(self.model_row(id, index, cx));
        }
        rows.extend(self.fetch_rows(id, cx));
        // The row the add control needs: the reference pairs its add button
        // with the id being added, so the press has something to add. Without
        // these fields the button could only ever report `settings.modelIdRequired`
        // about a field the pane never drew.
        //
        // The editor's own engines, not the add card's: this form adds a model
        // to the provider already open, which is a different fact from the one
        // the custom route types for a provider that does not exist yet.
        let new_id_selector = SharedString::from("model-new-id");
        let new_name_selector = SharedString::from("model-new-name");
        rows.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            Input::new(&self.new_model_id)
                                .id(SharedString::from("model-new-id-field"))
                                .appearance(false)
                                .w_full()
                                .h(px(28.0))
                                .rounded(px(4.0))
                                .bg(rgb(theme::page()))
                                .border_1()
                                .border_color(rgb(theme::border()))
                                .px(px(8.0)),
                        )
                        .debug_selector(move || new_id_selector.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            Input::new(&self.new_model_name)
                                .id(SharedString::from("model-new-name-field"))
                                .appearance(false)
                                .w_full()
                                .h(px(28.0))
                                .rounded(px(4.0))
                                .bg(rgb(theme::page()))
                                .border_1()
                                .border_color(rgb(theme::border()))
                                .px(px(8.0)),
                        )
                        .debug_selector(move || new_name_selector.to_string()),
                )
                .into_any_element(),
        );
        // The measured `addModelButton`: a 28px link-sized control on a
        // `.5px` l3 rule, its own 4px gap and `align-self: flex-start`.
        let add = hairline_all(
            div()
                .id(SharedString::from("model-add"))
                .debug_selector(|| "model-add".to_string())
                .flex()
                .flex_row()
                .items_center()
                .self_start()
                .gap(px(4.0))
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(4.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border()))
                .cursor_pointer()
                .child(icon(IconName::Plus, 14.0, theme::text()))
                .child(txt(lang.tr("settings.addModel"), 12.0, theme::text()))
                .on_click(cx.listener({
                    let owned = owned.clone();
                    move |this, _event, window, cx| {
                        let typed = this.new_model_id.read(cx).value().trim().to_string();
                        let named = this.new_model_name.read(cx).value().trim().to_string();
                        let outcome = this.state.add_model(&owned, &typed, &named, &typed, "");
                        this.state.provider_refusal = outcome.err().map(|refusal| refusal.key());
                        // A row that landed leaves the fields empty, so the next
                        // press adds a second model rather than re-adding this
                        // one. A refusal keeps the text, because the reader has
                        // to correct what they typed.
                        if outcome.is_ok() {
                            this.new_model_id.update(cx, |state, cx| {
                                state.set_value(String::new(), window, cx);
                            });
                            this.new_model_name.update(cx, |state, cx| {
                                state.set_value(String::new(), window, cx);
                            });
                        }
                        this.reload_model_rows(&owned, window, cx);
                    }
                })),
        );
        rows.push(add.into_any_element());
        rows
    }

    /// Rebuild the per-row engines from the tree, so the pane never holds a
    /// second copy of a leaf it is only echoing.
    pub(crate) fn reload_model_rows(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The draft, not the saved tree: every write in this pane lands in the
        // draft, so rebuilding the rows from `settings` re-echoes the copy the
        // reader has not applied yet and drops the row they just added.
        let provider = self.state.draft().global.providers.get(id).cloned();
        let Some(provider) = provider else {
            self.model_rows.clear();
            cx.notify();
            return;
        };
        self.model_rows = provider
            .models
            .iter()
            .map(|(key, entry)| ModelRowEditor {
                key: key.clone(),
                entry: entry.clone(),
                id: cx.new(|cx| InputState::new(window, cx)),
                name: cx.new(|cx| InputState::new(window, cx)),
                open: false,
            })
            .collect();
        for (row, (_, entry)) in self.model_rows.iter_mut().zip(provider.models.iter()) {
            set_value(&row.id, entry.model.clone(), window, cx);
            set_value(
                &row.name,
                entry.name.clone().unwrap_or_default(),
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// One measured model row: the id and name inputs, the chevron and the
    /// trash, with the declared input types under the row.
    /// The endpoint model picker: what a successful fetch returned, which of
    /// it is ticked, and the adopt control that turns the ticks into rows.
    ///
    /// A refusal is a sentence rather than an empty list, because an empty list
    /// says the provider has nothing where a refused call says the ask never
    /// landed. Nothing is held open between panes: closing the provider editor
    /// drops the fetched list along with it.
    fn fetch_rows(&self, id: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = Vec::new();
        if self.models_fetch_needs_base_url {
            rows.push(
                txt(
                    self.state.lang.tr("settings.models.fetchNeedsBaseUrl"),
                    12.0,
                    theme::error(),
                )
                .into_any_element(),
            );
            return rows;
        }
        if self.models_fetch_failed {
            rows.push(
                txt(
                    self.state.lang.tr("settings.models.fetchEmpty"),
                    12.0,
                    theme::faint(),
                )
                .into_any_element(),
            );
            return rows;
        }
        let Some(fetched) = self.models_fetched.clone() else {
            return rows;
        };
        let lang = self.state.lang;
        // The search narrows what the provider returned, so a row's index is its
        // position in the filtered list: the pane never renumbers a list a
        // filter has already cut down.
        let query = self.state.picker_query.trim().to_ascii_lowercase();
        let shown: Vec<String> = fetched
            .iter()
            .filter(|model| query.is_empty() || model.to_ascii_lowercase().contains(&query))
            .cloned()
            .collect();
        let empty = shown.is_empty();
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(rgb(theme::page()))
                .border_1()
                .border_color(rgb(theme::border_soft()))
                .child(txt(
                    lang.tr("settings.models.fetchTitle"),
                    12.0,
                    theme::text(),
                ))
                .children(shown.iter().enumerate().map(|(index, model)| {
                    let ticked = self.models_selected.contains(model);
                    let pick = model.clone();
                    div()
                        .id(SharedString::from(format!("fetch-{index}")))
                        .debug_selector(|| format!("fetch-{index}"))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(28.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .child(icon(
                            if ticked {
                                IconName::Check
                            } else {
                                IconName::Plus
                            },
                            12.0,
                            theme::text(),
                        ))
                        .child(txt(model.clone(), 12.0, theme::text()).flex_1())
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_fetched_model(&pick);
                            cx.notify();
                        }))
                        .into_any_element()
                }))
                .when(empty, |box_| {
                    box_.child(
                        div()
                            .id(SharedString::from("fetch-empty"))
                            .debug_selector(|| "fetch-empty".to_string())
                            .child(
                                txt(lang.tr("settings.models.fetchEmpty"), 12.0, theme::faint())
                                    .into_any_element(),
                            )
                            .into_any_element(),
                    )
                })
                .child({
                    let owned = id.to_string();
                    div()
                        .id(SharedString::from("fetch-adopt"))
                        .debug_selector(|| "fetch-adopt".to_string())
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .h(px(28.0))
                        .px(px(10.0))
                        .rounded(px(8.0))
                        .bg(rgb(theme::accent()))
                        .cursor_pointer()
                        .child(txt(lang.tr("settings.addSelected"), 12.0, theme::page()))
                        .on_click(cx.listener(move |this, _event, window, cx| {
                            this.add_selected_models(&owned, window, cx);
                        }))
                        .into_any_element()
                })
                .into_any_element(),
        );
        rows
    }

    fn model_row(&self, id: &str, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let Some(row) = self.model_rows.get(index) else {
            return div().into_any_element();
        };
        let id_input = Input::new(&row.id)
            .appearance(false)
            .flex_1()
            .h(px(32.0))
            .rounded(px(8.0))
            .bg(rgb(theme::page()))
            .border_1()
            .border_color(rgb(theme::border()))
            .px(px(10.0));
        let name_input = Input::new(&row.name)
            .appearance(false)
            .flex_1()
            .h(px(32.0))
            .rounded(px(8.0))
            .bg(rgb(theme::page()))
            .border_1()
            .border_color(rgb(theme::border()))
            .px(px(10.0));
        let open = row.open;
        let tip = lang.tr("settings.models.modelAdvanced");
        let chevron = div()
            .id(SharedString::from(format!("model-row-{index}")))
            .debug_selector(move || format!("model-row-{index}"))
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .child(if open {
                icon(IconName::ChevronDown, 12.0, theme::faint())
            } else {
                icon(IconName::ChevronRight, 12.0, theme::faint())
            })
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if let Some(row) = this.model_rows.get_mut(index) {
                    row.open = !open;
                }
                cx.notify();
            }))
            .into_any_element();
        let owned = id.to_string();
        let key = row.key.clone();
        let leaf = key.clone();
        let trash = div()
            .id(SharedString::from(format!("model-row-remove-{index}")))
            .debug_selector(move || format!("model-row-remove-{index}"))
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .child(icon(IconName::Trash, 14.0, theme::faint()))
            .on_click(cx.listener({
                let owned = owned.clone();
                move |this, _event, window, cx| {
                    this.state.remove_model(&owned, &key);
                    this.reload_model_rows(&owned, window, cx);
                }
            }))
            .into_any_element();
        let entry = hairline_all(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .py(px(6.0))
                .px(px(8.0))
                .rounded(px(16.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(id_input)
                        .child(name_input)
                        .child(chevron)
                        .child(trash),
                )
                .when(open, |row| {
                    row.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .pt(px(6.0))
                            .child(self.model_input_types(id, &leaf, index, cx)),
                    )
                }),
        );
        entry.into_any_element()
    }

    /// The declared input types of one model, as the reference's
    /// `ModelInputTypes` checkbox pair: the last one standing cannot be turned
    /// off, so a model never ends up with nothing to send.
    fn model_input_types(
        &self,
        id: &str,
        key: &str,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let draft = self.state.draft();
        let declared = draft.global.input_types_of(id, key);
        let owned = id.to_string();
        let mut row = div().flex().flex_row().items_center().gap(px(16.0));
        for entry in ["text", "image"] {
            let id = owned.clone();
            let key = key.to_string();
            let kind = entry.to_string();
            let checked = declared.iter().any(|entry| entry.as_str() == kind);
            let locked =
                declared.len() == 1 && declared.first().map(String::as_str) == Some(kind.as_str());
            let image = kind == "image";
            let tag = kind.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("model-input-{index}-{kind}")))
                    .debug_selector(move || format!("model-input-{index}-{tag}"))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .opacity(if locked { 0.4 } else { 1.0 })
                    .child(if checked {
                        icon(IconName::Check, 12.0, theme::text())
                    } else {
                        icon(IconName::Plus, 12.0, theme::faint())
                    })
                    .child(txt(
                        if image {
                            lang.tr("settings.inputImage")
                        } else {
                            lang.tr("settings.inputText")
                        },
                        12.0,
                        theme::muted(),
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.toggle_model_input(&id, &key, &kind);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        row.into_any_element()
    }

    /// The measured editor field: a label, and the control under it.
    ///
    /// The label argument is a locale KEY, not the copy: every caller names the
    /// field it draws (`settings.apiKey`, `settings.customRoute`, ...), and
    /// the pane is the one place that knows the current language. Translating
    /// here is what makes those arguments keys; without it the key itself is
    /// drawn, which is how the provider card showed `settings.provider`
    /// instead of "Provider".
    fn editor_field(&self, label: &'static str, control: AnyElement) -> AnyElement {
        let lang = self.state.lang;
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div().flex().items_center().gap(px(10.0)).child(
                    txt(lang.tr(label), 12.0, theme::muted()).font_weight(FontWeight::MEDIUM),
                ),
            )
            .child(control)
            .into_any_element()
    }

    /// A bare text input: the measured input, without the kit's wrapper chrome.
    fn editor_input(&self, input: &Entity<InputState>, id: String) -> AnyElement {
        let selector = id.clone();
        let field = Input::new(input)
            .appearance(false)
            .w_full()
            .h(px(36.0))
            .rounded(px(8.0))
            .bg(rgb(theme::page()))
            .border_1()
            .border_color(rgb(theme::border()))
            .px(px(12.0))
            .into_any_element();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .w_full()
            .child(field)
            .into_any_element()
    }

    /// The measured protocol picker: three segments, the chosen one filled.
    /// Changing the protocol leaves the typed address alone, the way the
    /// reference does.
    fn api_mode_choices(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let open = self.state.editing_provider.clone().unwrap_or_default();
        let current = self.state.draft().global.family_protocol_of(&open);
        let mut row = div().flex().flex_row().gap(px(4.0));
        for protocol in crate::settings::provider::PROTOCOLS {
            let owned = open.clone();
            let mode = protocol.to_string();
            let tag = mode.clone();
            let chosen = current == tag;
            let label = match mode.as_str() {
                "responses" => lang.tr("settings.apiResponses"),
                "anthropic" => lang.tr("settings.apiAnthropic"),
                _ => lang.tr("settings.apiChat"),
            };
            row = row.child(
                div()
                    .id(SharedString::from(format!("api-mode-{mode}")))
                    .debug_selector(move || format!("api-mode-{tag}"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(36.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .bg(if chosen {
                        rgb(theme::text())
                    } else {
                        rgb(theme::page())
                    })
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .cursor_pointer()
                    .child(txt(
                        label,
                        14.0,
                        if chosen { theme::page() } else { theme::text() },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state
                            .draft_mut()
                            .global
                            .set_family_protocol(&owned, &mode);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        row.into_any_element()
    }
    /// The dashed add button, and the card it swaps for once it is open.
    pub(crate) fn add_provider_block(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        if !self.models_add_open {
            return div()
                .flex()
                .flex_row()
                .flex_1()
                .gap(px(6.0))
                .child(
                    div()
                        .id(SharedString::from("provider-add"))
                        .debug_selector(|| "provider-add".to_string())
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .h(px(44.0))
                        .min_w(px(180.0))
                        .rounded(px(16.0))
                        .border_1()
                        .border_color(rgb(theme::border()))
                        .cursor_pointer()
                        .child(icon(IconName::Plus, 14.0, theme::text()))
                        .child(txt(lang.tr("settings.add"), 14.0, theme::text()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            let addable = this.state.draft().global.addable_providers();
                            this.models_add_route = if addable.is_empty() {
                                ModelsAddRoute::Custom
                            } else {
                                ModelsAddRoute::Catalogue
                            };
                            this.models_add_open = true;
                            this.add_key_vault = None;
                            cx.notify();
                        }))
                        .into_any_element(),
                )
                .into_any_element();
        }
        self.add_card(cx)
    }

    /// The add card: the measured segmented control, then the panel the chosen
    /// route owns.
    fn add_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let route = self.models_add_route;
        let mut modes = div().flex().flex_row().gap(px(4.0));
        for (kind, label) in [
            (ModelsAddRoute::Catalogue, lang.tr("settings.addCatalog")),
            (ModelsAddRoute::Custom, lang.tr("settings.addCustom")),
        ] {
            let chosen = kind == route;
            modes = modes.child(
                div()
                    .id(SharedString::from(format!("add-route-{kind:?}")))
                    .debug_selector(move || format!("add-route-{kind:?}"))
                    .flex()
                    .items_center()
                    .h(px(32.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .bg(if chosen {
                        rgb(theme::text())
                    } else {
                        rgb(theme::page())
                    })
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .cursor_pointer()
                    .child(txt(
                        label,
                        12.0,
                        if chosen { theme::page() } else { theme::text() },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.models_add_route = kind;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        // One line of purpose under the switch, the way the reference explains
        // what each route is for.
        let purpose = if route == ModelsAddRoute::Catalogue {
            lang.tr("settings.addCatalogHint")
        } else {
            lang.tr("settings.addCustomHint")
        };
        let panel = div().flex().flex_col().gap(px(14.0)).pt(px(12.0));
        let panel = match route {
            ModelsAddRoute::Catalogue => panel.child(self.catalogue_pick(cx)),
            ModelsAddRoute::Custom => panel.child(self.custom_panel(cx)),
        };
        let card = hairline_all(
            div()
                .id(SharedString::from("models-add-card"))
                .debug_selector(|| "models-add-card".to_string())
                .flex()
                .flex_col()
                .gap(px(12.0))
                .py(px(14.0))
                .px(px(16.0))
                .rounded(px(16.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border_soft()))
                .child(modes)
                .child(txt(purpose, 12.0, theme::faint()))
                .child(panel),
        );
        card.into_any_element()
    }

    /// The catalogue route: the reference's `Provider` label and its `select`,
    /// then the key field and the create action.
    ///
    /// The reference's chooser is a `select.input.selectInput`, so what the
    /// reader sees is one closed box naming the provider id beside a chevron,
    /// and the rest of the catalogue is drawn only while it is open. The rows
    /// this replaced claimed a place on screen the reference gives to the
    /// editor, and they read as a set of choices to make rather than the one
    /// value a select holds, which is what the provider id is.
    fn catalogue_pick(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let addable = self.state.draft().global.addable_providers();
        let chosen = self
            .catalogue_choice
            .clone()
            .or_else(|| addable.first().cloned());
        let Some(chosen) = chosen else {
            return txt(lang.tr("settings.modelsEmpty"), 12.0, theme::faint()).into_any_element();
        };
        // The closed box: the provider id on the left, the chevron at the
        // right, with the measured inner padding reserving the room for it.
        let closed = hairline_all(
            div()
                .id(SharedString::from("catalogue-select"))
                .debug_selector(|| "catalogue-select".to_string())
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .max_w(px(240.0))
                .h(px(32.0))
                .px(px(10.0))
                .pr(px(32.0))
                .rounded(px(8.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border()))
                .cursor_pointer()
                .child(txt(chosen.clone(), 14.0, theme::text()))
                .child(icon(IconName::ChevronDown, 12.0, theme::faint()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.catalogue_open = !this.catalogue_open;
                    cx.notify();
                })),
        );
        let mut boxed = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(closed.into_any_element());
        if self.catalogue_open {
            // The open list: every provider id the file does not carry yet,
            // the chosen one marked, and a pick that also closes again.
            let mut options = hairline_all(
                div()
                    .flex()
                    .flex_col()
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::page()))
                    .border_color(rgb(theme::border())),
            );
            for id in &addable {
                let owned = id.clone();
                let selected = *id == chosen;
                options = options.child(
                    div()
                        .id(SharedString::from(format!("catalogue-option-{id}")))
                        .debug_selector(move || format!("catalogue-option-{id}"))
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(28.0))
                        .px(px(10.0))
                        .bg(if selected {
                            rgb(theme::hover())
                        } else {
                            rgb(theme::page())
                        })
                        .cursor_pointer()
                        .child(txt(id.clone(), 14.0, theme::text()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.catalogue_choice = Some(owned.clone());
                            this.catalogue_open = false;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            boxed = boxed.child(options.into_any_element());
        }
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(self.editor_field("settings.provider", boxed.into_any_element()))
            .child(self.add_api_key_field(cx))
            .child(self.add_actions(&chosen, cx))
            .into_any_element()
    }

    /// The custom route: the measured field order, and the create button that
    /// refuses to fire until every required field is filled.
    fn custom_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let route = self.add_route.read(cx).value().trim().to_string();
        let base_url = self.add_base_url.read(cx).value().trim().to_string();
        let taken = self.state.draft().global.providers.contains_key(&route);
        let address = crate::settings::provider::base_url_refusal(&base_url);
        let ready = !route.is_empty()
            && !base_url.is_empty()
            && address.is_none()
            && !taken
            && !self.state.draft().global.providers.is_empty()
            && !self.model_rows.is_empty();
        let _ = ready;
        let model_field = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(txt(lang.tr("settings.modelId"), 12.0, theme::muted()))
            .child(
                Input::new(&self.new_row_id)
                    .id(SharedString::from("add-model-id"))
                    .appearance(false)
                    .flex_1()
                    .h(px(36.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::page()))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .px(px(12.0)),
            )
            .child(
                Input::new(&self.new_row_name)
                    .id(SharedString::from("add-model-name"))
                    .appearance(false)
                    .flex_1()
                    .h(px(36.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::page()))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .px(px(12.0)),
            );
        let mut column = div().flex().flex_col().gap(px(14.0));
        column = column
            .child(self.editor_field(
                "settings.customRoute",
                self.editor_input(&self.add_route, "add-route".to_string()),
            ))
            .child(self.editor_field(
                "settings.displayName",
                self.editor_input(&self.add_display, "add-display".to_string()),
            ))
            .child(self.editor_field(
                "settings.customBaseUrl",
                self.editor_input(&self.add_base_url, "add-base-url".to_string()),
            ))
            .child(self.editor_field("settings.apiMode", self.api_mode_choices(cx)))
            .child(self.add_api_key_field(cx))
            .child(model_field);
        // The refusals the reference prints above the create button, in the
        // order it checks them.
        if route.is_empty() {
            column = column.child(self.add_note(lang.tr("settings.providerIdRequired")));
        } else if taken {
            column = column.child(self.add_note(lang.tr("settings.providerIdTaken")));
        }
        if base_url.is_empty() {
            column = column.child(self.add_note(lang.tr("settings.customNeedsBaseUrl")));
        } else if let Some(refusal) = address {
            column = column.child(self.add_note(lang.tr(refusal.key())));
        }
        column
            .child(self.add_actions(&route, cx))
            .into_any_element()
    }

    /// One line of refusal copy, the measured inline error style.
    fn add_note(&self, copy: String) -> AnyElement {
        txt(copy, 12.0, theme::error()).into_any_element()
    }

    /// The create and cancel buttons at the foot of the add card.
    fn add_actions(&self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let owned = id.to_string();
        let create = div()
            .id(SharedString::from("add-create"))
            .debug_selector(|| "add-create".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .bg(rgb(theme::text()))
            .cursor_pointer()
            .child(txt(lang.tr("settings.create"), 14.0, theme::page()))
            .on_click(cx.listener({
                let owned = owned.clone();
                move |this, _event, _window, cx| {
                    this.create_provider(&owned, cx);
                }
            }))
            .into_any_element();
        let cancel = div()
            .id(SharedString::from("add-cancel"))
            .debug_selector(|| "add-cancel".to_string())
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme::border()))
            .cursor_pointer()
            .child(txt(lang.tr("settings.cancel"), 14.0, theme::text()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.models_add_open = false;
                this.catalogue_choice = None;
                this.catalogue_open = false;
                this.add_key_vault = None;
                cx.notify();
            }))
            .into_any_element();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(create)
            .child(cancel)
            .into_any_element()
    }

    /// Commit the add card, on the route it is showing.
    pub(crate) fn create_provider(&mut self, id: &str, cx: &mut Context<Self>) {
        let route = self.models_add_route;
        let base_url = self.add_base_url.read(cx).value().trim().to_string();
        let named = self.add_display.read(cx).value().trim().to_string();
        let typed = self.new_row_id.read(cx).value().trim().to_string();
        let vault_entry = self.add_key_vault.clone();
        let outcome = match route {
            ModelsAddRoute::Catalogue => self.state.add_shipped_provider(id),
            ModelsAddRoute::Custom => self.state.add_custom_provider(
                id,
                &named,
                &base_url,
                &[crate::settings::global::ModelEntry::named(&typed)],
            ),
        };
        self.state.provider_refusal = outcome.err().map(|refusal| refusal.key());
        if outcome.is_ok() {
            if let Some(entry) = vault_entry.as_deref() {
                self.state.set_provider_api_key_vault(id, Some(entry));
            }
            // Adding a provider no longer selects it for anything: a model is
            // chosen per chat from the composer, and the workspace document
            // holds no selection. The card closes on its own write.
            self.models_add_open = false;
            self.catalogue_choice = None;
            self.catalogue_open = false;
            self.add_key_vault = None;
        }
        cx.notify();
    }

    /// Tools and skills, with a switch each.
    fn section_tools(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let lang = self.state.lang;
        let web = self.state.draft().workspace.agent.web_tools_enabled;
        // The hosted web tool has no local registrar, so it cannot appear in the
        // capability list below - the provider runs it. It is still a tool the
        // reader can switch off, so it leads the pane as its own row.
        // The section's hint is drawn under the title by the pane header, so it is
        // NOT repeated as the first row: the sentence appeared twice, once above
        // the list and once at the top of it.
        let mut rows: Vec<AnyElement> = vec![hairline_bottom(
            div()
                .py(px(12.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(24.0))
                .border_color(rgb(theme::border_soft()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(px(4.0))
                        .min_w_0()
                        .pr(px(48.0))
                        .child(txt(tr(lang, "settings.webToolsTitle"), 14.0, theme::text()))
                        .child(txt(
                            tr(lang, "settings.webToolsDescription"),
                            12.0,
                            theme::faint(),
                        )),
                )
                .child(self.switch("settings-web-tools", web, true, web_tools_action, cx)),
        )
        .into_any_element()];
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
                    .workspace
                    .disabled
                    .skills
                    .iter()
                    .any(|s| s == &name)
            } else {
                self.state
                    .draft()
                    .workspace
                    .disabled
                    .tools
                    .iter()
                    .any(|t| t == &name)
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
                let on = self.state.draft().workspace.agent.memory_enabled;
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
                let on = self.state.draft().workspace.agent.parallel_tool_calls;
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
        let total = draft.global.schedules.len();
        let tasks: Vec<(String, String, String, bool, String)> = draft
            .global
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
                    self.state
                        .draft()
                        .global
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
        // A task runs in the session on screen, which is a runtime fact; the
        // settings draft carries no session identity.
        let session = self.state.active_id.clone().unwrap_or_default();
        let refusal = self.state.draft_mut().global.add_schedule(
            &name,
            &prompt,
            &session,
            minutes.max(1) * 60,
        );
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
        self.state.draft_mut().global.toggle_schedule(id);
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
            let removed = self.state.draft_mut().global.remove_schedule(&id);
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
        let has_credential =
            self.state
                .draft()
                .global
                .providers
                .get(&name)
                .is_some_and(|provider| {
                    provider.api_key_vault.is_some()
                        || self
                            .state
                            .vault
                            .contains(&crate::vault::provider_key(&name))
                });
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
    /// The Extensions pane, in the reference's own shape.
    ///
    /// Measured from `dsh-client-ui-plugins`: a tab strip (Plugin list / MCP /
    /// Plugin Market), then the ACTIVE tab's body. The Plugin list tab is a
    /// full-width search field over two collapsible groups - the SESSION plugins
    /// an agent preset composes, and the GLOBAL plugins the system and every
    /// session share - drawn as cards two per row. The MCP tab is this crate's
    /// server management (`mcp_rows`), drawn in the same pane rather than on a
    /// page of its own.
    fn section_builtin_plugins(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let lang = self.state.lang;
        let mut rows: Vec<AnyElement> = vec![self.plugin_tabs(cx)];
        if self.plugins_tab == 1 {
            rows.extend(self.mcp_rows(cx));
            return ("settings.plugins", "settings.mcpIntro", rows);
        }
        if self.plugins_tab == 2 {
            rows.extend(self.skills_rows(cx));
            return ("settings.plugins", "settings.skillsIntro", rows);
        }
        rows.push(
            div()
                .mb(px(10.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .h(px(34.0))
                .px(px(10.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(rgb(theme::border()))
                .child(icon(IconName::Search, 14.0, theme::faint()))
                .child(Input::new(&self.plugin_search).appearance(false))
                .into_any_element(),
        );

        // ---- Session plugins: what the active preset composes. ----
        let draft = self.state.draft();
        let preset_id = draft.global.default_preset.clone();
        let preset_label = if preset_id.is_empty() {
            tr(lang, "settings.presetDefault")
        } else {
            preset_id.clone()
        };
        let composed: Vec<String> = draft
            .global
            .preset(&preset_id)
            .map(|preset| preset.plugins)
            .unwrap_or_default();
        let preset_ids = draft.global.preset_ids();
        let session_open = self.plugins_session_open;
        let global_open = self.plugins_global_open;

        rows.push(
            self.plugin_group_header(
                "plugins-session",
                tr(lang, "settings.pluginsSession"),
                self.preset_pill(preset_label, preset_ids, cx),
                !session_open,
                true,
                cx,
            )
            .into_any_element(),
        );
        if session_open {
            rows.push(
                txt(
                    tr(lang, "settings.pluginsSessionHint"),
                    12.0,
                    theme::faint(),
                )
                .pb(px(8.0))
                .into_any_element(),
            );
            if composed.is_empty() {
                rows.push(
                    txt(tr(lang, "settings.pluginsEmpty"), 12.0, theme::faint())
                        .pb(px(10.0))
                        .into_any_element(),
                );
            }
            for pair in composed.chunks(2) {
                let mut row = div().flex().flex_row().gap(px(8.0)).pb(px(8.0));
                for name in pair {
                    let tools = self.plugin_tools(name);
                    row = row.child(self.plugin_card(
                        format!("plugins-composed-{name}"),
                        name.clone(),
                        tools,
                        None,
                        None,
                        cx,
                    ));
                }
                if pair.len() == 1 {
                    row = row.child(div().flex_1());
                }
                rows.push(row.into_any_element());
            }
        }

        // ---- Global plugins: the installed inventory. ----
        rows.push(
            self.plugin_group_header(
                "plugins-global",
                tr(lang, "settings.pluginsGlobal"),
                div().into_any_element(),
                !global_open,
                false,
                cx,
            )
            .into_any_element(),
        );
        if global_open {
            rows.push(
                txt(tr(lang, "settings.pluginsGlobalHint"), 12.0, theme::faint())
                    .pb(px(8.0))
                    .into_any_element(),
            );
            let query = self.state.plugin_query.clone();
            let listed: Vec<(usize, String, bool)> = self
                .plugins
                .iter()
                .enumerate()
                .filter(|(_, plugin)| crate::state::matches(&query, &plugin.name))
                .map(|(index, plugin)| {
                    (
                        index,
                        plugin.name.clone(),
                        self.plugin_enabled(&plugin.name),
                    )
                })
                .collect();
            if listed.is_empty() {
                rows.push(
                    txt(tr(lang, "settings.noMatchingPlugins"), 12.0, theme::faint())
                        .pb(px(10.0))
                        .into_any_element(),
                );
            }
            for pair in listed.chunks(2) {
                let mut row = div().flex().flex_row().gap(px(8.0)).pb(px(8.0));
                for (index, name, enabled) in pair {
                    let tools = self.plugin_tools(name);
                    row = row.child(self.plugin_card(
                        format!("builtin-plugin-{index}"),
                        name.clone(),
                        tools,
                        Some(*enabled),
                        Some(name.clone()),
                        cx,
                    ));
                }
                if pair.len() == 1 {
                    row = row.child(div().flex_1());
                }
                rows.push(row.into_any_element());
            }
        }

        ("settings.plugins", "settings.builtinPluginsHint", rows)
    }

    /// The tools one plugin contributes, as the card's description.
    fn plugin_tools(&self, name: &str) -> String {
        self.plugins
            .iter()
            .find(|plugin| plugin.name == name)
            .map(|plugin| plugin.tools.join(", "))
            .filter(|tools| !tools.is_empty())
            .unwrap_or_else(|| tr(self.state.lang, "settings.pluginsNoTools"))
    }

    /// Switch one source on or off in the DRAFT.
    ///
    /// The write goes to the draft and nowhere else: Save is the one door a
    /// settings change goes through, so Cancel discards it like every other pane.
    pub(crate) fn toggle_source(&mut self, name: &str, cx: &mut Context<Self>) {
        let mut sources = self.state.draft().global.skill_sources();
        for source in sources.iter_mut() {
            if source.name == name {
                source.enabled = !source.enabled;
            }
        }
        self.state.draft_mut().global.set_skill_sources(sources);
        cx.notify();
    }

    /// Drop one source from the DRAFT.
    ///
    /// The framework directory is not removable: it is the one this build owns
    /// and writes, and a list without it would leave nothing to discover a
    /// first-run skill in. The refusal is silent to the click because the
    /// control is not drawn for that row, so there is no press to answer.
    pub(crate) fn remove_source(&mut self, name: &str, cx: &mut Context<Self>) {
        let mut sources = self.state.draft().global.skill_sources();
        sources.retain(|source| source.name != name || source.path == "~/.bos/skills");
        self.state.draft_mut().global.set_skill_sources(sources);
        cx.notify();
    }

    /// Add the custom directory the form holds to the DRAFT.
    ///
    /// An empty path adds nothing: a source with no directory has nothing to
    /// scan and would draw an empty card the reader cannot interpret. The name
    /// is optional and falls back to the path's last segment, which is what a
    /// reader who typed only a path expects to see.
    pub(crate) fn add_source(&mut self, cx: &mut Context<Self>) {
        let path = self.skill_add_path.read(cx).value().trim().to_string();
        if path.is_empty() {
            return;
        }
        let typed_name = self.skill_add_name.read(cx).value().trim().to_string();
        let name = if typed_name.is_empty() {
            crate::settings::global::source_name_for(&path)
        } else {
            typed_name
        };
        let mut sources = self.state.draft().global.skill_sources();
        if sources.iter().any(|source| source.path == path) {
            // The same directory twice is one source, so a repeat press does
            // nothing rather than drawing a duplicate card.
            return;
        }
        sources.push(crate::settings::global::SkillSource {
            name,
            path,
            enabled: true,
        });
        self.state.draft_mut().global.set_skill_sources(sources);
        self.skill_add_open = false;
        cx.notify();
    }

    /// The Skills tab: where skills come from and which ones are on.
    ///
    /// It lives here rather than on a rail cell of its own because the rail is
    /// the reference's five measured `settings.section` registrations plus the
    /// Vault, and a seventh cell would be a deviation with nothing behind it.
    /// The reference's own plugin pane is already the place a "what is installed
    /// and what is on" question is answered, and its tab strip is this strip, so
    /// Skills is the third REAL tab beside Plugin list and MCP.
    ///
    /// Shape, top to bottom: the measured hint line, the two controls (browse
    /// all, executor dirs), then one card per SOURCE - a coloured initial, the
    /// executor name, the skill count and the path. Both controls are
    /// disclosures over that grid, so the grid is what the pane always shows.
    fn skills_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let lang = self.state.lang;
        let sources = self.state.draft().global.skill_sources();
        // The catalogue is DISCOVERED against the draft, so a toggle the reader
        // has not saved yet already changes the counts they see. Building it here
        // rather than reading a cached list is what keeps the pane and the agent
        // from disagreeing about what "on" means.
        // The scan runs against the DRAFT through the one resolution the build
        // uses, so the counts on the cards, the list under Browse all, and the
        // skills the agent receives are three readings of a single fact. A second
        // scan here would be a second answer waiting to disagree.
        let effective = self.state.draft().effective();
        let resolved = effective.skill_sources.clone();
        let skills = effective.skills.clone();
        let _ = &sources;
        let mut rows: Vec<AnyElement> = Vec::new();

        rows.push(
            txt(tr(lang, "settings.skillsPick"), 12.0, theme::faint())
                .pb(px(10.0))
                .into_any_element(),
        );
        rows.push(self.skills_controls(&resolved, skills.len(), cx));

        if self.skills_browse_open {
            rows.extend(self.skills_browse(&skills, cx));
        }
        if self.skills_dirs_open {
            rows.extend(self.skills_dirs(&resolved, cx));
        } else {
            // The grid is the pane's resting state: one card per executor
            // directory, two to a row, exactly as the reference draws its own.
            for pair in resolved.chunks(2) {
                let mut row = div().flex().flex_row().gap(px(8.0)).pb(px(8.0));
                for source in pair {
                    row = row.child(self.skill_source_card(source, cx));
                }
                if pair.len() == 1 {
                    row = row.child(div().flex_1());
                }
                rows.push(row.into_any_element());
            }
        }
        rows
    }

    /// The two controls above the grid: browse all, and executor dirs.
    fn skills_controls(
        &self,
        sources: &[crate::settings::resolve::ResolvedSource],
        total: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .pb(px(12.0))
            .child(self.skills_control(
                "skills-browse",
                tr_args(
                    lang,
                    "settings.skillsBrowseAll",
                    &[("count", total.to_string())],
                ),
                self.skills_browse_open,
                SkillsControl::Browse,
                cx,
            ))
            .child(self.skills_control(
                "skills-dirs",
                tr_args(
                    lang,
                    "settings.skillsExecutorDirs",
                    &[("count", sources.len().to_string())],
                ),
                self.skills_dirs_open,
                SkillsControl::Dirs,
                cx,
            ))
            .into_any_element()
    }

    /// One of the pane's two disclosure controls.
    fn skills_control(
        &self,
        id: &'static str,
        label: String,
        open: bool,
        which: SkillsControl,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        hairline_all(
            div()
                .id(SharedString::from(id))
                .debug_selector(move || id.to_string())
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h(px(30.0))
                .px(px(10.0))
                .rounded(px(8.0))
                .bg(rgb(if open {
                    theme::new_session_fill()
                } else {
                    theme::page()
                }))
                .border_color(rgb(theme::border()))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::hover())))
                .child(icon(
                    if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    12.0,
                    theme::muted(),
                ))
                .child(txt(label, 12.0, theme::text()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    match which {
                        SkillsControl::Browse => {
                            this.skills_browse_open = !this.skills_browse_open;
                        }
                        SkillsControl::Dirs => this.skills_dirs_open = !this.skills_dirs_open,
                    }
                    cx.notify();
                })),
        )
        .into_any_element()
    }

    /// One source card: a coloured initial, the name, the count and the path.
    ///
    /// The initial's colour is derived from the name, so the six executor cards
    /// are visually distinct without a second table to keep in step - the same
    /// trick the reference's own avatar marks use.
    fn skill_source_card(
        &self,
        source: &crate::settings::resolve::ResolvedSource,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let initial = source
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        let card_id = format!("skills-source-{}", source.name);
        let selector = card_id.clone();
        let toggle_name = source.name.clone();
        let on = source.enabled;
        div()
            .id(SharedString::from(card_id))
            .debug_selector(move || selector.clone())
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(10.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(theme::page()))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(20.0))
                            .h(px(20.0))
                            .flex_none()
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(source_tint(&source.name)))
                            .child(txt(initial, 11.0, theme::page())),
                    )
                    .child(
                        txt(source.name.clone(), 13.0, theme::text())
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(txt(
                        tr_args(
                            lang,
                            "settings.skillsCount",
                            &[("count", source.count.to_string())],
                        ),
                        12.0,
                        theme::faint(),
                    ))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex_none()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(if on {
                                theme::accent()
                            } else {
                                theme::border_strong()
                            }))
                            .bg(rgb(if on { theme::accent() } else { theme::page() }))
                            .when(on, |boxed| {
                                boxed.child(icon(IconName::Check, 10.0, theme::page()))
                            }),
                    ),
            )
            .child(
                txt(source.path.clone(), 12.0, theme::muted())
                    .line_height(px(17.0))
                    .overflow_hidden(),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_source(&toggle_name, cx);
            }))
            .into_any_element()
    }

    /// The browse-all disclosure: every discovered skill, its source, and its
    /// own switch.
    fn skills_browse(
        &mut self,
        skills: &[crate::settings::resolve::DiscoveredSkill],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let lang = self.state.lang;
        let query = self.skill_query.clone();
        let mut rows: Vec<AnyElement> = vec![div()
            .mb(px(10.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(30.0))
            .px(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme::border()))
            .child(icon(IconName::Search, 13.0, theme::faint()))
            .child(Input::new(&self.skill_search).appearance(false))
            .into_any_element()];
        let listed: Vec<&crate::settings::resolve::DiscoveredSkill> = skills
            .iter()
            .filter(|skill| {
                query.is_empty()
                    || crate::state::matches(&query, &skill.name)
                    || crate::state::matches(&query, &skill.source)
            })
            .collect();
        if listed.is_empty() {
            rows.push(
                txt(tr(lang, "settings.skillsNone"), 12.0, theme::faint())
                    .pb(px(10.0))
                    .into_any_element(),
            );
        }
        for (index, skill) in listed.iter().enumerate() {
            let name = skill.name.clone();
            let on = skill.enabled;
            let row_id = format!("skills-browse-{index}");
            let selector = row_id.clone();
            rows.push(
                div()
                    .id(SharedString::from(row_id))
                    .debug_selector(move || selector.clone())
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
                            .flex_none()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(if on {
                                theme::accent()
                            } else {
                                theme::border_strong()
                            }))
                            .bg(rgb(if on { theme::accent() } else { theme::page() }))
                            .when(on, |boxed| {
                                boxed.child(icon(IconName::Check, 10.0, theme::page()))
                            }),
                    )
                    .child(txt(name.clone(), 13.0, theme::text()).w(px(160.0)))
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .bg(rgb(theme::new_session_fill()))
                            .child(txt(skill.source.clone(), 11.0, theme::muted())),
                    )
                    .child(txt(skill.description.clone(), 12.0, theme::faint()).flex_1())
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.toggle_skill_off(&name);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        rows
    }

    /// The executor-dirs disclosure: every source, its switch, a remove control
    /// and the add form.
    fn skills_dirs(
        &mut self,
        sources: &[crate::settings::resolve::ResolvedSource],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let lang = self.state.lang;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            let name = source.name.clone();
            let on = source.enabled;
            let missing = !source.exists;
            let row_id = format!("skills-dir-{index}");
            let selector = row_id.clone();
            let remove_name = name.clone();
            rows.push(
                div()
                    .id(SharedString::from(row_id))
                    .debug_selector(move || selector.clone())
                    .py(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(rgb(theme::border_soft()))
                    .child(
                        div()
                            .id(SharedString::from(format!("skills-dir-toggle-{index}")))
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex_none()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(if on {
                                theme::accent()
                            } else {
                                theme::border_strong()
                            }))
                            .bg(rgb(if on { theme::accent() } else { theme::page() }))
                            .cursor_pointer()
                            .when(on, |boxed| {
                                boxed.child(icon(IconName::Check, 10.0, theme::page()))
                            })
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_source(&name, cx);
                            })),
                    )
                    .child(txt(source.name.clone(), 13.0, theme::text()).w(px(120.0)))
                    .child(txt(source.path.clone(), 12.0, theme::faint()).flex_1())
                    .child(
                        // A directory that is not there says so rather than
                        // showing a silent zero: the reader can then fix the path
                        // instead of wondering why the card is empty.
                        txt(
                            if missing {
                                tr(lang, "settings.skillsMissing")
                            } else {
                                tr_args(
                                    lang,
                                    "settings.skillsCount",
                                    &[("count", source.count.to_string())],
                                )
                            },
                            11.0,
                            if missing {
                                theme::warn()
                            } else {
                                theme::faint()
                            },
                        )
                        .w(px(60.0)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("skills-dir-remove-{index}")))
                            .debug_selector(move || format!("skills-dir-remove-{index}"))
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(txt(tr(lang, "settings.skillsRemove"), 11.0, theme::muted()))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.remove_source(&remove_name, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            hairline_all(
                div()
                    .id("skills-add-toggle")
                    .debug_selector(|| "skills-add-toggle".to_string())
                    .mt(px(10.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .self_start()
                    .gap(px(4.0))
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::page()))
                    .border_color(rgb(theme::border()))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::Plus, 13.0, theme::text()))
                    .child(txt(tr(lang, "settings.skillsAdd"), 12.0, theme::text()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.skill_add_open = !this.skill_add_open;
                        cx.notify();
                    })),
            )
            .into_any_element(),
        );
        if self.skill_add_open {
            rows.push(self.skills_add_form(cx));
        }
        rows
    }

    /// The add-custom-dir form: a path and a name, then the write.
    fn skills_add_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        hairline_all(
            div()
                .mt(px(8.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border()))
                .child(self.skills_field(
                    "skills-add-path",
                    tr(lang, "settings.skillsPath"),
                    &self.skill_add_path,
                ))
                .child(self.skills_field(
                    "skills-add-name",
                    tr(lang, "settings.skillsName"),
                    &self.skill_add_name,
                ))
                .child(
                    div()
                        .id("skills-add-save")
                        .debug_selector(|| "skills-add-save".to_string())
                        .self_start()
                        .px(px(12.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::accent()))
                        .cursor_pointer()
                        .child(txt(tr(lang, "settings.skillsAddSave"), 12.0, theme::page()))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.add_source(cx);
                        })),
                ),
        )
        .into_any_element()
    }

    /// One labelled field of the add form.
    fn skills_field(
        &self,
        id: &'static str,
        label: String,
        input: &Entity<InputState>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(txt(label, 12.0, theme::muted()).w(px(80.0)))
            .child(
                div()
                    .id(SharedString::from(id))
                    .debug_selector(move || id.to_string())
                    .flex_1()
                    .min_w_0()
                    .h(px(28.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .child(Input::new(input).appearance(false)),
            )
            .into_any_element()
    }

    /// The tab strip the Extensions pane opens with.
    ///
    /// "Plugin list", "MCP" and "Skills" are REAL tabs: each sets the pane's
    /// tab, and the same pane draws that tab's body under the strip. "Plugin
    /// Market" is drawn DISABLED: this deployment ships no market, and a tab
    /// that looks pressable but does nothing is what the standing rule forbids.
    fn plugin_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let pane = section_index("plugins");
        let mut strip = div()
            .debug_selector(|| String::from("plugins-tabs"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(18.0))
            .mb(px(10.0))
            .border_b_1()
            .border_color(rgb(theme::border_soft()));
        for (tab, key, id) in [
            (0, "settings.pluginsTabs", "plugins-tab-list"),
            (1, "settings.pluginsTabMcp", "plugins-tab-mcp"),
            (2, "settings.pluginsTabSkills", "plugins-tab-skills"),
        ] {
            let selected = self.plugins_tab == tab;
            strip = strip.child(
                div()
                    .id(SharedString::from(id))
                    .debug_selector(move || id.to_string())
                    .pb(px(8.0))
                    .cursor_pointer()
                    .when(selected, |cell| {
                        cell.border_b_2().border_color(rgb(theme::text()))
                    })
                    .child(txt(
                        tr(lang, key),
                        13.0,
                        if selected {
                            theme::text()
                        } else {
                            theme::muted()
                        },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.plugins_tab = tab;
                        // A tab belongs to the pane, not to a section of its own:
                        // selecting it keeps the dialog on Extensions.
                        if let Some(index) = pane {
                            this.state.settings_section = index;
                        }
                        cx.notify();
                    })),
            );
        }
        strip
            .child(
                div()
                    .debug_selector(|| String::from("plugins-tab-market"))
                    .pb(px(8.0))
                    .child(txt(
                        tr(lang, "settings.pluginsTabMarket"),
                        13.0,
                        theme::faint(),
                    )),
            )
            .into_any_element()
    }

    /// One collapsible group header: a chevron, the title and a trailing control.
    fn plugin_group_header(
        &self,
        id: &'static str,
        title: String,
        trailing: AnyElement,
        folded: bool,
        session: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(id))
            .debug_selector(move || id.to_string())
            .h(px(32.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .child(icon(
                if folded {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                },
                14.0,
                theme::muted(),
            ))
            .child(
                txt(title, 14.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .flex_1(),
            )
            .child(trailing)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if session {
                    this.plugins_session_open = !this.plugins_session_open;
                } else {
                    this.plugins_global_open = !this.plugins_global_open;
                }
                cx.notify();
            }))
    }

    /// The Session plugins header's trailing control: the active preset.
    ///
    /// It writes the DRAFT, the same value the agent-presets pane writes, so Save
    /// is the one door either control's change goes through.
    fn preset_pill(&self, label: String, ids: Vec<String>, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let shell = cx.entity();
        Button::new("plugins-preset")
            .ghost()
            .compact()
            .h(px(28.0))
            .rounded(px(8.0))
            .bg(rgb(theme::new_session_fill()))
            .label(label)
            .icon(IconName::ChevronDown)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut menu = menu;
                for id in ids.iter() {
                    let pick = id.clone();
                    let text = if id.is_empty() {
                        tr(lang, "settings.presetDefault")
                    } else {
                        id.clone()
                    };
                    let pick_shell = shell.clone();
                    menu = menu.item(PopupMenuItem::new(text).on_click(
                        move |_event, _window, cx| {
                            let picked = pick.clone();
                            pick_shell.update(cx, |this, cx| {
                                this.state.select_preset(&picked);
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }

    /// Whether the DRAFT leaves a plugin installed.
    ///
    /// The pane reads this rather than the installed inventory, so a plugin
    /// switched off reads off here with no second state to keep in sync.
    pub(crate) fn plugin_enabled(&self, name: &str) -> bool {
        !self
            .state
            .draft()
            .workspace
            .disabled
            .extentions
            .iter()
            .any(|plugin| plugin == name)
    }

    /// One plugin card: an optional switch, the name, the description and a
    /// chevron - the reference's `.card` inside the pane's own grid.
    fn plugin_card(
        &self,
        id: String,
        name: String,
        description: String,
        enabled: Option<bool>,
        toggle: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = id.clone();
        let mut head = div().flex().flex_row().items_center().gap(px(8.0));
        if let Some(on) = enabled {
            head = head.child(
                div()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if on {
                        theme::accent()
                    } else {
                        theme::border_strong()
                    }))
                    .bg(rgb(if on { theme::accent() } else { theme::page() }))
                    .when(on, |boxed| {
                        boxed.child(icon(IconName::Check, 10.0, theme::page()))
                    }),
            );
        }
        head = head
            .child(txt(name, 13.0, theme::text()).flex_1().min_w_0())
            .child(icon(IconName::ChevronDown, 12.0, theme::faint()));
        let mut card = div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(10.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(theme::page()))
            .child(head)
            .child(
                txt(description, 12.0, theme::muted())
                    .line_height(px(17.0))
                    .max_h(px(38.0))
                    .overflow_hidden(),
            );
        if let Some(plugin) = toggle {
            card = card
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::hover())));
            card = card.on_click(cx.listener(move |this, _event, _window, cx| {
                this.state.toggle_plugin_off(&plugin);
                cx.notify();
            }));
        }
        card.into_any_element()
    }
    /// Load one entry into the MCP form, or blank it for a new one.
    fn mcp_fill_form(
        &mut self,
        server: Option<&McpServer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, command, args, url, http) = match server {
            Some(server) => {
                let (command, args, url, http) = match &server.transport {
                    McpTransport::Stdio { command, args } => {
                        (command.clone(), args.join(" "), String::new(), false)
                    }
                    McpTransport::Http { url } => (String::new(), String::new(), url.clone(), true),
                };
                (server.name.clone(), command, args, url, http)
            }
            None => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                false,
            ),
        };
        set(&self.mcp_name, name, window, cx);
        set(&self.mcp_command, command, window, cx);
        set(&self.mcp_args, args, window, cx);
        set(&self.mcp_url, url, window, cx);
        self.mcp_http = http;
    }

    /// Open the MCP form on a blank entry.
    pub(crate) fn mcp_begin_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mcp_fill_form(None, window, cx);
        self.mcp_form_open = true;
        self.mcp_editing = None;
        self.mcp_scope = McpScope::Global;
        self.mcp_refusal = None;
        cx.notify();
    }

    /// Open the MCP form on an existing entry.
    pub(crate) fn mcp_begin_edit(
        &mut self,
        scope: McpScope,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self
            .state
            .draft()
            .mcp_servers_of(scope)
            .into_iter()
            .find(|server| server.name.eq_ignore_ascii_case(name))
        else {
            return;
        };
        self.mcp_fill_form(Some(&server), window, cx);
        self.mcp_form_open = true;
        self.mcp_editing = Some((scope, server.name.clone()));
        self.mcp_scope = scope;
        self.mcp_refusal = None;
        cx.notify();
    }

    /// Save the form's entry into the draft, or say why it cannot.
    pub(crate) fn mcp_save(&mut self, cx: &mut Context<Self>) {
        let name = self.mcp_name.read(cx).value().trim().to_string();
        let transport = if self.mcp_http {
            McpTransport::Http {
                url: self.mcp_url.read(cx).value().trim().to_string(),
            }
        } else {
            McpTransport::Stdio {
                command: self.mcp_command.read(cx).value().trim().to_string(),
                args: self
                    .mcp_args
                    .read(cx)
                    .value()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
            }
        };
        // An edit keeps the row's enabled state: changing an address must not
        // silently reconnect a server the reader had parked.
        let enabled = match &self.mcp_editing {
            Some((scope, original)) => self
                .state
                .draft()
                .mcp_servers_of(*scope)
                .into_iter()
                .find(|server| server.name.eq_ignore_ascii_case(original))
                .map(|server| server.enabled)
                .unwrap_or(true),
            None => true,
        };
        let server = McpServer {
            name,
            transport,
            enabled,
            scope: self.mcp_scope,
        };
        let outcome = match &self.mcp_editing {
            Some((scope, original)) if *scope == self.mcp_scope => self
                .state
                .draft_mut()
                .update_mcp_server(*scope, original, server),
            Some((scope, original)) => {
                // Moving an entry between documents: validate and add in the
                // target first, so a refusal leaves the original where it was.
                let target = self.mcp_scope;
                let outcome = self.state.draft_mut().add_mcp_server(target, server);
                if outcome.is_ok() {
                    self.state.draft_mut().remove_mcp_server(*scope, original);
                }
                outcome
            }
            None => self
                .state
                .draft_mut()
                .add_mcp_server(self.mcp_scope, server),
        };
        match outcome {
            Ok(()) => {
                self.mcp_form_open = false;
                self.mcp_editing = None;
                self.mcp_refusal = None;
            }
            Err(refusal) => self.mcp_refusal = Some(refusal.key()),
        }
        cx.notify();
    }

    /// Close the MCP form without writing.
    pub(crate) fn mcp_cancel(&mut self, cx: &mut Context<Self>) {
        self.mcp_form_open = false;
        self.mcp_editing = None;
        self.mcp_refusal = None;
        cx.notify();
    }

    /// Drop one server from the document that owns it.
    pub(crate) fn mcp_remove(&mut self, scope: McpScope, name: &str, cx: &mut Context<Self>) {
        self.state.draft_mut().remove_mcp_server(scope, name);
        if self
            .mcp_editing
            .as_ref()
            .is_some_and(|(edited_scope, edited)| {
                *edited_scope == scope && edited.eq_ignore_ascii_case(name)
            })
        {
            self.mcp_form_open = false;
            self.mcp_editing = None;
        }
        cx.notify();
    }

    /// Connect or park one server.
    pub(crate) fn mcp_toggle(&mut self, scope: McpScope, name: &str, cx: &mut Context<Self>) {
        let Some(mut server) = self
            .state
            .draft()
            .mcp_servers_of(scope)
            .into_iter()
            .find(|server| server.name.eq_ignore_ascii_case(name))
        else {
            return;
        };
        server.enabled = !server.enabled;
        let outcome = self
            .state
            .draft_mut()
            .update_mcp_server(scope, name, server);
        self.mcp_refusal = outcome.err().map(|refusal| refusal.key());
        cx.notify();
    }

    /// One segmented transport/scope choice.
    fn mcp_segment(
        &self,
        id: &'static str,
        label: String,
        selected: bool,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Shell) + 'static,
    ) -> AnyElement {
        div()
            .id(SharedString::from(id))
            .debug_selector(move || id.to_string())
            .h(px(28.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .cursor_pointer()
            .rounded(px(6.0))
            .when(selected, |cell| cell.bg(rgb(theme::accent())))
            .when(!selected, |cell| cell.bg(rgb(theme::hover())))
            .child(txt(
                label,
                12.0,
                if selected {
                    theme::page()
                } else {
                    theme::text()
                },
            ))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                run(this);
                cx.notify();
            }))
            .into_any_element()
    }

    /// One MCP server card: identity, transport, the live connection state and
    /// the four actions the document supports.
    fn mcp_row(&self, index: usize, server: &McpServer, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let name = server.name.clone();
        let scope = server.scope;
        // A draft row whose configuration is not the committed one cannot be
        // connected yet, and saying so is what makes the dialog Save legible.
        let pending = !self.state.settings.mcp_servers().iter().any(|committed| {
            committed.name == server.name
                && committed.signature() == server.signature()
                && committed.enabled == server.enabled
        });
        let (status, ink) = if !server.enabled {
            (lang.tr("settings.mcpDisabled"), theme::faint())
        } else if pending {
            (lang.tr("settings.mcpSaveToConnect"), theme::warn())
        } else if let Some(error) = self.state.mcp.error(&server.name) {
            (
                format!("{} · {error}", lang.tr("settings.mcpFailed")),
                theme::error(),
            )
        } else if let Some(count) = self.state.mcp.tool_count(&server.name) {
            let key = if count == 1 {
                "settings.mcpConnectedOne"
            } else {
                "settings.mcpConnected"
            };
            (
                crate::i18n::tr_args(lang, key, &[("count", count.to_string())]),
                theme::success(),
            )
        } else {
            (lang.tr("settings.mcpNotConnected"), theme::faint())
        };
        // The exact names the model can call, not only a count: this is what
        // makes "it is connected" checkable against the next request.
        let tools = if server.enabled {
            self.state.mcp.tool_names(&server.name)
        } else {
            None
        };
        let edit = {
            let name = name.clone();
            div()
                .id(SharedString::from(format!("mcp-edit-{index}")))
                .debug_selector(move || format!("mcp-edit-{index}"))
                .cursor_pointer()
                .child(txt(lang.tr("settings.mcpEdit"), 12.0, theme::muted()))
                .on_click(cx.listener(move |this, _event, window, cx| {
                    this.mcp_begin_edit(scope, &name, window, cx);
                }))
        };
        let remove = {
            let name = name.clone();
            div()
                .id(SharedString::from(format!("mcp-remove-{index}")))
                .debug_selector(move || format!("mcp-remove-{index}"))
                .cursor_pointer()
                .child(txt(lang.tr("settings.mcpDelete"), 12.0, theme::muted()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mcp_remove(scope, &name, cx);
                }))
        };
        let toggle = {
            let name = name.clone();
            let key = if server.enabled {
                "settings.mcpDisable"
            } else {
                "settings.mcpEnable"
            };
            div()
                .id(SharedString::from(format!("mcp-toggle-{index}")))
                .debug_selector(move || format!("mcp-toggle-{index}"))
                .cursor_pointer()
                .child(txt(lang.tr(key), 12.0, theme::muted()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.mcp_toggle(scope, &name, cx);
                }))
        };
        let head = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                txt(name, 13.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .flex_1()
                    .min_w_0(),
            )
            .child(
                div()
                    .px(px(6.0))
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .rounded(px(4.0))
                    .bg(rgb(theme::hover()))
                    .child(txt(lang.tr(scope.label_key()), 10.0, theme::muted())),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .child(edit)
                    .child(remove)
                    .child(toggle),
            );
        let card = hairline_all(
            div()
                .debug_selector(move || format!("mcp-row-{index}"))
                .p(px(12.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border())),
        )
        .child(head)
        .child(
            txt(server.transport_summary(), 12.0, theme::muted())
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap(),
        )
        .child(
            txt(status, 12.0, ink)
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap(),
        );
        let mut card = card;
        if let Some(tools) = tools {
            if tools.is_empty() {
                card = card.child(txt(lang.tr("settings.mcpNoTools"), 11.0, theme::faint()));
            } else {
                card = card.child(txt(lang.tr("settings.mcpTools"), 11.0, theme::faint()));
                let mut grid = div().flex().flex_row().flex_wrap().gap(px(4.0));
                for tool in tools {
                    grid = grid.child(
                        div()
                            .h(px(18.0))
                            .px(px(6.0))
                            .flex()
                            .items_center()
                            .rounded(px(4.0))
                            .bg(rgb(theme::hover()))
                            .child(txt(tool, 11.0, theme::text()).font_family(theme::CODE_FONT)),
                    );
                }
                card = card.child(grid);
            }
        }
        card.into_any_element()
    }

    /// The form that adds or edits one server.
    fn mcp_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let transport = {
            let mut row = div().flex().flex_row().gap(px(4.0));
            row = row.child(self.mcp_segment(
                "mcp-transport-stdio",
                lang.tr("settings.mcpStdio"),
                !self.mcp_http,
                cx,
                |this| this.mcp_http = false,
            ));
            row = row.child(self.mcp_segment(
                "mcp-transport-http",
                lang.tr("settings.mcpHttp"),
                self.mcp_http,
                cx,
                |this| this.mcp_http = true,
            ));
            row.into_any_element()
        };
        let scope = {
            let mut row = div().flex().flex_row().gap(px(4.0));
            for (value, id, key) in [
                (
                    McpScope::Global,
                    "mcp-scope-global",
                    "settings.mcpScopeGlobal",
                ),
                (
                    McpScope::Workspace,
                    "mcp-scope-workspace",
                    "settings.mcpScopeWorkspace",
                ),
            ] {
                let selected = self.mcp_scope == value;
                row = row.child(
                    self.mcp_segment(id, lang.tr(key), selected, cx, move |this| {
                        this.mcp_scope = value;
                    }),
                );
            }
            row.into_any_element()
        };
        let mut fields = div().flex().flex_col().gap(px(12.0));
        fields = fields
            .child(self.editor_field(
                "settings.mcpName",
                self.editor_input(&self.mcp_name, "mcp-field-name".to_string()),
            ))
            .child(self.editor_field("settings.mcpTransport", transport));
        if self.mcp_http {
            fields = fields.child(self.editor_field(
                "settings.mcpUrl",
                self.editor_input(&self.mcp_url, "mcp-field-url".to_string()),
            ));
        } else {
            fields = fields
                .child(self.editor_field(
                    "settings.mcpCommand",
                    self.editor_input(&self.mcp_command, "mcp-field-command".to_string()),
                ))
                .child(self.editor_field(
                    "settings.mcpArgs",
                    self.editor_input(&self.mcp_args, "mcp-field-args".to_string()),
                ));
        }
        fields = fields.child(self.editor_field("settings.mcpScope", scope));
        let save = div()
            .id("mcp-save")
            .debug_selector(|| "mcp-save".to_string())
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .rounded(px(6.0))
            .bg(rgb(theme::accent()))
            .cursor_pointer()
            .child(txt(lang.tr("settings.mcpSave"), 12.0, theme::page()))
            .on_click(cx.listener(|this, _event, _window, cx| this.mcp_save(cx)));
        let cancel = div()
            .id("mcp-cancel")
            .debug_selector(|| "mcp-cancel".to_string())
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .rounded(px(6.0))
            .bg(rgb(theme::hover()))
            .cursor_pointer()
            .child(txt(lang.tr("settings.cancel"), 12.0, theme::text()))
            .on_click(cx.listener(|this, _event, _window, cx| this.mcp_cancel(cx)));
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(save)
            .child(cancel);
        let mut card = hairline_all(
            div()
                .debug_selector(|| "mcp-form".to_string())
                .p(px(12.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .bg(rgb(theme::page()))
                .border_color(rgb(theme::border())),
        )
        .child(fields)
        .child(actions);
        if let Some(key) = self.mcp_refusal {
            card = card.child(self.settings_note(key, true));
        }
        card.into_any_element()
    }

    /// The MCP tab's body: the servers, the add control and the open form.
    ///
    /// It is a tab of the Extensions pane, so it carries no page title of
    /// its own - the strip above it says which tab the reader is on.
    fn mcp_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let lang = self.state.lang;
        let mut rows: Vec<AnyElement> = Vec::new();
        let servers = self.state.draft().mcp_servers();
        if servers.is_empty() {
            rows.push(
                txt(lang.tr("settings.mcpEmpty"), 13.0, theme::muted())
                    .pb(px(1.0))
                    .into_any_element(),
            );
        }
        for (index, server) in servers.iter().enumerate() {
            rows.push(self.mcp_row(index, server, cx));
        }
        rows.push(
            hairline_all(
                div()
                    .id("mcp-add")
                    .debug_selector(|| "mcp-add".to_string())
                    .flex()
                    .flex_row()
                    .items_center()
                    .self_start()
                    .gap(px(4.0))
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded(px(4.0))
                    .bg(rgb(theme::page()))
                    .border_color(rgb(theme::border()))
                    .cursor_pointer()
                    .child(icon(IconName::Plus, 14.0, theme::text()))
                    .child(txt(lang.tr("settings.mcpAdd"), 12.0, theme::text()))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.mcp_begin_add(window, cx);
                    })),
            )
            .into_any_element(),
        );
        if self.mcp_form_open {
            rows.push(self.mcp_form(cx));
        }
        rows
    }

    /// The MCP view: the Extensions pane opened on its MCP tab.
    ///
    /// `--settings=mcp` and any older deep link still land here, and they land on
    /// the pane the tab belongs to rather than on a separate page.
    fn section_mcp(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        self.plugins_tab = 1;
        self.section_builtin_plugins(cx)
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
        let active = self.state.draft().global.general.active_workspace.clone();
        let workspaces: Vec<(String, String, String)> = self
            .state
            .draft()
            .global
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
                    this.state.draft_mut().global.general.active_workspace = pick.clone();
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
        (
            "settings.global.workspaces",
            "settings.workspacesHint",
            rows,
        )
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

    /// Vault: the admin-key store, locked or unlocked.
    ///
    /// The locked state explains what the file is and asks for the one secret
    /// that opens it; a create asks twice, because a vault sealed under a typo
    /// has no recovery. The unlocked state lists names and dates, reveals one
    /// value inline on press, deletes with a confirmation and adds a row.
    fn section_vault(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (&'static str, &'static str, Vec<AnyElement>) {
        let lang = self.state.lang;
        let exists = self.state.vault.exists();
        let unlocked = self.state.vault.is_unlocked();
        let app_access = self.state.vault.is_app_available();
        let mut rows: Vec<AnyElement> = Vec::new();
        if unlocked {
            rows.extend(self.vault_unlocked_rows(cx));
        } else {
            rows.push(self.settings_note("settings.vaultExplain", false));
            if self.vault_busy {
                rows.push(self.settings_note("settings.vaultBusy", false));
            } else if exists {
                rows.push(self.editor_field(
                    "settings.vaultPassphrase",
                    self.editor_input(
                        &self.vault_inputs.passphrase,
                        "vault-passphrase".to_string(),
                    ),
                ));
                rows.push(self.vault_button(
                    "vault-unlock",
                    "settings.vaultUnlock",
                    true,
                    Shell::vault_unlock,
                    cx,
                ));
                rows.push(self.settings_note(
                    if app_access {
                        "settings.vaultAdminLockedHint"
                    } else {
                        "settings.vaultLockedHint"
                    },
                    false,
                ));
            } else {
                rows.push(self.editor_field(
                    "settings.vaultPassphrase",
                    self.editor_input(
                        &self.vault_inputs.passphrase,
                        "vault-passphrase".to_string(),
                    ),
                ));
                rows.push(self.editor_field(
                    "settings.vaultConfirm",
                    self.editor_input(&self.vault_inputs.confirm, "vault-confirm".to_string()),
                ));
                rows.push(self.vault_strength_note(cx));
                rows.push(self.vault_button(
                    "vault-create",
                    "settings.vaultCreate",
                    true,
                    Shell::vault_create,
                    cx,
                ));
                rows.push(self.settings_note("settings.vaultCreateHint", true));
            }
        }
        // The refusal leads the pane: a wrong passphrase or an unreadable file
        // is the first thing the reader must see, not a line past the list.
        if let Some(key) = self.vault_error {
            rows.insert(
                0,
                div()
                    .debug_selector(|| "vault-error".to_string())
                    .pb(px(10.0))
                    .child(txt(tr(lang, key), 12.0, theme::error()))
                    .into_any_element(),
            );
        }
        ("settings.vault", "settings.vaultHint", rows)
    }

    /// The strength note under a create's two passphrase fields.
    ///
    /// It is a note, not a gate: the only hard rule is the confirmation match,
    /// because refusing a passphrase the reader chose is a policy this shell is
    /// not entitled to make. The note still tells them what a good one is.
    fn vault_strength_note(&self, cx: &mut Context<Self>) -> AnyElement {
        let passphrase = self.vault_inputs.passphrase.read(cx).value().to_string();
        let length = passphrase.chars().count();
        let (key, colour) = if length < 12 {
            ("settings.vaultStrengthShort", theme::warn())
        } else if length < 20 {
            ("settings.vaultStrengthFair", theme::muted())
        } else {
            ("settings.vaultStrengthStrong", theme::success())
        };
        txt(tr(self.state.lang, key), 12.0, colour).into_any_element()
    }

    /// A masked control button of the Vault pane.
    fn vault_button(
        &self,
        id: &'static str,
        label: &'static str,
        primary: bool,
        action: fn(&mut Shell, &mut Window, &mut Context<Shell>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .h(px(32.0))
            .px(px(14.0))
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
            .hover(|style| style.opacity(0.9))
            .child(txt(
                tr(self.state.lang, label),
                13.0,
                if primary {
                    theme::page()
                } else {
                    theme::text()
                },
            ))
            .on_click(cx.listener(move |this, _event, window, cx| action(this, window, cx)))
            .into_any_element()
    }

    /// The unlocked state's status line and its Lock now control.
    fn vault_status_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self
            .state
            .vault
            .entries()
            .map(|rows| rows.len())
            .unwrap_or(0);
        hairline_bottom(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .pb(px(10.0))
                .border_color(rgb(theme::border_soft()))
                .child(icon(IconName::Lock, 14.0, theme::success()))
                .child(txt(
                    tr(self.state.lang, "settings.vaultUnlocked"),
                    13.0,
                    theme::text(),
                ))
                .child(txt(
                    tr_args(
                        self.state.lang,
                        "settings.vaultEntryCount",
                        &[("count", count.to_string())],
                    ),
                    12.0,
                    theme::faint(),
                ))
                .child(div().flex_1())
                .child(self.vault_button(
                    "vault-lock",
                    "settings.vaultLock",
                    false,
                    Shell::vault_lock,
                    cx,
                )),
        )
        .into_any_element()
    }

    /// The unlocked state: the entry list and the add row.
    fn vault_unlocked_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let lang = self.state.lang;
        let entries = self.state.vault.entries().unwrap_or_default();
        let mut rows: Vec<AnyElement> = Vec::with_capacity(entries.len() + 3);
        rows.push(self.vault_status_row(cx));
        rows.push(
            div()
                .pt(px(14.0))
                .pb(px(4.0))
                .child(txt(
                    tr_args(
                        lang,
                        "settings.vaultEntries",
                        &[("count", entries.len().to_string())],
                    ),
                    12.0,
                    theme::muted(),
                ))
                .into_any_element(),
        );
        if entries.is_empty() {
            rows.push(self.settings_note("settings.vaultEmpty", false));
        }
        for entry in entries {
            rows.push(self.vault_entry_row(&entry.name, entry.created, cx));
        }
        rows.push(self.vault_add_block(cx));
        rows
    }

    /// One entry: its name and date, a masked or revealed value, and its
    /// controls.
    ///
    /// The value is resolved only for the row the reader revealed, so an
    /// ordinary repaint does not copy every secret out of the vault.
    fn vault_entry_row(&self, name: &str, created: u64, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let revealed = self.vault_revealed.as_deref() == Some(name);
        let owned = name.to_string();
        let owned_copy = name.to_string();
        let owned_delete = name.to_string();
        let reveal_label = if revealed {
            "settings.vaultHide"
        } else {
            "settings.vaultReveal"
        };
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .id(SharedString::from(format!("vault-reveal-{name}")))
                    .debug_selector({
                        let selector = format!("vault-reveal-{name}");
                        move || selector.clone()
                    })
                    .cursor_pointer()
                    .child(txt(tr(lang, reveal_label), 12.0, theme::muted()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.vault_revealed = if this.vault_revealed.as_deref() == Some(&owned) {
                            None
                        } else {
                            Some(owned.clone())
                        };
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("vault-copy-{name}")))
                    .debug_selector({
                        let selector = format!("vault-copy-{name}");
                        move || selector.clone()
                    })
                    .cursor_pointer()
                    .child(txt(tr(lang, "settings.vaultCopy"), 12.0, theme::muted()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.vault_copy(&owned_copy, cx);
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("vault-delete-{name}")))
                    .debug_selector({
                        let selector = format!("vault-delete-{name}");
                        move || selector.clone()
                    })
                    .cursor_pointer()
                    .child(txt(tr(lang, "settings.vaultDelete"), 12.0, theme::error()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.vault_confirm_delete = Some(owned_delete.clone());
                        cx.notify();
                    })),
            );
        let head = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(
                txt(name.to_string(), 13.0, theme::text())
                    .font_family("monospace")
                    .flex_1()
                    .min_w_0(),
            )
            .child(txt(
                tr_args(
                    lang,
                    "settings.vaultCreated",
                    &[("when", vault_stamp(created))],
                ),
                11.0,
                theme::faint(),
            ))
            .child(controls);
        let mut row = div().flex().flex_col().gap(px(8.0)).py(px(10.0));
        if revealed {
            let value = self
                .state
                .vault
                .resolve(name)
                .map(|secret| secret.as_str().to_string())
                .unwrap_or_default();
            row = row.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .bg(rgb(theme::page()))
                            .child(
                                txt(value, 12.0, theme::text())
                                    .font_family("monospace")
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis(),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("vault-reveal-copy-{name}")))
                            .debug_selector({
                                let selector = format!("vault-reveal-copy-{name}");
                                move || selector.clone()
                            })
                            .cursor_pointer()
                            .child(icon(IconName::Copy, 14.0, theme::muted()))
                            .on_click(cx.listener({
                                let copied = name.to_string();
                                move |this, _event, _window, cx| {
                                    this.vault_copy(&copied, cx);
                                }
                            })),
                    ),
            );
        } else {
            row = row.child(txt("••••••••", 12.0, theme::faint()).font_family("monospace"));
        }
        hairline_bottom(row.border_color(rgb(theme::border_soft())).child(head)).into_any_element()
    }

    /// The add-entry block under the list.
    fn vault_add_block(&self, cx: &mut Context<Self>) -> AnyElement {
        hairline_top(
            div()
                .mt(px(6.0))
                .pt(px(14.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .border_color(rgb(theme::border_soft()))
                .child(
                    txt(
                        tr(self.state.lang, "settings.vaultAddTitle"),
                        13.0,
                        theme::text(),
                    )
                    .font_weight(FontWeight::MEDIUM),
                )
                .child(self.editor_field(
                    "settings.vaultName",
                    self.editor_input(&self.vault_inputs.name, "vault-entry-name".to_string()),
                ))
                .child(self.editor_field(
                    "settings.vaultValue",
                    self.editor_input(&self.vault_inputs.value, "vault-entry-value".to_string()),
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(self.vault_button(
                            "vault-add",
                            "settings.vaultAdd",
                            true,
                            Shell::vault_add,
                            cx,
                        ))
                        .child(txt(
                            tr(self.state.lang, "settings.vaultNameHint"),
                            12.0,
                            theme::faint(),
                        )),
                ),
        )
        .into_any_element()
    }

    /// Create the vault from the two passphrase fields.
    fn vault_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vault_busy {
            return;
        }
        let passphrase = zeroize::Zeroizing::new(
            self.vault_inputs
                .passphrase
                .read(cx)
                .value()
                .trim()
                .to_string(),
        );
        let confirm = zeroize::Zeroizing::new(
            self.vault_inputs
                .confirm
                .read(cx)
                .value()
                .trim()
                .to_string(),
        );
        if passphrase.is_empty() {
            self.vault_error = Some("settings.vaultPassphraseRequired");
            cx.notify();
            return;
        }
        if *passphrase != *confirm {
            self.vault_error = Some("settings.vaultMismatch");
            cx.notify();
            return;
        }
        self.run_vault_op(move |vault| vault.create(&passphrase), window, cx);
    }

    /// Unlock the vault with the passphrase field.
    fn vault_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vault_busy {
            return;
        }
        let passphrase = zeroize::Zeroizing::new(
            self.vault_inputs
                .passphrase
                .read(cx)
                .value()
                .trim()
                .to_string(),
        );
        if passphrase.is_empty() {
            self.vault_error = Some("settings.vaultPassphraseRequired");
            cx.notify();
            return;
        }
        self.run_vault_op(move |vault| vault.unlock(&passphrase), window, cx);
    }

    /// Drop the derived key and every decrypted value.
    fn vault_lock(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.vault.is_unlocked() {
            return;
        }
        self.state.vault.lock();
        self.vault_revealed = None;
        self.vault_error = None;
        self.state
            .notice(tr(self.state.lang, "settings.vaultLockedNotice").to_string());
        cx.notify();
    }

    /// Write the add row's entry to the vault.
    fn vault_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.vault_inputs.name.read(cx).value().trim().to_string();
        let value = zeroize::Zeroizing::new(self.vault_inputs.value.read(cx).value().to_string());
        if name.is_empty() || value.is_empty() {
            self.vault_error = Some("settings.vaultEntryRequired");
            cx.notify();
            return;
        }
        match self.state.vault.put(&name, &value) {
            Ok(()) => {
                self.vault_error = None;
                self.vault_inputs
                    .name
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.vault_inputs
                    .value
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.state
                    .notice(tr(self.state.lang, "settings.vaultAdded").to_string());
            }
            Err(error) => {
                self.vault_error = Some(error.locale_key());
            }
        }
        cx.notify();
    }

    /// Copy one entry's value to the clipboard.
    ///
    /// get, not resolve, so a locked or absent entry says why instead of the
    /// copied notice over an empty pasteboard.
    fn vault_copy(&mut self, name: &str, cx: &mut Context<Self>) {
        match self.state.vault.get(name) {
            Ok(secret) => {
                cx.write_to_clipboard(ClipboardItem::new_string(secret.as_str().to_string()));
                self.state
                    .notice(tr(self.state.lang, "settings.vaultCopied").to_string());
                self.vault_error = None;
            }
            Err(error) => {
                self.vault_error = Some(error.locale_key());
                let why = tr(self.state.lang, error.locale_key());
                self.state.notice(why);
            }
        }
        cx.notify();
    }

    /// Remove the entry the confirmation names.
    fn vault_delete(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.vault_confirm_delete.clone() else {
            return;
        };
        self.vault_confirm_delete = None;
        match self.state.vault.remove(&name) {
            Ok(removed) => {
                if self.vault_revealed.as_deref() == Some(&name) {
                    self.vault_revealed = None;
                }
                self.vault_error = None;
                if removed {
                    self.state
                        .notice(tr(self.state.lang, "settings.vaultRemoved").to_string());
                }
            }
            Err(error) => self.vault_error = Some(error.locale_key()),
        }
        cx.notify();
    }

    /// Run a key derivation off the view thread.
    ///
    /// Argon2 at 64 MiB / 3 passes is deliberately expensive, so an unlock or a
    /// create moves the vault to the shell's runtime, derives there, and hands
    /// it back. The pane says why it is waiting rather than freezing.
    fn run_vault_op(
        &mut self,
        op: impl FnOnce(&mut crate::vault::Vault) -> Result<(), crate::vault::VaultError>
            + Send
            + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut vault = std::mem::replace(&mut self.state.vault, crate::vault::Vault::disabled());
        let (tx, rx) = async_channel::bounded(1);
        self.state.runner.rt().spawn(async move {
            let outcome = op(&mut vault);
            let _ = tx.send((vault, outcome)).await;
        });
        self.vault_busy = true;
        self.vault_error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = rx.recv().await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.vault_busy = false;
                match outcome {
                    Ok((vault, Ok(()))) => {
                        this.state.vault = vault;
                        this.vault_error = None;
                        for input in [&this.vault_inputs.passphrase, &this.vault_inputs.confirm] {
                            input.update(cx, |state, cx| state.set_value("", window, cx));
                        }
                    }
                    Ok((vault, Err(error))) => {
                        this.state.vault = vault;
                        this.vault_error = Some(error.locale_key());
                    }
                    // The task went away without answering; the vault is locked
                    // rather than half-open, and the pane can try again.
                    Err(_) => this.vault_error = Some("settings.vaultIo"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Choose the vault entry a provider reads its API key from.
    ///
    /// The id goes into the DRAFT and never the value; the settings tree holds
    /// only the id, and the secret stays in the vault.
    pub(crate) fn choose_provider_vault_key(
        &mut self,
        id: &str,
        entry: Option<&str>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.set_provider_api_key_vault(id, entry);
        cx.notify();
    }

    /// Choose the vault entry the add card will land on its new provider.
    ///
    /// The provider does not exist until Create, so the choice is pending on
    /// the shell rather than written to the draft. Clear drops it again.
    pub(crate) fn choose_add_vault_key(
        &mut self,
        entry: Option<&str>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_key_vault = entry
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        cx.notify();
    }

    /// The vault delete confirmation, drawn over the settings panel.
    pub(crate) fn render_vault_delete(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let name = self.vault_confirm_delete.clone().unwrap_or_default();
        let confirm = div()
            .id("vault-delete-confirm")
            .debug_selector(|| String::from("vault-delete-confirm"))
            .px(px(16.0))
            .h(px(30.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(rgb(theme::accent()))
            .hover(|style| style.opacity(0.9))
            .child(txt(
                tr(lang, "settings.vaultDeleteConfirm"),
                13.0,
                theme::page(),
            ))
            .on_click(cx.listener(|this, _event, window, cx| {
                this.vault_delete(window, cx);
            }));
        let cancel = div()
            .id("vault-delete-cancel")
            .debug_selector(|| String::from("vault-delete-cancel"))
            .px(px(16.0))
            .h(px(30.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(theme::border()))
            .hover(|style| style.bg(rgb(theme::hover())))
            .child(txt(tr(lang, "settings.cancel"), 13.0, theme::text()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.vault_confirm_delete = None;
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
                    .debug_selector(|| String::from("vault-delete-dialog"))
                    .w(px(460.0))
                    .p(px(18.0))
                    .rounded(px(12.0))
                    .bg(rgb(theme::page()))
                    .border_1()
                    .border_color(rgb(theme::border_strong()))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(txt(
                        crate::i18n::tr_with(lang, "settings.vaultDeleteTitle", &name),
                        15.0,
                        theme::text(),
                    ))
                    .child(txt(
                        tr(lang, "settings.vaultDeleteDescription"),
                        12.5,
                        theme::muted(),
                    ))
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
}

/// Format a unix timestamp as 'YYYY-MM-DD HH:MM UTC'.
///
/// The vault stores plain unix seconds and the pane shows a readable date; the
/// civil-date arithmetic is Howard Hinnant's days-from-civil inverse, so one
/// label does not pull a calendar crate into the shell.
fn vault_stamp(created: u64) -> String {
    let seconds = created as i64;
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        of_day / 3_600,
        (of_day % 3_600) / 60
    )
}

/// The calendar date a count of days since 1970-01-01 names.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
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

    /// A stored unix timestamp reads as the civil date the row shows.
    #[test]
    fn a_vault_timestamp_reads_as_a_date() {
        assert_eq!(super::vault_stamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(super::vault_stamp(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(super::vault_stamp(951_782_400), "2000-02-29 00:00 UTC");
    }
}

/// The model and context labels are the strings measured off the reference own
/// models package, in both locales; the Chinese keeps the protocol names.
#[cfg(test)]
mod label_tests {
    fn tr(locale: &str, key: &'static str) -> String {
        crate::i18n::tr(crate::i18n::Lang::from_locale(locale), key)
    }

    /// A provider is named by its own name, its shipped family, or its id — and
    /// never by its endpoint.
    ///
    /// `built_in` answers `(name, base URL, protocol)`; two call sites read the
    /// second element, so the Models card showed
    /// `https://integrate.api.nvidia.com/v1` where the provider's name belongs.
    #[test]
    fn a_provider_is_named_by_its_name_not_its_endpoint() {
        use crate::ui::settings::provider_label;
        assert_eq!(
            provider_label("nvidia", None),
            "NVIDIA",
            "a shipped family is named by the catalog's own label"
        );
        assert_eq!(
            provider_label("nvidia", Some("My Relay")),
            "My Relay",
            "the name the tree carries wins"
        );
        assert_eq!(
            provider_label("nvidia", Some("   ")),
            "NVIDIA",
            "a blank name is absence, not a name"
        );
        assert_eq!(
            provider_label("my-relay", None),
            "my-relay",
            "an unknown id names itself"
        );
        for id in ["deepseek", "openai", "anthropic", "nvidia", "openrouter"] {
            let label = provider_label(id, None);
            assert!(
                !label.contains("://"),
                "{id} is named by a name, not by a URL (got {label})"
            );
        }
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

    /// The source control's label names the chosen vault entry, and is the
    /// affordance when nothing is chosen.
    #[test]
    fn the_vault_source_label_names_the_entry() {
        use crate::i18n::Lang;
        use crate::ui::settings::vault_source_label;
        assert_eq!(
            vault_source_label(Lang::En, Some("keys/work")),
            "vault: keys/work"
        );
        assert_eq!(vault_source_label(Lang::En, None), "Choose from vault");
        let zh = vault_source_label(Lang::Zh, Some("keys/work"));
        assert!(zh.contains("keys/work"), "the id survives translation");
        assert_ne!(zh, "Choose from vault", "the value is translated");
    }
}
