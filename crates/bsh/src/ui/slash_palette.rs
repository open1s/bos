//! The composer's slash menu, mirrored from the reference's input-trigger menu.
//!
//! The reference registers TWO sources on the slash trigger: the command source
//! (whose rows carry an Add or Commands section) and the skill source (whose
//! rows form the Skills group). Its MenuView.module.css is the specification
//! this file follows, measured from dsh-client-ui-input-trigger:
//!
//! - card: padding 4, bottom calc(100% + 4px), max-height 400, no border,
//!   prominent elevation, overflow hidden;
//! - row: min-height 34, radius md, gap 6, padding 6/8, 13px/20px primary;
//!   a 14px glyph, the name ellipsised at 40%, the description flex-1,
//!   right-aligned, 12px/18px tertiary;
//! - section title: 11px/16px weight 500, tertiary, min-height 23,
//!   padding 5/8/2, 3px above a later section;
//! - group title: 11px/15px tertiary, padding 6/8.
//!
//! The rows are the reference's own catalog: Add = file, goal, plan, feedback;
//! Commands = compact, permission, model, export. The skill source supplies the
//! Skills group, and a skill whose manifest restricts it to user invocation
//! carries the user-only marker as a DESCRIPTION PREFIX, exactly the way the
//! reference's skill source does. Two commands open a child option list in the
//! same card (the permission presets and the model catalogue), which is what
//! makes switch and select real choices instead of a cycle.
//!
//! Every row runs a handler this build backs: the Export row downloads the
//! Session log as a ZIP archive, the same contract as the reference's row.
//!
//! The item list is a VIEW MODEL with two caches, because a keystroke must be
//! O(keystroke), not O(catalogue). The catalogue's rows and their lowercased
//! haystacks are built once per settings generation and language; a query then
//! returns row INDICES into them. A frame reads what the last query change
//! computed rather than walking or copying the catalogue on every paint.

use std::time::Duration;

use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

use super::shell::Shell;
use super::{icon, txt, DivExt, IconName};
use crate::i18n::Lang;
use crate::theme;

/// The menu card's own padding, from .tki__G_menu.
pub(crate) const PALETTE_PAD: f32 = 4.0;
/// The gap between the card's bottom edge and the composer card's top.
pub(crate) const PALETTE_GAP: f32 = 4.0;
/// .tki__G_menu max-height.
pub(crate) const PALETTE_MAX_H: f32 = 400.0;
/// A section title: .tki__G_sectionTitle.
pub(crate) const SECTION_MIN_H: f32 = 23.0;
pub(crate) const SECTION_SIZE: f32 = 11.0;
pub(crate) const SECTION_LINE: f32 = 16.0;
pub(crate) const SECTION_PAD_X: f32 = 8.0;
pub(crate) const SECTION_PAD_TOP: f32 = 5.0;
pub(crate) const SECTION_PAD_BOTTOM: f32 = 2.0;
pub(crate) const SECTION_GAP: f32 = 3.0;
/// A group title (the Skills source): .tki__G_groupTitle.
pub(crate) const GROUP_SIZE: f32 = 11.0;
pub(crate) const GROUP_LINE: f32 = 15.0;
pub(crate) const GROUP_PAD_X: f32 = 8.0;
pub(crate) const GROUP_PAD_Y: f32 = 6.0;
/// A row: .tki__G_item.
pub(crate) const ROW_MIN_H: f32 = 34.0;
pub(crate) const ROW_PAD_X: f32 = 8.0;
pub(crate) const ROW_PAD_Y: f32 = 6.0;
pub(crate) const ROW_GAP: f32 = 6.0;
pub(crate) const ROW_RADIUS: f32 = 12.0;
pub(crate) const LABEL_SIZE: f32 = 13.0;
pub(crate) const LABEL_LINE: f32 = 20.0;
/// .tki__G_itemIcon: 14px, the menu-icon token.
pub(crate) const ROW_ICON: f32 = 14.0;
/// .tki__G_itemName max-width: 40%.
pub(crate) const NAME_MAX_SHARE: f32 = 0.4;
/// .tki__G_itemDescription: 12px/18px, tertiary, right-aligned.
pub(crate) const DESC_SIZE: f32 = 12.0;
pub(crate) const DESC_LINE: f32 = 18.0;
const EMPTY_PAD: f32 = 10.0;
const EMPTY_SIZE: f32 = 12.0;

/// Which list the card shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SlashKind {
    /// The Add section (file, goal, plan, feedback).
    Add,
    /// The Commands section (compact, permission, model, export).
    Command,
    /// The Skills group.
    Skill,
    /// A row of a child option list (a preset, a model).
    Choice,
}

impl SlashKind {
    /// The header's locale key (the choice panes carry their own title).
    fn title_key(self) -> &'static str {
        match self {
            SlashKind::Add => "slash.add",
            SlashKind::Command => "slash.commands",
            SlashKind::Skill => "slash.skills",
            SlashKind::Choice => "slash.commands",
        }
    }

    /// The header's debug-selector suffix.
    fn id(self) -> &'static str {
        match self {
            SlashKind::Add => "add",
            SlashKind::Command => "commands",
            SlashKind::Skill => "skills",
            SlashKind::Choice => "choice",
        }
    }
}

/// One row of the menu: the reference's label/name/description/icon candidate
/// plus this build's section and action.
pub(crate) struct SlashItem {
    /// Stable id, unique across the card.
    pub(crate) id: String,
    /// The section the row belongs to.
    pub(crate) kind: SlashKind,
    /// The leading glyph.
    pub(crate) icon: IconName,
    /// The row's label, as drawn.
    pub(crate) label: String,
    /// The one-line description, as drawn.
    pub(crate) description: String,
    /// Whether the skill's manifest restricts it to user invocation.
    pub(crate) user_only: bool,
    /// What choosing the row does.
    pub(crate) action: SlashAction,
}

/// What choosing a row does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum SlashAction {
    /// Run a command this build backs.
    Command(SlashCommand),
    /// Put the skill's name in the prompt, after its slash.
    ///
    /// The load itself belongs to the SEND: the runner's turn task resolves
    /// the gesture against the skills that build registered and appends the
    /// skill's instructions, so a click never reads a file and a name the
    /// settings switched off loads nothing. See
    /// [`crate::skill_invocation`].
    Insert(String),
    /// Switch to one permission preset.
    Permission(&'static str),
    /// Select a model for this conversation.
    SelectModel {
        /// The provider node the model hangs under.
        provider: String,
        /// The model nickname in that provider.
        model: String,
    },
}

/// One section or group of the card.
///
/// `items` are INDICES into the palette's active row list (the root catalogue,
/// or a child list's own rows), not copies of them: a query change re-derives
/// these indices without re-allocating a single catalogue entry.
pub(crate) struct SlashGroup {
    /// Which section this is.
    pub(crate) kind: SlashKind,
    /// The translated header text.
    pub(crate) title: String,
    /// The rows this query matched, as indices into the active row list.
    pub(crate) items: Vec<usize>,
}

/// A skill the card can offer, with the catalogue facts the row needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct SlashSkill {
    /// The skill's own name, from its manifest.
    pub(crate) name: String,
    /// Its one-line description.
    pub(crate) description: String,
    /// Whether its manifest restricts it to user invocation.
    pub(crate) user_only: bool,
}

/// What running a command did to the card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SlashEffect {
    /// The command acted; the card closes and the trigger is consumed.
    Done,
    /// The command opened a child list; the card stays and the draft is kept.
    Chooser,
    /// The command replaced the draft with a surface of its own (the File row
    /// opens the `@` picker), so the card closes but the text is kept.
    Kept,
}

/// The commands this build backs, in the reference catalog's order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SlashCommand {
    /// Attach a file to the next turn.
    File,
    /// Set or view the session goal.
    Goal,
    /// Enter or leave plan mode.
    Plan,
    /// Record feedback about the session.
    Feedback,
    /// Compact older conversation history.
    Compact,
    /// Switch the permission preset.
    Permission,
    /// Select the conversation model.
    Model,
    /// Download the Session log as a ZIP archive.
    Export,
}

impl SlashCommand {
    /// The Add section's rows, in the reference's SECTION_ROWS order.
    pub(crate) fn adds() -> [SlashCommand; 4] {
        [
            SlashCommand::File,
            SlashCommand::Goal,
            SlashCommand::Plan,
            SlashCommand::Feedback,
        ]
    }

    /// The Commands section's rows, in the reference's SECTION_ROWS order.
    pub(crate) fn commands() -> [SlashCommand; 4] {
        [
            SlashCommand::Compact,
            SlashCommand::Permission,
            SlashCommand::Model,
            SlashCommand::Export,
        ]
    }

    /// Every command the card can draw, for the table test.
    #[cfg(test)]
    pub(crate) fn all() -> [SlashCommand; 8] {
        [
            SlashCommand::File,
            SlashCommand::Goal,
            SlashCommand::Plan,
            SlashCommand::Feedback,
            SlashCommand::Compact,
            SlashCommand::Permission,
            SlashCommand::Model,
            SlashCommand::Export,
        ]
    }

    /// The row id and debug-selector suffix (the reference's command name).
    pub(crate) fn id(self) -> &'static str {
        match self {
            SlashCommand::File => "file",
            SlashCommand::Goal => "goal",
            SlashCommand::Plan => "plan",
            SlashCommand::Feedback => "feedback",
            SlashCommand::Compact => "compact",
            SlashCommand::Permission => "permission",
            SlashCommand::Model => "model",
            SlashCommand::Export => "export",
        }
    }

    /// The row's leading glyph, the reference's own icon per command.
    fn icon(self) -> IconName {
        match self {
            SlashCommand::File => IconName::Paperclip,
            SlashCommand::Goal => IconName::Target,
            SlashCommand::Plan => IconName::ClipboardList,
            SlashCommand::Feedback => IconName::Send,
            SlashCommand::Compact => IconName::CircleDashed,
            SlashCommand::Permission => IconName::ShieldAlert,
            SlashCommand::Model => IconName::Database,
            SlashCommand::Export => IconName::Download,
        }
    }

    /// The row label's locale key.
    fn label_key(self) -> &'static str {
        match self {
            SlashCommand::File => "slash.file",
            SlashCommand::Goal => "slash.goal",
            SlashCommand::Plan => "slash.plan",
            SlashCommand::Feedback => "slash.feedback",
            SlashCommand::Compact => "slash.compact",
            SlashCommand::Permission => "slash.permission",
            SlashCommand::Model => "slash.model",
            SlashCommand::Export => "slash.export",
        }
    }

    /// The row description's locale key; the File command has none in the
    /// reference's dictionary, so it draws no description.
    fn description_key(self) -> Option<&'static str> {
        match self {
            SlashCommand::File => None,
            SlashCommand::Goal => Some("slash.goalDetail"),
            SlashCommand::Plan => Some("slash.planDetail"),
            SlashCommand::Feedback => Some("slash.feedbackDetail"),
            SlashCommand::Compact => Some("slash.compactDetail"),
            SlashCommand::Permission => Some("slash.permissionDetail"),
            SlashCommand::Model => Some("slash.modelDetail"),
            SlashCommand::Export => Some("slash.exportDetail"),
        }
    }

    /// Run the command's real handler.
    pub(crate) fn run(
        self,
        shell: &mut Shell,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) -> SlashEffect {
        match self {
            SlashCommand::File => {
                // File LINKS a workspace file (or a line range) instead of
                // attaching a copy: one picker and one grammar own every
                // reference, and the model is told to read what it points at.
                shell.begin_file_reference(window, cx);
                SlashEffect::Kept
            }
            SlashCommand::Goal => {
                // The reference's /goal command CLAIMS the composer: the row
                // leaves `/goal ` in the draft with the command's hint as the
                // placeholder, and Enter runs the grammar (objective, edit,
                // pause, resume, clear). Direct output stays in the UI.
                shell.begin_command_claim("/goal", window, cx);
                SlashEffect::Kept
            }
            SlashCommand::Plan => {
                // Same claim shape, with the reference's [off|message] hint:
                // bare /plan enters, `off` leaves, anything else is the task.
                shell.begin_command_claim("/plan", window, cx);
                SlashEffect::Kept
            }
            SlashCommand::Feedback => {
                shell.open_note(NoteKind::Feedback, window, cx);
                SlashEffect::Done
            }
            SlashCommand::Compact => {
                shell.compact_history(cx);
                SlashEffect::Done
            }
            SlashCommand::Permission => {
                // The row marked current is the mode the tools obey, not a
                // request a preset already narrowed: the chip and this picker
                // answer the same question the same way.
                let current = shell.state.effective().permission.effective;
                let lang = shell.state.lang;
                shell.slash.show_permissions(&current, lang);
                SlashEffect::Chooser
            }
            SlashCommand::Model => {
                let (groups, lang) = {
                    let state = &shell.state;
                    let active = state.effective().provider;
                    (state.draft().catalogs_by_provider(&active), state.lang)
                };
                shell.slash.show_models(groups, lang);
                SlashEffect::Chooser
            }
            SlashCommand::Export => {
                shell.export_session(window, cx);
                SlashEffect::Done
            }
        }
    }
}

/// The leading-slash trigger: a slash as the FIRST character, with everything
/// after it as the query.
///
/// The reference's trigger is a draft token; for this build the token has to be
/// the whole draft (the menu opens over the composer) and the same whitespace
/// rule applies: a space ends the token, which closes the menu and leaves the
/// text.
pub(crate) fn slash_palette_query(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    if rest.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// The discovered skills the card offers, name-ordered, enabled only.
pub(crate) fn slash_skills(state: &crate::state::AppState) -> Vec<SlashSkill> {
    let mut skills: Vec<SlashSkill> = state
        .effective()
        .skills
        .iter()
        .filter(|skill| skill.enabled)
        .map(|skill| SlashSkill {
            name: skill.name.clone(),
            description: skill.description.clone(),
            user_only: skill.user_only,
        })
        .collect();
    skills.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    skills
}

/// The root card's rows, built once per settings generation and language.
///
/// The rows and their lowercased haystacks are the expensive part of the list
/// (a 120-skill install is 120 owned labels and descriptions), and they are
/// built when the catalogue MOVES, not when the query does. A query then walks
/// the cached rows and returns indices, so a keystroke costs its needle plus
/// one `usize` per match: O(keystroke), not O(catalogue).
#[derive(Default)]
pub(crate) struct SlashCatalogue {
    /// Every row, in draw order: the Add section, the Commands section, then
    /// the skills by name.
    rows: Vec<SlashItem>,
    /// The lowercased (label, description) per row, the substring haystack.
    lowered: Vec<(String, String)>,
}

impl SlashCatalogue {
    /// Build every row these settings and this language describe.
    pub(crate) fn build(skills: &[SlashSkill], lang: Lang) -> Self {
        let mut rows: Vec<SlashItem> = Vec::with_capacity(8 + skills.len());
        for command in SlashCommand::adds() {
            rows.push(command_item(SlashKind::Add, command, lang));
        }
        for command in SlashCommand::commands() {
            rows.push(command_item(SlashKind::Command, command, lang));
        }
        for skill in skills {
            rows.push(SlashItem {
                id: format!("skill-{}", skill.name),
                kind: SlashKind::Skill,
                icon: IconName::Sparkles,
                label: skill.name.clone(),
                description: skill.description.clone(),
                user_only: skill.user_only,
                action: SlashAction::Insert(skill.name.clone()),
            });
        }
        let lowered = rows
            .iter()
            .map(|row| (row.label.to_lowercase(), row.description.to_lowercase()))
            .collect();
        Self { rows, lowered }
    }

    /// The groups one query matches.
    ///
    /// A trimmed, lowercased substring test over the label AND the description,
    /// the way the reference's slash menu ranks its catalog rows. An empty
    /// group is left out; a query that matches nothing returns no groups, which
    /// the renderer treats as the reference's own auto-close condition.
    pub(crate) fn groups(&self, query: &str, lang: Lang) -> Vec<SlashGroup> {
        let needle = query.trim().to_lowercase();
        let adds = SlashCommand::adds().len();
        let commands = SlashCommand::commands().len();
        let sections = [
            (SlashKind::Add, 0, adds),
            (SlashKind::Command, adds, adds + commands),
            (SlashKind::Skill, adds + commands, self.rows.len()),
        ];
        let mut groups: Vec<SlashGroup> = Vec::with_capacity(sections.len());
        for (kind, start, end) in sections {
            let items: Vec<usize> = (start..end)
                .filter(|&index| {
                    needle.is_empty()
                        || self.lowered[index].0.contains(&needle)
                        || self.lowered[index].1.contains(&needle)
                })
                .collect();
            if !items.is_empty() {
                groups.push(SlashGroup {
                    kind,
                    title: lang.tr(kind.title_key()),
                    items,
                });
            }
        }
        groups
    }

    /// The row at a catalogue index.
    pub(crate) fn row(&self, index: usize) -> Option<&SlashItem> {
        self.rows.get(index)
    }
}

/// One command row, built from the command's own keys in the section it was
/// collected for.
fn command_item(kind: SlashKind, command: SlashCommand, lang: Lang) -> SlashItem {
    SlashItem {
        id: format!("cmd-{}", command.id()),
        kind,
        icon: command.icon(),
        label: lang.tr(command.label_key()),
        description: command
            .description_key()
            .map(|key| lang.tr(key))
            .unwrap_or_default(),
        user_only: false,
        action: SlashAction::Command(command),
    }
}

/// The description a row draws.
///
/// The reference's skill source PREFIXES a restricted skill's description with
/// the user-only marker; it is not a badge of its own, so the rule lives here
/// where both the list and a test can read it.
pub(crate) fn marked_description(item: &SlashItem, lang: Lang) -> String {
    if item.kind == SlashKind::Skill && item.user_only {
        format!("{} · {}", lang.tr("slash.userOnly"), item.description)
    } else {
        item.description.clone()
    }
}

/// The end of `text` as the editor's (line, character) position.
///
/// `Position.character` counts CHARACTERS in its line, not bytes - the
/// editor maps it to a byte offset itself - so a draft holding CJK text ends
/// at the same caret a reader sees.
pub(crate) fn text_end_position(text: &str) -> gpui_kit::component::input::Position {
    match text.rfind('\n') {
        Some(index) => gpui_kit::component::input::Position::new(
            text[..=index].matches('\n').count() as u32,
            text[index + 1..].chars().count() as u32,
        ),
        None => gpui_kit::component::input::Position::new(0, text.chars().count() as u32),
    }
}

/// Which list the card is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum SlashPane {
    /// The Add / Commands / Skills catalog for the typed query.
    #[default]
    Root,
    /// The permission presets, after the Permission row.
    Permissions,
    /// The conversation's model catalogue, after the Model row.
    Models,
}

/// The palette's own state: open, the live query, the rows that query computed,
/// the cached rows they index, and the highlight.
///
/// Nothing here is recomputed by a frame, and a KEYSTROKE costs only what its
/// own query needs: SlashPalette::sync runs from the draft's change event, the
/// catalogue is built once per settings generation and language, and a query
/// change re-derives indices into it rather than rebuilding rows.
#[derive(Default)]
pub(crate) struct SlashPalette {
    /// Whether the card is on screen.
    pub(crate) open: bool,
    /// Which list it is showing.
    pub(crate) pane: SlashPane,
    /// The query the cached groups were built for.
    query: String,
    /// The language those groups were built in.
    lang: Option<Lang>,
    /// The grouped rows, as indices into the active row list, rebuilt only when
    /// the query or the list moves.
    pub(crate) groups: Vec<SlashGroup>,
    /// The highlighted row, as a flat index across the groups.
    pub(crate) active: usize,
    /// The root rows, built once per settings generation and language.
    catalogue: SlashCatalogue,
    /// The settings generation that catalogue was built from, so a settings
    /// edit, a toggle or an install rebuilds it and nothing else does.
    catalogue_generation: Option<u64>,
    /// The language that catalogue was built in (row labels are translated).
    catalogue_lang: Option<Lang>,
    /// The rows a child list shows, addressed by the same indices as the root.
    choice: Vec<SlashItem>,
    /// The scroller's child index for each flat row (headers included).
    child_of_row: Vec<usize>,
    /// The list's scroll offset, so the wheel and a key move share it.
    pub(crate) scroll: ScrollHandle,
    /// Text the interceptor asked the editor to show. The key path has no
    /// window; the next frame does, and applies this the way it re-points the
    /// placeholder.
    pub(crate) pending: Option<String>,
}

impl SlashPalette {
    /// Follow the draft.
    ///
    /// A leading slash opens the menu and a changed query rebuilds the rows; a
    /// query that matches nothing CLOSES it, which is the reference's
    /// all-ready-and-empty rule, and leaves the typed text alone. The trigger is
    /// never touched here: Esc closes and leaves the slash exactly as typed.
    pub(crate) fn sync(&mut self, text: &str, state: &crate::state::AppState) {
        let Some(query) = slash_palette_query(text) else {
            self.close();
            return;
        };
        let lang = state.lang;
        // The catalogue is the expensive half, so it is rebuilt only when the
        // settings it was scanned from moved (their generation) or the language
        // did. Reading the generation first is a fingerprint compare inside the
        // settings cache - no filesystem work, no catalogue copy.
        let generation = state.settings.skill_generation();
        let mut catalogue_moved = false;
        if self.catalogue_generation != Some(generation) || self.catalogue_lang != Some(lang) {
            let skills = slash_skills(state);
            self.catalogue = SlashCatalogue::build(&skills, lang);
            // Read the generation AFTER the build: the scan it may have run
            // bumps it, and storing the pre-scan value would rebuild on every
            // keystroke.
            self.catalogue_generation = Some(state.settings.skill_generation());
            self.catalogue_lang = Some(lang);
            catalogue_moved = true;
        }
        if !self.open || self.query != query || self.lang != Some(lang) || catalogue_moved {
            self.open = true;
            self.pane = SlashPane::Root;
            self.query = query.to_string();
            self.lang = Some(lang);
            self.rebuild();
            self.active = 0;
        }
        // The reference closes the menu as soon as every source is ready and
        // empty; it never draws an empty popover over the composer.
        if self.pane == SlashPane::Root && self.is_empty() {
            self.close();
        }
    }

    /// Drop the card and its transient rows.
    ///
    /// The ROOT CATALOGUE survives: it is keyed by the settings generation and
    /// the language, so reopening the card on the same settings is a filter
    /// over rows that are already built, not a rescan.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.pane = SlashPane::Root;
        self.query.clear();
        self.lang = None;
        self.groups.clear();
        self.choice.clear();
        self.child_of_row.clear();
        self.active = 0;
        self.pending = None;
    }

    /// Replace the rows and the flat-row map, parking the highlight on the top.
    fn reset_rows(&mut self, groups: Vec<SlashGroup>) {
        self.groups = groups;
        self.child_of_row.clear();
        let mut child = 0;
        for group in &self.groups {
            child += 1;
            for _ in &group.items {
                self.child_of_row.push(child);
                child += 1;
            }
        }
        self.active = 0;
    }

    /// Rebuild the root rows for the current query.
    fn rebuild(&mut self) {
        let lang = self.lang.unwrap_or(Lang::En);
        let groups = self.catalogue.groups(&self.query, lang);
        self.reset_rows(groups);
    }

    /// Show the permission presets, with the one in effect marked.
    pub(crate) fn show_permissions(&mut self, current: &str, lang: Lang) {
        self.pane = SlashPane::Permissions;
        let title = lang.tr("slash.permission");
        let current_label = lang.tr("slash.current");
        let items: Vec<SlashItem> = super::composer::permission_modes()
            .into_iter()
            .map(|(value, key)| SlashItem {
                id: format!("preset-{value}"),
                kind: SlashKind::Choice,
                icon: IconName::ShieldAlert,
                label: lang.tr(key),
                description: if value == current {
                    current_label.clone()
                } else {
                    String::new()
                },
                user_only: false,
                action: SlashAction::Permission(value),
            })
            .collect();
        self.choice = items;
        self.reset_rows(vec![SlashGroup {
            kind: SlashKind::Choice,
            title,
            items: (0..self.choice.len()).collect(),
        }]);
    }

    /// Show the conversation's model catalogue.
    pub(crate) fn show_models(&mut self, groups: Vec<(String, Vec<String>)>, lang: Lang) {
        self.pane = SlashPane::Models;
        let title = lang.tr("slash.model");
        let mut items: Vec<SlashItem> = Vec::new();
        for (provider, models) in groups {
            for model in models {
                items.push(SlashItem {
                    id: format!("model-{provider}-{model}"),
                    kind: SlashKind::Choice,
                    icon: IconName::Database,
                    label: model.clone(),
                    description: provider.clone(),
                    user_only: false,
                    action: SlashAction::SelectModel {
                        provider: provider.clone(),
                        model,
                    },
                });
            }
        }
        self.choice = items;
        self.reset_rows(vec![SlashGroup {
            kind: SlashKind::Choice,
            title,
            items: (0..self.choice.len()).collect(),
        }]);
    }

    /// The number of rows across every group.
    pub(crate) fn len(&self) -> usize {
        self.groups.iter().map(|group| group.items.len()).sum()
    }

    /// Whether the current list has no rows.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The empty state's locale key. The reference closes the root menu when
    /// nothing matches, so this only ever draws for an option list, where the
    /// reference's popupSelect shows status.empty.
    pub(crate) fn empty_key(&self) -> &'static str {
        "commands.empty"
    }

    /// The row an index addresses.
    ///
    /// The root list indexes the cached catalogue; a child list indexes its own
    /// rows. Both are shared by reference, so drawing a row copies nothing.
    pub(crate) fn row(&self, index: usize) -> Option<&SlashItem> {
        match self.pane {
            SlashPane::Root => self.catalogue.row(index),
            SlashPane::Permissions | SlashPane::Models => self.choice.get(index),
        }
    }

    /// Every row, in draw order.
    pub(crate) fn rows(&self) -> impl Iterator<Item = &SlashItem> + '_ {
        let pane = self.pane;
        self.groups
            .iter()
            .flat_map(move |group| group.items.iter())
            .filter_map(move |index| match pane {
                SlashPane::Root => self.catalogue.row(*index),
                SlashPane::Permissions | SlashPane::Models => self.choice.get(*index),
            })
    }

    /// The row at a flat index.
    pub(crate) fn item(&self, index: usize) -> Option<&SlashItem> {
        self.rows().nth(index)
    }

    /// The highlighted row.
    pub(crate) fn selected(&self) -> Option<&SlashItem> {
        self.item(self.active)
    }

    /// Move the highlight by a delta, wrapping at both ends.
    fn move_by(&mut self, delta: isize) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let next = (self.active as isize + delta).rem_euclid(len as isize) as usize;
        self.active = next;
        if let Some(child) = self.child_of_row.get(next) {
            self.scroll.scroll_to_item(*child);
        }
    }

    /// Follow the pointer: the row it entered becomes the highlight.
    pub(crate) fn set_active(&mut self, index: usize) {
        if index < self.len() {
            self.active = index;
        }
    }
}

/// What one key did to the palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SlashKey {
    /// The palette handled the key and the keystroke is consumed.
    Handled,
    /// The key belongs to the editor.
    Pass,
}

/// The inline note the composer is editing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NoteKind {
    /// The session's goal.
    Goal,
    /// The reader's note about the session.
    Feedback,
}

impl Shell {
    /// The card the composer draws: the palette while it is open, otherwise the
    /// plus command card. One overlay, so the two can never stack.
    pub(crate) fn composer_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.slash.open {
            self.slash_palette(cx)
        } else {
            self.command_menu(cx)
        }
    }

    /// The slash menu, drawn above the composer card.
    pub(crate) fn slash_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.slash.open {
            return None;
        }
        let lang = self.state.lang;
        let mut viewport = div()
            .id("slash-viewport")
            .debug_selector(|| String::from("slash-viewport"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.slash.scroll);
        if self.slash.is_empty() {
            viewport = viewport.child(
                div()
                    .debug_selector(|| String::from("slash-empty"))
                    .px(px(SECTION_PAD_X))
                    .py(px(EMPTY_PAD))
                    .text_size(px(EMPTY_SIZE))
                    .text_color(rgb(theme::faint()))
                    .child(lang.tr(self.slash.empty_key())),
            );
        } else {
            let mut index = 0;
            for group in &self.slash.groups {
                viewport = viewport.child(slash_group_header(group));
                for row in &group.items {
                    if let Some(item) = self.slash.row(*row) {
                        viewport = viewport.child(self.slash_row(index, item, cx));
                    }
                    index += 1;
                }
            }
        }
        Some(
            div()
                .debug_selector(|| String::from("composer-slash-palette"))
                .absolute()
                .bottom(relative(1.0))
                .mb(px(PALETTE_GAP))
                .left(px(0.0))
                .right(px(0.0))
                .max_h(px(PALETTE_MAX_H))
                .flex()
                .flex_col()
                .p(px(PALETTE_PAD))
                .rounded(px(16.0))
                .bg(rgb(theme::page()))
                .shadow_lg()
                .occlude()
                .child(viewport)
                .into_any_element(),
        )
    }

    /// One row, exactly the reference's .tki__G_item: glyph, name, description.
    fn slash_row(&self, index: usize, item: &SlashItem, cx: &mut Context<Self>) -> AnyElement {
        let selected = index == self.slash.active;
        let id = item.id.clone();
        let mut row = div()
            .id(SharedString::from(format!("slash-row-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("slash-row-{id}")
            })
            .min_h(px(ROW_MIN_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(ROW_GAP))
            .px(px(ROW_PAD_X))
            .py(px(ROW_PAD_Y))
            .rounded(px(ROW_RADIUS))
            .cursor_pointer()
            .text_size(px(LABEL_SIZE))
            .line_height(px(LABEL_LINE))
            .text_color(rgb(theme::text()))
            .on_mouse_move(cx.listener(move |this, _event, _window, cx| {
                if this.slash.active != index {
                    this.slash.set_active(index);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.run_slash_row(index, window, cx);
            }))
            .child(icon(item.icon, ROW_ICON, theme::faint()).flex_none())
            .child(
                div()
                    .flex_none()
                    .max_w(relative(NAME_MAX_SHARE))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(SharedString::from(item.label.clone())),
            );
        let description = marked_description(item, self.state.lang);
        if !description.is_empty() {
            row = row.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_size(px(DESC_SIZE))
                    .line_height(px(DESC_LINE))
                    .text_color(rgb(theme::faint()))
                    .text_right()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(SharedString::from(description)),
            );
        }
        if selected {
            row = row.bg(rgb(theme::hover()));
        }
        row.into_any_element()
    }

    /// Choosing a row: run it, then either keep the card on its child list or
    /// close it and consume the trigger text.
    pub(crate) fn run_slash_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.slash.item(index) else {
            return;
        };
        let action = item.action.clone();
        match action {
            SlashAction::Command(command) => match command.run(self, window, cx) {
                SlashEffect::Chooser => {
                    // The child list stays in the card and the draft is kept.
                    // The write is still a write, not a no-op: the kit's
                    // submit-on-enter path propagates the keystroke, and a row
                    // that did not rewrite the draft would receive that
                    // newline as text (which closes the card). This is the
                    // same cancellation the inserting rows get from their own
                    // write.
                    self.slash.active = 0;
                    let text = self.composer_value.clone();
                    self.set_composer_text(&text, window, cx);
                }
                SlashEffect::Kept => {
                    // The command left the draft as it wants it (the File row
                    // put a bare `@` there and opened the reference picker).
                    self.slash.close();
                }
                SlashEffect::Done => {
                    self.slash.close();
                    self.set_composer_text("", window, cx);
                }
            },
            // The reference inserts the picked skill's name as the prompt's
            // slash invocation; it never runs a tool on the reader's behalf.
            // The send is what loads it: the runner's turn task resolves the
            // gesture against the skills that build registered and appends
            // the instructions (crate::skill_invocation), which is exactly
            // the reference's pre-step skill listener.
            SlashAction::Insert(name) => {
                self.slash.close();
                self.set_composer_text(&format!("/{name} "), window, cx);
            }
            SlashAction::Permission(value) => {
                self.slash.close();
                self.set_composer_text("", window, cx);
                self.request_permission(value, cx);
            }
            SlashAction::SelectModel { provider, model } => {
                self.slash.close();
                self.set_composer_text("", window, cx);
                // The choice belongs to THIS conversation, which is exactly what
                // the session-scoped selector records and saves.
                self.state.select_model_from(&provider, &model);
            }
        }
        cx.notify();
    }

    /// Tab completes the highlighted row's name into the draft, keeping the card
    /// open on the completed query. A child list is a choice, not a query, so
    /// Tab does not complete there.
    fn complete_slash(&mut self) {
        if self.slash.pane != SlashPane::Root {
            return;
        }
        let Some(label) = self.slash.selected().map(|item| item.label.clone()) else {
            return;
        };
        let next = format!("/{label}");
        // The key path has no window, so the editor is updated on the next
        // frame; the query moves NOW, so the rows the reader sees match what
        // they will be typing.
        self.slash.pending = Some(next.clone());
        self.slash.sync(&next, &self.state);
    }

    /// The menu's key contract: up and down move, Esc closes and leaves the
    /// trigger, Tab completes the name. Enter is the composer's own PressEnter
    /// event, because the kit's input owns that keystroke.
    pub(crate) fn slash_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> SlashKey {
        if !self.slash.open {
            return SlashKey::Pass;
        }
        if keystroke.modifiers.shift || keystroke.modifiers.control || keystroke.modifiers.alt {
            return SlashKey::Pass;
        }
        match keystroke.key.as_str() {
            "up" => {
                self.slash.move_by(-1);
                cx.notify();
                SlashKey::Handled
            }
            "down" => {
                self.slash.move_by(1);
                cx.notify();
                SlashKey::Handled
            }
            "escape" => {
                self.slash.close();
                cx.notify();
                SlashKey::Handled
            }
            "tab" => {
                self.complete_slash();
                cx.notify();
                SlashKey::Handled
            }
            _ => SlashKey::Pass,
        }
    }

    /// Whether the composer's editor holds the keys.
    pub(crate) fn composer_focused(&self, window: &Window, cx: &App) -> bool {
        self.composer.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Replace the whole draft, keeping the mirror and the editor in step, and
    /// leave the caret at the END of the new text.
    ///
    /// `TextareaState::set_value` resets a MULTI-LINE editor's selection to
    /// `0..0`. That is right for restoring a document and wrong for an INSERT:
    /// a Tab completion, a command row, a command claim and an `@` mention all
    /// write text the reader is about to continue typing, and the caret was
    /// landing in front of it. The position is expressed as (line, character),
    /// which is the editor's own mapping and stays exact for non-ASCII text.
    ///
    /// The caret is only moved while the composer already holds the keys: a
    /// write whose command hands focus to another surface (the Feedback row
    /// opens the note editor) must not steal that focus back.
    pub(crate) fn set_composer_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer_value = text.to_string();
        let end = text_end_position(text);
        // The draft goes in as CONTENT, not as a string: every `@path` the
        // grammar finds rides as an atomic token, which is what lets the
        // editor paint the reference's chip while still holding one run of
        // text for editing, undo and selection.
        let content = super::reference_chip::content(text);
        self.composer.update(cx, |state, cx| {
            let holds_keys = state.focus_handle(cx).is_focused(window);
            state.set_value(content, window, cx);
            if holds_keys {
                state.set_cursor_position(end, window, cx);
            }
        });
    }

    /// Turn every finished `@reference` in the draft into a chip.
    ///
    /// `set_value` replaces the whole document, so this runs ONLY when the
    /// draft has a reference that is not already a token: ordinary typing,
    /// where nothing is at stake, never pays for it and never moves the caret.
    /// A reference still being typed is left alone - the reader has not
    /// finished naming the file, and chipping a half-typed path would fight
    /// them - which the scanner already handles by not matching an unfinished
    /// quoted form.
    pub(crate) fn chip_open_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer_value.clone();
        let wanted = super::reference_chip::content(&text);
        // Compared by the run of text the tokens would cover: a draft whose
        // references are already chipped is left untouched, so this cannot
        // loop on its own rewrite.
        let held: Vec<(usize, usize)> = self
            .composer
            .read(cx)
            .tokens()
            .iter()
            .map(|span| (span.range().start, span.range().end))
            .collect();
        let want: Vec<(usize, usize)> = wanted
            .tokens()
            .iter()
            .map(|span| (span.range().start, span.range().end))
            .collect();
        if held == want {
            return;
        }
        let end = text_end_position(&text);
        let holds_keys = self.composer.read(cx).focus_handle(cx).is_focused(window);
        self.composer.update(cx, |state, cx| {
            state.set_value(wanted, window, cx);
            if holds_keys {
                state.set_cursor_position(end, window, cx);
            }
        });
    }

    /// Open the file a reference chip names, in the right pane's editor.
    ///
    /// The token carries the path as its id, so this never re-parses the draft:
    /// the chip the reader pressed is the file that opens.
    pub(crate) fn open_reference_chip(
        &mut self,
        event: &gpui_kit::component::input::InlineTokenClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_reference_path(&super::reference_chip::clicked_path(event), window, cx);
    }

    /// Open one reference path in the right pane's editor.
    ///
    /// Split from the listener so the decision is testable: the kit builds
    /// `InlineTokenClickEvent` itself and offers no constructor, so a test
    /// cannot synthesise the press - it drives this instead, and the listener
    /// above is a one-line adapter over it.
    pub(crate) fn open_reference_path(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A directory has nothing to open: the reference links files.
        if path.is_empty() || path.ends_with('/') {
            return;
        }
        let rel = std::path::Path::new(path);
        self.show_right_tab(super::right_panel::RightTab::Files, cx);
        self.open_file(rel, window, cx);
        cx.notify();
    }

    /// Open the inline note editor for the session on screen, pre-filled with
    /// what it already holds - which is what makes Goal "set or view".
    pub(crate) fn open_note(
        &mut self,
        kind: NoteKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lang = self.state.lang;
        let (text, placeholder) = match kind {
            NoteKind::Goal => (
                self.state
                    .active_id
                    .as_deref()
                    .and_then(|id| self.state.goal(id))
                    .map(|goal| goal.objective)
                    .unwrap_or_default(),
                lang.tr("slash.goalPlaceholder"),
            ),
            NoteKind::Feedback => (
                self.state
                    .active_session()
                    .and_then(|session| session.feedback.clone())
                    .unwrap_or_default(),
                lang.tr("slash.feedbackPlaceholder"),
            ),
        };
        self.note_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
        self.note_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.note = Some(kind);
        let handle = self.note_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Close the note editor and give the keys back to the composer.
    pub(crate) fn close_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.note = None;
        let handle = self.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Commit the note editor: a goal objective or a feedback note, persisted.
    ///
    /// The goal editor is the reference's inline edit form: it rewrites the
    /// objective in place (a completed goal is replaced with a fresh identity)
    /// and never clears - its own Save control is disabled on an empty draft, so
    /// a blank line is a no-op that leaves the editor open. The goal domain's
    /// clear verb is the strip's Clear control.
    pub(crate) fn commit_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.note else {
            return;
        };
        let Some(id) = self.state.active_id.clone() else {
            self.close_note(window, cx);
            return;
        };
        let text = self.note_input.read(cx).value().to_string();
        let blank = text.trim().is_empty();
        match kind {
            NoteKind::Goal => {
                if blank {
                    return;
                }
                if self.state.edit_goal(&id, &text).is_err()
                    && self.state.create_goal(&id, &text).is_err()
                {
                    return;
                }
            }
            NoteKind::Feedback => {
                self.state.set_session_feedback(&id, &text);
                let key = if blank {
                    "slash.feedbackCleared"
                } else {
                    "slash.feedbackSaved"
                };
                let notice = self.state.lang.tr(key);
                self.state.notice(notice);
            }
        }
        self.close_note(window, cx);
    }

    /// The inline note editor, drawn above the composer card while one is open.
    pub(crate) fn note_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let kind = self.note?;
        let lang = self.state.lang;
        let title = match kind {
            NoteKind::Goal => lang.tr("slash.goal"),
            NoteKind::Feedback => lang.tr("slash.feedback"),
        };
        let save = lang.tr("slash.save");
        let cancel = lang.tr("slash.cancel");
        Some(
            div()
                .debug_selector(|| String::from("composer-note"))
                .id("composer-note")
                .mb(px(8.0))
                .p(px(10.0))
                .rounded(px(16.0))
                .border_1()
                .border_color(rgb(theme::border_soft()))
                .bg(rgb(theme::page()))
                .shadow_sm()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key.as_str() == "escape" {
                        this.close_note(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(txt(title, 11.0, theme::faint()))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .debug_selector(|| String::from("note-input"))
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.note_input).appearance(false).h(px(28.0))),
                        )
                        .child(
                            div()
                                .id("note-save")
                                .debug_selector(|| String::from("note-save"))
                                .h(px(28.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .bg(rgb(theme::accent()))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.commit_note(window, cx);
                                }))
                                .child(txt(save, theme::CHIP_SIZE, 0xffffff)),
                        )
                        .child(
                            div()
                                .id("note-cancel")
                                .debug_selector(|| String::from("note-cancel"))
                                .h(px(28.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(rgb(theme::border()))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .hover(|style| style.bg(rgb(theme::hover())))
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.close_note(window, cx);
                                }))
                                .child(txt(cancel, theme::CHIP_SIZE, theme::text())),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The reference's plan slot: a mode chip that exists ONLY while plan mode
    /// is on.
    ///
    /// The reference shows it as entered-and-marked state, not as a switch that
    /// is always sitting in the footer: entering plan mode (the `/plan` row or
    /// command) draws it, and pressing it - its circled-x glyph - leaves plan
    /// mode, which removes the chip and the plan-specific placeholder with it.
    pub(crate) fn plan_chip(
        &self,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.state.active_plan_mode() {
            return None;
        }
        let colour = theme::accent();
        let label = lang.tr("input.plan");
        let hint = lang.tr("plan.onHint");
        // Measured from PlanModeControl.module.css: .chip{min-width:34px;
        // height:28px;color:state-business-primary;background:state-business-
        // tertiary;border:none;gap:4px;padding:0 8px;font:500 13px/20px;
        // border-radius:radius-sm} with a 14px rest glyph (plan outline) that
        // the enabled hover/focus swaps for a filled circled cross.
        Some(
            div()
                .debug_selector(|| String::from("composer-plan"))
                .id("composer-plan")
                .group("composer-plan")
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .bg(rgb(theme::accent_soft()))
                .hover(|style| style.bg(rgb(theme::accent_soft())))
                .tooltip_show_delay(Duration::from_millis(500))
                .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
                .child(
                    div()
                        .flex()
                        .group_hover("composer-plan", |style| style.opacity(0.0))
                        .child(icon(IconName::ClipboardList, 14.0, colour)),
                )
                .child(
                    div()
                        .flex()
                        .opacity(0.0)
                        .group_hover("composer-plan", |style| style.opacity(1.0))
                        .child(icon(IconName::CircleX, 14.0, colour)),
                )
                .child(
                    txt(label, 13.0, colour)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .line_height(px(20.0)),
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.exit_plan_mode(cx);
                }))
                .into_any_element(),
        )
    }

    /// Leave plan mode: the chip's own action and the `/plan off` command.
    pub(crate) fn exit_plan_mode(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        if !self.state.plan_mode(&id) {
            return;
        }
        self.state.set_plan_mode(&id, false);
        let notice = self.state.lang.tr("slash.planOff");
        self.state.notice(notice);
        cx.notify();
    }

    /// Enter plan mode: bare `/plan` and `/plan <task>`.
    pub(crate) fn enter_plan_mode(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        if self.state.plan_mode(&id) {
            return;
        }
        self.state.set_plan_mode(&id, true);
        let notice = self.state.lang.tr("slash.planOn");
        self.state.notice(notice);
        cx.notify();
    }

    /// Put a command token in the draft and keep the caret there, which is how
    /// the reference's commands take their argument.
    pub(crate) fn begin_command_claim(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_composer_text(&format!("{name} "), window, cx);
        let handle = self.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Run one parsed `/plan` or `/goal` line. Its output is a UI notice, the
    /// way the reference keeps command output out of model requests.
    pub(crate) fn run_command_line(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use crate::state::{PlanRequest, SlashLine};
        let text = self.composer_value.clone();
        let Some(line) = crate::state::parse_slash_line(&text) else {
            return false;
        };
        self.set_composer_text("", window, cx);
        match line {
            SlashLine::Plan(PlanRequest::On) => self.enter_plan_mode(cx),
            SlashLine::Plan(PlanRequest::Off) => self.exit_plan_mode(cx),
            SlashLine::Plan(PlanRequest::Task(task)) => {
                self.enter_plan_mode(cx);
                self.set_composer_text(&task, window, cx);
                self.send(window, cx);
            }
            SlashLine::Goal(request) => self.run_goal_line(request, cx),
            SlashLine::Compact(args) => self.compact_command_line(&args, cx),
        }
        true
    }

    /// Apply one /goal request and report only what the goal bar cannot.
    ///
    /// A state change is reported by the goal bar above the composer, which
    /// already carries the phase, the objective and the valid actions. The
    /// multi-line status dump is not repeated on the status bar, which speaks
    /// only in the one-line refusals and errors it exists for. A bare /goal is
    /// the one explicit query, so it keeps the reference command's own output -
    /// a title, Status, an optional Blocker, Objective, Rounds, Activation, a
    /// blank line and the commands meaningful from the live state - as a UI
    /// notice that never enters a model request.
    fn run_goal_line(&mut self, request: crate::state::GoalRequest, cx: &mut Context<Self>) {
        use crate::state::{GoalRefusal, GoalRequest};
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        let lang = self.state.lang;
        let usage = lang.tr("goal.usage");
        let noun_objective = matches!(request, GoalRequest::Create(_) | GoalRequest::Edit(_));
        // Attachments accompany only an objective. Every other verb refuses
        // BEFORE any mutation and leaves the composer's cards intact, which is
        // the reference's command contract.
        if !self.state.attachments.is_empty() && !noun_objective {
            self.state.notice(lang.tr("goal.attachmentsOnly"));
            cx.notify();
            return;
        }
        let attachments = self.state.attachments.clone();
        let notice = match request {
            GoalRequest::Show => Some(match self.state.goal(&id) {
                None => format!("{}\n{}", lang.tr("goal.none"), usage),
                Some(goal) => goal_report(lang, &lang.tr("goal.showTitle"), &goal),
            }),
            GoalRequest::Create(objective) => match self.state.create_goal(&id, &objective) {
                Ok(_) => {
                    self.submit_goal_attachments(&id, &attachments);
                    // Creating arms the goal; an idle session starts its first
                    // round now, exactly as the reference driver does.
                    self.continue_goal(&id, cx);
                    None
                }
                Err(GoalRefusal::AlreadyExists(state)) => Some(crate::i18n::tr_args(
                    lang,
                    "goal.already",
                    &[("phase", goal_phase_label(lang, state))],
                )),
                Err(_) => Some(lang.tr("goal.invalid")),
            },
            GoalRequest::Edit(objective) => match self.state.edit_goal(&id, &objective) {
                Ok(_) => {
                    self.submit_goal_attachments(&id, &attachments);
                    self.continue_goal(&id, cx);
                    None
                }
                Err(GoalRefusal::Missing) => Some(crate::i18n::tr_args(
                    lang,
                    "goal.missing",
                    &[("action", "edit".to_string()), ("usage", usage)],
                )),
                Err(GoalRefusal::InvalidObjective) => Some(crate::i18n::tr_args(
                    lang,
                    "goal.needObjective",
                    &[("usage", usage)],
                )),
                Err(_) => Some(lang.tr("goal.invalid")),
            },
            GoalRequest::InvalidEdit => Some(crate::i18n::tr_args(
                lang,
                "goal.needObjective",
                &[("usage", usage)],
            )),
            GoalRequest::Pause => match self.state.pause_goal(&id) {
                Ok(()) => None,
                Err(GoalRefusal::Missing) => Some(crate::i18n::tr_args(
                    lang,
                    "goal.missing",
                    &[("action", "pause".to_string()), ("usage", usage)],
                )),
                Err(_) => Some(lang.tr("goal.invalid")),
            },
            GoalRequest::Resume => match self.state.resume_goal(&id) {
                Ok(()) => {
                    self.continue_goal(&id, cx);
                    None
                }
                Err(GoalRefusal::Missing) => Some(crate::i18n::tr_args(
                    lang,
                    "goal.missing",
                    &[("action", "resume".to_string()), ("usage", usage)],
                )),
                Err(_) => Some(lang.tr("goal.invalid")),
            },
            GoalRequest::Clear => Some(if self.state.clear_goal(&id) {
                lang.tr("goal.cleared")
            } else {
                lang.tr("goal.noClear")
            }),
        };
        match notice {
            Some(notice) => self.state.notice(notice),
            None => self.state.clear_notice(),
        }
        cx.notify();
    }

    /// Submit the attachments a successful create or edit admitted, then
    /// consume them. One ordinary user message, ahead of the next round.
    fn submit_goal_attachments(&mut self, id: &str, attachments: &[crate::state::Attachment]) {
        if attachments.is_empty() {
            return;
        }
        let slot = self
            .state
            .store
            .session(id)
            .map(|session| session.loaded_from + session.messages.len())
            .unwrap_or(0);
        if self.state.append_goal_attachments(id, attachments) {
            self.transcript_images.borrow_mut().invalidate_from(slot);
            self.state.attachments.clear();
        }
    }

    /// Download the session on screen as a ZIP of its log, through the
    /// platform's save dialog.
    pub(crate) fn export_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_id.clone() else {
            let notice = self.state.lang.tr("slash.exportEmpty");
            self.state.notice(notice);
            cx.notify();
            return;
        };
        let Some(dir) = self.state.store.history_dir(&id) else {
            let notice = self.state.lang.tr("slash.exportEmpty");
            self.state.notice(notice);
            cx.notify();
            return;
        };
        let title = self
            .state
            .active_session()
            .map(|session| session.title.clone())
            .unwrap_or_default();
        let suggested = export_name(&title);
        let target = cx.prompt_for_new_path(&dir, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = target.await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { crate::export::write_zip(&dir, &path).map(|_| path) })
                .await;
            let _ = this.update_in(cx, |this, _window, cx| {
                let text = match result {
                    Ok(path) => crate::i18n::tr_args(
                        this.state.lang,
                        "slash.exported",
                        &[("path", path.display().to_string())],
                    ),
                    Err(_) => this.state.lang.tr("slash.exportFailed"),
                };
                this.state.notice(text);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep an armed goal going: one finished round starts the next.
    ///
    /// The gate is the reference driver's: idle, active, armed, and room left
    /// under the round cap. No model means no round; the goal stays armed and a
    /// later idle edge tries again. The round instruction is the reference's
    /// exact goal_round prompt, and the transcript grows only the assistant
    /// placeholder - bsh has no goal-sourced user row, and the parity table says
    /// so rather than faking one.
    pub(crate) fn continue_goal(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.is_streaming(id) {
            return;
        }
        let Some(goal) = self.state.goal(id) else {
            return;
        };
        // The reference driver's cap edge: an armed active goal with no rounds
        // left is BLOCKED with the stable code round-limit, not left active and
        // silently stalled.
        if goal.state.is_open() && goal.activation.is_armed() && goal.exhausted() {
            let message = format!(
                "Goal reached its configured limit of {} rounds.",
                goal.max_rounds
            );
            if self.state.block_goal(id, "round-limit", &message) {
                cx.notify();
            }
            return;
        }
        if !goal.can_advance() {
            return;
        }
        let choice = self.state.session_choice(id);
        if self.state.settings.refusal_for(&choice).is_some() {
            return;
        }
        if crate::caps::credential_error(&self.state.settings, &self.state.vault, &choice).is_some()
        {
            return;
        }
        let Some((transcript, budget)) = self.state.begin_goal_turn_in(id) else {
            return;
        };
        let input = react::llm::Content::Text(crate::state::goal_round_input(
            &goal.objective,
            goal.rounds + 1,
            goal.max_rounds,
        ));
        self.spawn_turn(id.to_string(), input, transcript, budget, cx);
    }

    /// A failed round disarms the goal instead of retrying forever.
    ///
    /// The phase stays active, so the strip offers resume - which is the
    /// reference driver's rule (a cancellation disarms continuation; it does
    /// not change the durable lifecycle).
    pub(crate) fn disarm_goal_on_error(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.state.goal_can_advance(id) {
            return;
        }
        if self.state.disarm_goal(id) {
            let notice = self.state.lang.tr("goal.disarmedOnError");
            self.state.notice(notice);
            cx.notify();
        }
    }

    /// The goal strip's pause: stop further rounds, keep the goal.
    pub(crate) fn pause_active_goal(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        let lang = self.state.lang;
        match self.state.pause_goal(&id) {
            Ok(()) => self.state.clear_notice(),
            Err(_) => {
                let notice = lang.tr("goal.invalid");
                self.state.notice(notice);
            }
        }
        cx.notify();
    }

    /// The goal strip's resume: re-arm the goal and continue it now when idle.
    pub(crate) fn resume_active_goal(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        let lang = self.state.lang;
        match self.state.resume_goal(&id) {
            Ok(()) => {
                self.state.clear_notice();
                self.continue_goal(&id, cx);
            }
            Err(_) => {
                let notice = lang.tr("goal.invalid");
                self.state.notice(notice);
            }
        }
        cx.notify();
    }

    /// The goal strip's clear: remove the goal, keeping its history.
    pub(crate) fn clear_active_goal(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_id.clone() else {
            return;
        };
        let lang = self.state.lang;
        let notice = if self.state.clear_goal(&id) {
            lang.tr("goal.cleared")
        } else {
            lang.tr("goal.noClear")
        };
        self.state.notice(notice);
        cx.notify();
    }
}

/// The archive name the save dialog suggests, from the session's own title.
fn export_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                ' '
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join("-");
    let stem: String = cleaned.chars().take(60).collect();
    let stem = if stem.is_empty() {
        "session".to_string()
    } else {
        stem
    };
    format!("{stem}.zip")
}

/// The locale label for one durable phase, in the reference's own words.
fn goal_phase_label(lang: Lang, state: agent::goal::GoalState) -> String {
    lang.tr(match state {
        agent::goal::GoalState::Active => "goal.phaseActive",
        agent::goal::GoalState::Paused => "goal.phasePaused",
        agent::goal::GoalState::Completed => "goal.phaseCompleted",
        agent::goal::GoalState::Blocked => "goal.phaseBlocked",
    })
}

/// The commands that are meaningful from one live state.
///
/// The reference's rule: an armed active goal can pause; an active-but-disarmed,
/// paused or blocked goal can resume; a completed goal can only be replaced or
/// cleared.
fn goal_command_hint(lang: Lang, goal: &crate::model::Goal) -> String {
    use agent::goal::GoalState;
    let key = match goal.state {
        GoalState::Completed => "goal.commandsComplete",
        GoalState::Active if goal.activation.is_armed() => "goal.commandsActive",
        _ => "goal.commandsResume",
    };
    lang.tr(key)
}

/// Render the reference command's direct output for one goal view.
///
/// The line order is the reference's own, byte for byte in English:
/// title, Status, the Blocker line (blocked goals only), Objective, Rounds,
/// Activation, a blank line, Commands.
fn goal_report(lang: Lang, title: &str, goal: &crate::model::Goal) -> String {
    let mut lines = vec![
        title.to_string(),
        crate::i18n::tr_args(
            lang,
            "goal.status",
            &[("phase", goal_phase_label(lang, goal.state))],
        ),
    ];
    if let Some(reason) = &goal.blocked_reason {
        lines.push(crate::i18n::tr_args(
            lang,
            "goal.blocker",
            &[
                ("code", reason.code.clone()),
                ("message", reason.message.clone()),
            ],
        ));
    }
    lines.push(crate::i18n::tr_args(
        lang,
        "goal.objective",
        &[("objective", goal.objective.clone())],
    ));
    lines.push(crate::i18n::tr_args(
        lang,
        "goal.rounds",
        &[
            ("round", goal.rounds.to_string()),
            ("max", goal.max_rounds.to_string()),
        ],
    ));
    let activation = if goal.activation.is_armed() {
        lang.tr("goal.activationArmed")
    } else {
        lang.tr("goal.activationDisarmed")
    };
    lines.push(crate::i18n::tr_args(
        lang,
        "goal.activation",
        &[("activation", activation)],
    ));
    lines.push(String::new());
    lines.push(crate::i18n::tr_args(
        lang,
        "goal.commands",
        &[("commands", goal_command_hint(lang, goal))],
    ));
    lines.join("\n")
}

/// The Add and Commands section titles, and the Skills group title.
fn slash_group_header(group: &SlashGroup) -> AnyElement {
    match group.kind {
        SlashKind::Skill => div()
            .debug_selector(|| String::from("slash-header-skills"))
            .px(px(GROUP_PAD_X))
            .py(px(GROUP_PAD_Y))
            .text_size(px(GROUP_SIZE))
            .line_height(px(GROUP_LINE))
            .text_color(rgb(theme::faint()))
            .child(SharedString::from(group.title.clone()))
            .into_any_element(),
        _ => {
            let later = group.kind == SlashKind::Command;
            let id = group.kind.id();
            div()
                .debug_selector(move || format!("slash-header-{id}"))
                .min_h(px(SECTION_MIN_H))
                .px(px(SECTION_PAD_X))
                .pt(px(SECTION_PAD_TOP))
                .pb(px(SECTION_PAD_BOTTOM))
                .when(later, |style| style.mt(px(SECTION_GAP)))
                .text_size(px(SECTION_SIZE))
                .line_height(px(SECTION_LINE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(theme::faint()))
                .child(SharedString::from(group.title.clone()))
                .into_any_element()
        }
    }
}
