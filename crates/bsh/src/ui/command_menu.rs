//! The composer's menu card: the reference's `+` command list and its `@`
//! reference (`@file` / `@session`) source, in one card.
//!
//! The card itself is measured from the reference's `PopupSelectView`
//! (`docs/ui-mirror/reference/pkgs/dsh-client-ui-commands-client.js`): a card
//! that spans the input's width, rows carrying a label and an 11px detail, a
//! search text that filters them locally, and `status.empty` when nothing
//! matches. The `+` control (`.add`, tooltipped `input.commands` - "Add files or
//! run commands") opens it on the command source; typing `/` opens the same card
//! and typing `@` swaps it to the reference source, which the reference
//! registers as an `inputTriggers` source in
//! `docs/ui-mirror/reference/pkgs/dsh-client-ui-reference-client.js`
//! (`trigger: "@"`, `name: "reference"`, `showGroupTitle: false`, candidates
//! from `fileReferences.list` beside `sessionReferenceResolver.candidates`, and
//! an `onPick` that either inserts a mention or drills into a directory with
//! `continue: true`).
//!
//! bsh draws the card in the composer itself - not in a kit popup layer - so the
//! rows are real controls in the element tree, and it lists only what this
//! harness backs: local rows, workspace files and retained sessions.

use std::path::PathBuf;

use gpui_kit::*;
use notify::Watcher as _;

use super::shell::Shell;
use crate::fuzzy;
use crate::theme;
use crate::workspace_index::{Entry, IndexStatus};

/// Measured `.card{min-width:min(220px,100%);max-width:100%;max-height:320px;
/// padding:3px;position:absolute;bottom:calc(100% + 4px);left:0;right:0;
/// overflow:hidden}` over the menu surface and the prominent elevation.
pub(crate) const CARD_MIN_W: f32 = 220.0;
pub(crate) const CARD_MAX_H: f32 = 320.0;
pub(crate) const CARD_PAD: f32 = 3.0;
pub(crate) const CARD_GAP: f32 = 4.0;
/// Measured `.row{border-radius:radius-md;cursor:pointer;color:label-primary;
/// align-items:center;gap:6px;padding:5px 7px;font-size:12px}` with
/// `.rowActive{background:interactive-bg-hover}`.
pub(crate) const ROW_GAP: f32 = 6.0;
pub(crate) const ROW_PAD_X: f32 = 7.0;
pub(crate) const ROW_PAD_Y: f32 = 5.0;
pub(crate) const ROW_RADIUS: f32 = 12.0;
pub(crate) const ROW_FONT: f32 = 12.0;
/// Measured `.label{align-items:baseline;gap:4px;min-width:0}` with its
/// `.labelText{white-space:nowrap;text-overflow:ellipsis}`.
pub(crate) const LABEL_GAP: f32 = 4.0;
/// Measured `.detail{min-width:0;color:label-tertiary;white-space:nowrap;
/// text-overflow:ellipsis;flex:1;font-size:11px}`.
pub(crate) const DETAIL_FONT: f32 = 11.0;
/// Measured `.status{color:label-tertiary;padding:7px 8px;font-size:12px}`.
pub(crate) const STATUS_PAD_X: f32 = 8.0;
pub(crate) const STATUS_PAD_Y: f32 = 7.0;
pub(crate) const STATUS_FONT: f32 = 12.0;

/// The most rows one card draws.
///
/// The index ranks every candidate; the card draws this many, so a query that
/// matches thousands of paths still costs one bounded list. The status line
/// reports the full match count, which is what makes the cut honest.
const MAX_ROWS: usize = 200;

/// One menu row: the reference's own `{ id, label, detail }` option, with what
/// bsh does when it is chosen. The reference's `badge` (`<sup>`, 8px/600) and
/// its `check` (`IconCheckOutlineRegular`, 14px, on the option that is already
/// in effect) have no data behind them here and are not drawn.
#[derive(Clone, Copy)]
pub(crate) struct CommandEntry {
    /// A stable id, for the row's key and its debug selector.
    pub(crate) id: &'static str,
    /// The row's label key.
    pub(crate) label: &'static str,
    /// The row's 11px detail key.
    pub(crate) detail: &'static str,
    /// What choosing the row does.
    pub(crate) run: fn(&mut Shell, &mut Window, &mut Context<Shell>),
}

fn attach_file(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    shell.attach_files(window, cx);
}

fn new_chat(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    shell.new_chat(window, cx);
}

fn settings(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    shell.open_settings(window, cx);
}

fn toggle_sidebar(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    let viewport = window.viewport_size().width.as_f32();
    shell.toggle_sidebar_at(viewport, cx);
}

fn stop_turn(shell: &mut Shell, _window: &mut Window, cx: &mut Context<Shell>) {
    shell.stop_active(cx);
}

/// The actions bsh backs, in the reference's own row order: the attach action
/// the `+` used to run directly, then the harness's own commands. The stop row
/// exists only while something runs, so no row is ever a no-op.
pub(crate) fn entries(streaming: bool) -> Vec<CommandEntry> {
    let mut rows = vec![
        CommandEntry {
            id: "attach-file",
            label: "commands.attachFile",
            detail: "commands.attachFileDetail",
            run: attach_file,
        },
        CommandEntry {
            id: "new-chat",
            label: "commands.newChat",
            detail: "commands.newChatDetail",
            run: new_chat,
        },
        CommandEntry {
            id: "settings",
            label: "commands.settings",
            detail: "commands.settingsDetail",
            run: settings,
        },
        CommandEntry {
            id: "toggle-sidebar",
            label: "commands.toggleSidebar",
            detail: "commands.toggleSidebarDetail",
            run: toggle_sidebar,
        },
    ];
    if streaming {
        rows.push(CommandEntry {
            id: "stop-turn",
            label: "commands.stopTurn",
            detail: "commands.stopTurnDetail",
            run: stop_turn,
        });
    }
    rows
}

/// The measured trigger: the draft's last whitespace-delimited token, when it
/// begins with `/`, is the command token and its remainder is the query. A
/// slash that is not at a token's start (a URL) is not a trigger, and a space
/// after the token ends it.
pub(crate) fn slash_query(text: &str) -> Option<&str> {
    let token = text.rsplit(char::is_whitespace).next()?;
    token.strip_prefix('/')
}

/// The rows one query keeps, best first.
///
/// The reference's `filterOptions` was a trimmed, lowercased substring test;
/// this build splits the two fields by their nature: a NAME is matched as a
/// subsequence (the needle `ab` matches *a*b*, and the score orders the rows),
/// while a one-line DESCRIPTION is prose and matches only as a substring, so a
/// short query cannot drag in every row whose sentence happens to contain its
/// letters. A name match always outranks a description match, and a blank query
/// keeps every row in catalog order.
pub(crate) fn filter_indices(
    entries: &[CommandEntry],
    query: &str,
    lang: crate::i18n::Lang,
) -> Vec<usize> {
    let needle = query.trim();
    if needle.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut label_hits: Vec<(usize, i64)> = Vec::new();
    let mut detail_hits: Vec<usize> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match fuzzy::score(&lang.tr(entry.label), needle) {
            Some(score) => label_hits.push((index, score)),
            None => {
                // A description is prose: it matches as a substring, not as a
                // scattered subsequence, so a short query cannot drag in every
                // row whose one-liner happens to contain its letters.
                if !fuzzy::substring_runs(&lang.tr(entry.detail), needle).is_empty() {
                    detail_hits.push(index);
                }
            }
        }
    }
    label_hits.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    label_hits
        .into_iter()
        .map(|(index, _)| index)
        .chain(detail_hits)
        .collect()
}

/// One scored row, or none when the needle matches neither the label nor the
/// detail.
///
/// The label wins the score, so a row whose NAME matches is never described as
/// a description match; the detail is only tried when the label does not
/// match at all.
fn scored_row(
    id: String,
    label: String,
    detail: String,
    action: CardAction,
    needle: &str,
) -> Option<(i64, CardRow)> {
    if needle.is_empty() {
        return Some((
            0,
            CardRow {
                id,
                label,
                detail,
                action,
                label_runs: Vec::new(),
                detail_runs: Vec::new(),
            },
        ));
    }
    let (score, label_runs, detail_runs) = match fuzzy::hit(&label, needle) {
        Some((score, chars)) => (
            score,
            fuzzy::runs_in(&label, &chars, 0..label.len()),
            Vec::new(),
        ),
        None => {
            // A detail-only row is prose and ranks below every name match,
            // which is why its score is zero rather than a fuzzy one.
            let runs = fuzzy::substring_runs(&detail, needle);
            if runs.is_empty() {
                return None;
            }
            (0, Vec::new(), runs)
        }
    };
    Some((
        score,
        CardRow {
            id,
            label,
            detail,
            action,
            label_runs,
            detail_runs,
        },
    ))
}

/// Whether a watcher event names a path the index never lists.
///
/// Checked per path COMPONENT, not by substring: an event for the `.bsh`
/// directory itself (created when the ignore rules are first loaded) must not
/// trigger a rescan, and on Windows a path may spell the separator either way.
fn unindexed_path(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(
                ".bsh"
                    | ".bsh-run"
                    | ".codel"
                    | ".bos"
                    | ".git"
                    | ".jj"
                    | "target"
                    | "node_modules"
            )
        )
    })
}

/// Split a reference query into the directory scope and the needle.
///
/// `src/u` searches under `src` for `u`; `src/` browses `src`; a bare
/// `supal` searches the whole workspace. The scope is a PREFIX OF THE TEXT the
/// reader sees, which is what makes a drill something to backspace out of
/// instead of a hidden mode.
pub(crate) fn split_scope(query: &str) -> (&str, &str) {
    match query.rfind('/') {
        Some(index) => (&query[..index], &query[index + 1..]),
        None => ("", query),
    }
}

/// Which source the open card is showing. The reference's trigger component
/// takes one source at a time and swaps it as the trigger character changes:
/// `/` is the command list, `@` is the reference source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Trigger {
    /// The `+` control's own list, filtered by a `/` token.
    Command,
    /// `@file` / `@session` rows, filtered by an `@` token.
    Reference,
}

/// One drawn row. Both sources build these, so the card keeps one renderer and
/// one set of measured numbers.
#[derive(Clone)]
pub(crate) struct CardRow {
    /// Stable id, for the row's key and its debug selector.
    pub(crate) id: String,
    /// The row's label as drawn.
    pub(crate) label: String,
    /// The row's 11px detail as drawn.
    pub(crate) detail: String,
    /// What choosing the row does.
    pub(crate) action: CardAction,
    /// The byte ranges of `label` the query matched.
    pub(crate) label_runs: Vec<(usize, usize)>,
    /// The byte ranges of `detail` the query matched.
    pub(crate) detail_runs: Vec<(usize, usize)>,
}

/// The rows one trigger draws, plus what the status line needs to say.
pub(crate) struct CardRows {
    /// The rows, in draw order.
    pub(crate) rows: Vec<CardRow>,
    /// How many candidates matched before [`MAX_ROWS`] cut the list.
    pub(crate) matched: usize,
    /// Whether the row cap cut the list.
    pub(crate) truncated: bool,
}

impl CardRows {
    /// The drawn row count.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }
}

/// What choosing a row does.
#[derive(Clone)]
pub(crate) enum CardAction {
    /// Run a command row.
    Run(fn(&mut Shell, &mut Window, &mut Context<Shell>)),
    /// Replace the trigger token with this mention and close the card.
    Insert(String),
    /// Write one extension command's `/name ` annotation and close the card.
    ///
    /// The command's template is not inserted: the send composes it with the
    /// words the reader types after the annotation
    /// ([`crate::command_invocation`]).
    Extension(String),
    /// Replace the trigger token with this directory mention and keep the card
    /// open, so the next query lists inside that directory (the reference's
    /// `onPick` returning `continue: true`).
    Drill(String),
}

/// True when the byte at `index` starts a token.
fn starts_token(text: &str, index: usize) -> bool {
    text[..index]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

/// The reference source's own trigger: the `@` that starts the draft's active
/// token. Unlike a command token, a quoted mention keeps the spaces inside it
/// (`@"a file with spaces"`), which is what the reference's `quoted` flag means.
pub(crate) fn at_query(text: &str) -> Option<&str> {
    let index = text.rfind('@')?;
    starts_token(text, index).then(|| &text[index + 1..])
}

/// The byte index where the active trigger token starts, or 0 when the draft
/// carries none.
fn trigger_start(text: &str) -> usize {
    let slash = text.rfind('/').filter(|index| starts_token(text, *index));
    let at = text.rfind('@').filter(|index| starts_token(text, *index));
    match (slash, at) {
        (Some(slash), Some(at)) => slash.max(at),
        (Some(slash), None) => slash,
        (None, Some(at)) => at,
        (None, None) => 0,
    }
}

/// The mention text a row inserts: the reference's `value.mention`, quoted when
/// the name carries a space (its own `@"..."` form), with a trailing separator
/// for a directory so the next query lists inside it.
pub(crate) fn mention(name: &str, directory: bool) -> String {
    mention_with_range(name, directory, None)
}

/// The canonical session mention: a labelled reference whose hidden target is
/// the session id, like the reference's `@[label](dsh-session:...)` form.
pub(crate) fn session_mention(title: &str, id: &str) -> String {
    let label: String = title
        .chars()
        .map(|character| match character {
            '[' | ']' => ' ',
            other => other,
        })
        .collect();
    format!("@[{}](dsh-session:{id})", label.trim())
}

/// The same mention with an optional `#L<start>-L<end>` reference range, so a
/// reader can link an excerpt of a file rather than its whole body.
pub(crate) fn mention_with_range(name: &str, directory: bool, range: Option<(u32, u32)>) -> String {
    // One grammar for picks and for the text the model is told how to read.
    crate::reference::format_mention(name, directory, range, false).unwrap_or_default()
}

/// The menu's own state: the reference's controller keeps `open`, the live
/// search and the active index. bsh derives the search from the composer's own
/// text - which is what the reference filters against - so only the flag and
/// the active row live here.
#[derive(Default)]
pub(crate) struct CommandMenu {
    pub(crate) open: bool,
    pub(crate) trigger: Option<Trigger>,
    pub(crate) active: usize,
    /// The rows the last draft change (or index install) computed.
    ///
    /// A frame and a key press read THIS list. Neither may walk the
    /// filesystem: the reference source used to call the walk from `render`,
    /// from every arrow press and again from `run_row`.
    pub(crate) rows: Vec<CardRow>,
    /// How many candidates matched before [`MAX_ROWS`] cut the list.
    pub(crate) matched: usize,
    /// Whether [`MAX_ROWS`] cut the list.
    pub(crate) truncated: bool,
}

impl CommandMenu {
    /// The add control's own press, the reference's `onToggleCommandMenu`.
    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        self.trigger = self.open.then_some(Trigger::Command);
        self.active = 0;
        self.rows.clear();
        self.matched = 0;
        self.truncated = false;
    }

    /// Closing clears the highlight, so the next open parks on the top row the
    /// way a fresh open does.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.trigger = None;
        self.active = 0;
        self.rows.clear();
        self.matched = 0;
        self.truncated = false;
    }

    /// Install the rows one query computed, keeping the highlight in range.
    pub(crate) fn install(&mut self, rows: CardRows) {
        if self.active >= rows.rows.len() {
            self.active = rows.rows.len().saturating_sub(1);
        }
        self.rows = rows.rows;
        self.matched = rows.matched;
        self.truncated = rows.truncated;
    }

    /// Follow the draft: the card shows the source whose trigger the draft's
    /// active token carries, and closes when there is none. The reference's
    /// trigger component opens on the character and closes with the token, so a
    /// `+`-opened card also closes once ordinary text is typed.
    ///
    /// A slash as the FIRST character belongs to the slash palette instead, so
    /// the plus card stands down there rather than stacking underneath it. A
    /// mid-draft slash token (`hello /set`) is still this card's trigger.
    pub(crate) fn sync(&mut self, text: &str) {
        if super::slash_palette::slash_palette_query(text).is_some() {
            self.close();
            return;
        }
        let command = slash_query(text).is_some();
        let reference = at_query(text)
            .is_some_and(|query| query.starts_with('"') || !query.contains(char::is_whitespace));
        let reference_later = match (
            text.rfind('/').filter(|index| starts_token(text, *index)),
            text.rfind('@').filter(|index| starts_token(text, *index)),
        ) {
            (Some(slash), Some(at)) => at > slash,
            (None, Some(_)) => true,
            _ => false,
        };
        self.trigger = match (command, reference) {
            (true, true) => Some(if reference_later {
                Trigger::Reference
            } else {
                Trigger::Command
            }),
            (true, false) => Some(Trigger::Command),
            (false, true) => Some(Trigger::Reference),
            (false, false) => None,
        };
        self.open = self.trigger.is_some();
        if !self.open {
            self.active = 0;
        }
    }

    /// The reference highlights the row the pointer enters.
    pub(crate) fn set_active(&mut self, index: usize) {
        self.active = index;
    }
}

/// The add control's action: open or close the menu.
pub(crate) fn toggle_action(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    // One overlay at a time: pressing plus while the slash palette is open
    // closes it, so the two cards can never be on screen together.
    shell.slash.close();
    shell.command_menu.toggle();
    // The card's keys are intercepted only while the composer holds them, so the
    // press has to put them there. A click on the + control focuses the BUTTON,
    // which left the arrows doing nothing at all on this path - the card opened
    // by the + could only be driven by pointer, while the one opened by `@` was
    // already keyboard-driven because that path focuses the field itself.
    if shell.command_menu.open {
        let handle = shell.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }
    shell.sync_file_index(cx);
    shell.refresh_command_rows(cx);
    cx.notify();
}

impl Shell {
    /// The command card, drawn only while the menu is open. The empty search
    /// keeps every row and the first row starts active, as the reference's own
    /// open does.
    pub(crate) fn command_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let trigger = self.command_menu.trigger?;
        let lang = self.state.lang;
        // The rows the last draft/index change computed: a frame never walks
        // the filesystem, and never rebuilds a label it already has.
        let rows = &self.command_menu.rows;
        // A filter that shrank under the highlight parks it back on the top.
        let active = if rows.is_empty() {
            0
        } else {
            self.command_menu.active.min(rows.len() - 1)
        };
        // A first build has nothing to list yet. "Indexing…" is the status
        // line's job; the empty copy would contradict it.
        let building = trigger == Trigger::Reference
            && self.file_index.status() == IndexStatus::Building
            && self.file_index.entries().is_empty();
        let mut viewport = div()
            .id("commands-viewport")
            .debug_selector(|| "commands-viewport".to_string())
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .overflow_y_scroll();
        if rows.is_empty() && !building {
            viewport = viewport.child(
                div()
                    .debug_selector(|| "commands-empty".to_string())
                    .px(px(STATUS_PAD_X))
                    .py(px(STATUS_PAD_Y))
                    .text_size(px(STATUS_FONT))
                    .text_color(rgb(theme::faint()))
                    .child(lang.tr("commands.empty")),
            );
        }
        for (row, card_row) in rows.iter().enumerate() {
            let id = card_row.id.clone();
            let selected = row == active;
            let mut row_el = div()
                .id(format!("command-row-{id}"))
                .debug_selector({
                    let id = id.clone();
                    move || format!("command-row-{id}")
                })
                .flex()
                .items_center()
                .gap(px(ROW_GAP))
                .px(px(ROW_PAD_X))
                .py(px(ROW_PAD_Y))
                .rounded(px(ROW_RADIUS))
                .cursor_pointer()
                .text_size(px(ROW_FONT))
                .text_color(rgb(theme::text()))
                .on_mouse_move(cx.listener(move |this, _event, _window, cx| {
                    this.command_menu.set_active(row);
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _event, window, cx| {
                    this.run_row(row, window, cx);
                }))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(LABEL_GAP))
                        .min_w(px(0.0))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(super::marked_text(&card_row.label, &card_row.label_runs)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(DETAIL_FONT))
                        .text_color(rgb(theme::faint()))
                        .text_ellipsis()
                        .child(super::marked_text(&card_row.detail, &card_row.detail_runs)),
                );
            if selected {
                row_el = row_el.bg(rgb(theme::hover()));
            }
            viewport = viewport.child(row_el);
        }
        let status = self.command_status();
        let mut card = div()
            .debug_selector(|| "composer-command-menu".to_string())
            .absolute()
            // Measured `.card{bottom:calc(100% + 4px)}`: ABOVE the composer
            // card, not 4px off its own bottom, which put the list over the
            // field it belongs to.
            .bottom(relative(1.0))
            .mb(px(CARD_GAP))
            .left(px(0.0))
            .right(px(0.0))
            .min_w(px(CARD_MIN_W))
            .max_h(px(CARD_MAX_H))
            .flex()
            .flex_col()
            .p(px(CARD_PAD))
            .rounded(px(16.0))
            .bg(rgb(theme::page()))
            .shadow_lg()
            .occlude()
            .child(viewport);
        if let Some(status) = status {
            card = card.child(
                div()
                    .debug_selector(|| "commands-status".to_string())
                    .px(px(STATUS_PAD_X))
                    .py(px(2.0))
                    .text_size(px(STATUS_FONT))
                    .text_color(rgb(theme::faint()))
                    .child(status),
            );
        }
        Some(card.into_any_element())
    }

    /// What the card's status line says, when it has something true to say.
    ///
    /// The line exists for the two states a list cannot show by itself: a first
    /// build that has not finished ("Indexing…"), and a scan that failed. The
    /// match count appears only while a query is present, so a clean browse is
    /// not decorated with a number nobody asked for.
    fn command_status(&self) -> Option<SharedString> {
        if self.command_menu.trigger != Some(Trigger::Reference) {
            return None;
        }
        let lang = self.state.lang;
        if self.file_index.is_building() && self.file_index.entries().is_empty() {
            return Some(lang.tr("commands.indexing").into());
        }
        if self.file_index.status() == IndexStatus::Failed {
            return Some(lang.tr("commands.indexUnavailable").into());
        }
        let query = at_query(&self.composer_value).unwrap_or("");
        if query.trim_start_matches('"').trim().is_empty() || self.command_menu.rows.is_empty() {
            return None;
        }
        let count = self.command_menu.matched;
        if self.command_menu.truncated || self.file_index.truncated() {
            return Some(
                crate::i18n::tr_args(
                    lang,
                    "commands.matchCapped",
                    &[
                        ("shown", self.command_menu.rows.len().to_string()),
                        ("count", count.to_string()),
                    ],
                )
                .into(),
            );
        }
        let key = if count == 1 {
            "commands.matchOne"
        } else {
            "commands.matchCount"
        };
        Some(crate::i18n::tr_args(lang, key, &[("count", count.to_string())]).into())
    }

    /// Enter over an open menu runs the highlighted row instead of sending, and
    /// a row that names a mention consumes its own trigger token the way the
    /// reference's command does.
    pub(crate) fn run_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(_trigger) = self.command_menu.trigger else {
            return;
        };
        // The drawn row, not a recomputation: the query may have moved since the
        // last frame, but the row the reader pressed is the row that runs.
        let Some(card_row) = self.command_menu.rows.get(row).cloned() else {
            cx.notify();
            return;
        };
        match card_row.action {
            CardAction::Run(run) => {
                self.command_menu.close();
                self.cut_command_token("", window, cx);
                run(self, window, cx);
            }
            CardAction::Insert(text) => {
                self.command_menu.close();
                self.cut_command_token(&format!("{text} "), window, cx);
            }
            CardAction::Extension(name) => {
                self.insert_extension_command(&name, window, cx);
            }
            CardAction::Drill(text) => {
                self.cut_command_token(&text, window, cx);
                self.command_menu.open = true;
                self.command_menu.trigger = Some(Trigger::Reference);
                self.command_menu.active = 0;
                // The scope is the draft now, so the next query needs its rows
                // recomputed from the folder that was just entered.
                self.sync_file_index(cx);
                self.refresh_command_rows(cx);
            }
        }
        cx.notify();
    }

    /// The File row's action: link a workspace file (or a line range of one).
    ///
    /// It opens the SAME `@` reference card the reader gets by typing `@`,
    /// with a bare `@` token in the draft, so one picker and one grammar own
    /// every file reference. The card's pick then replaces the `@` with the
    /// canonical mention; a range typed after the path (`#L12-L20`) travels
    /// with it.
    pub(crate) fn begin_file_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_composer_text("@", window, cx);
        self.command_menu.open = true;
        self.command_menu.trigger = Some(Trigger::Reference);
        self.command_menu.active = 0;
        let handle = self.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.sync_file_index(cx);
        self.refresh_command_rows(cx);
        cx.notify();
    }

    /// Write one extension command's `/name ` annotation into the draft.
    ///
    /// The annotation is the COMMAND, not its template: the template is the
    /// prompt body and belongs to the request the send composes. Three draft
    /// shapes are handled, so a picked row is never a silent no-op:
    ///
    /// - the `/query` the row was picked under is replaced by the annotation;
    /// - an empty draft becomes the annotation plus the caret;
    /// - words the reader already typed - the `+` card can open over a draft -
    ///   are kept AFTER the annotation, because a command token resolves only
    ///   at the start of a message.
    pub(crate) fn insert_extension_command(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_menu.close();
        self.slash.close();
        let annotation = crate::command_invocation::draft(name);
        let text = self.composer_value.clone();
        let token = text.rsplit(char::is_whitespace).next().unwrap_or_default();
        let remainder = if token.starts_with('/') {
            text[..text.len() - token.len()].trim()
        } else {
            text.trim()
        };
        let next = if remainder.is_empty() {
            annotation
        } else {
            format!("{annotation}{remainder}")
        };
        // The reader continues typing the command's input, so the composer owns
        // the keys again and the caret parks after the annotation (a multi-line
        // editor resets its selection to 0..0 on a write while unfocused).
        let handle = self.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.set_composer_text(&next, window, cx);
    }

    /// Replace the draft's active trigger token with `append`, keeping what
    /// preceded it. A draft whose active token is not a trigger is left alone.
    fn cut_command_token(&mut self, append: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer_value.clone();
        let start = trigger_start(&text);
        if !matches!(text[start..].chars().next(), Some('/') | Some('@')) {
            return;
        }
        let next = format!("{}{append}", &text[..start]);
        // The same write the slash rows use, so the caret follows the inserted
        // mention instead of landing in front of it.
        self.set_composer_text(&next, window, cx);
    }

    /// The rows the active source produces for the draft's own query.
    pub(crate) fn card_rows(&self, trigger: Trigger) -> CardRows {
        match trigger {
            Trigger::Command => self.command_rows(),
            Trigger::Reference => self.reference_rows(),
        }
    }

    /// The plus card's rows: the built-in commands and the extension commands,
    /// ranked together by one fuzzy score.
    fn command_rows(&self) -> CardRows {
        let entries = entries(self.state.active_streaming());
        let query = slash_query(&self.composer_value).unwrap_or("");
        let needle = query.trim();
        let lang = self.state.lang;
        let mut scored: Vec<(i64, CardRow)> = Vec::new();
        for index in filter_indices(&entries, query, lang) {
            let entry = &entries[index];
            let label = lang.tr(entry.label);
            let detail = lang.tr(entry.detail);
            if let Some(row) = scored_row(
                entry.id.to_string(),
                label,
                detail,
                CardAction::Run(entry.run),
                needle,
            ) {
                scored.push(row);
            }
        }
        for command in &self.extension_commands {
            if let Some(row) = scored_row(
                format!("ext-command-{}", command.name),
                command.name.clone(),
                command.description.clone(),
                CardAction::Extension(command.name.clone()),
                needle,
            ) {
                scored.push(row);
            }
        }
        if !needle.is_empty() {
            // The built-in rows and the extension rows are one ranking: the
            // best row is first whichever source owns it.
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        CardRows {
            matched: scored.len(),
            truncated: false,
            rows: scored.into_iter().map(|(_, row)| row).collect(),
        }
    }

    /// The reference card's rows: the file block (folders tied first), then the
    /// session block, both ranked by the same fuzzy score.
    fn reference_rows(&self) -> CardRows {
        let query = at_query(&self.composer_value).unwrap_or("");
        let quoted = query.starts_with('"');
        let raw = query.trim_start_matches('"');
        // A line-range suffix is part of the reference, not part of the path it
        // matches: the query lists files by their path and the pick inserts the
        // range back.
        let (path_query, range) = crate::reference::split_range(raw);
        let (scope, needle) = split_scope(path_query);
        let mut rows: Vec<CardRow> = Vec::new();
        let mut matched: usize;
        let mut truncated = false;

        // The file block. A blank needle BROWSES the scope's own children;
        // anything else searches the whole subtree, so a nested file is found
        // by its own name without the reader typing the folders above it.
        if needle.is_empty() {
            let children = self.file_index.children(scope);
            matched = children.len();
            for (index, entry) in children.into_iter().enumerate() {
                if rows.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                rows.push(self.reference_row(index, entry, range, &[]));
            }
        } else {
            let candidates: Vec<&Entry> = self.file_index.under(scope).collect();
            let mut hits = fuzzy::rank(needle, candidates.iter().map(|entry| entry.rel.as_str()));
            matched = hits.len();
            // Score first; on an equal score a folder wins, then the filtered
            // list's own order (the index is sorted by path, so this is
            // deterministic without a second sort key).
            hits.sort_by(|left, right| {
                right
                    .score
                    .cmp(&left.score)
                    .then_with(|| candidates[right.index].dir.cmp(&candidates[left.index].dir))
                    .then_with(|| left.index.cmp(&right.index))
            });
            for (index, hit) in hits.into_iter().enumerate() {
                if rows.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                rows.push(self.reference_row(index, candidates[hit.index], range, &hit.chars));
            }
        }

        if quoted {
            return CardRows {
                rows,
                matched,
                truncated,
            };
        }

        // The session block: the same matcher over the titles, so a session is
        // found by a partial title exactly as a path is found by a partial path.
        let sessions: Vec<&crate::model::Session> = self
            .state
            .store
            .sessions
            .iter()
            .filter(|session| session.archived_at.is_none())
            .collect();
        if needle.is_empty() {
            matched += sessions.len();
            for (index, session) in sessions.into_iter().enumerate() {
                if rows.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                rows.push(self.session_row(index, session, &[]));
            }
        } else {
            let ranked = fuzzy::rank(
                path_query,
                sessions.iter().map(|session| session.title.as_str()),
            );
            matched += ranked.len();
            for (index, hit) in ranked.into_iter().enumerate() {
                if rows.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                rows.push(self.session_row(index, sessions[hit.index], &hit.chars));
            }
        }
        CardRows {
            rows,
            matched,
            truncated,
        }
    }

    /// One file row: the drawn label, the parent path, and the mention it
    /// inserts (or the drill it starts).
    fn reference_row(
        &self,
        index: usize,
        entry: &Entry,
        range: Option<(u32, u32)>,
        chars: &[usize],
    ) -> CardRow {
        let name = entry.name();
        let parent = entry.parent();
        let label = if entry.dir {
            format!("{name}/")
        } else {
            name.to_string()
        };
        let detail = match range {
            Some((start, end)) if !entry.dir && !parent.is_empty() => {
                format!("{parent} · L{start}-{end}")
            }
            Some((start, end)) if !entry.dir => format!("L{start}-{end}"),
            _ => parent.to_string(),
        };
        // The match is against the full relative path; split its character
        // indices across the label (the basename) and the detail (the parent
        // path) so each drawn piece highlights only the characters it shows.
        let name_start = entry.rel.len() - name.len();
        let label_runs = fuzzy::runs_in(&entry.rel, chars, name_start..entry.rel.len());
        let detail_end = parent.len();
        let detail_runs = fuzzy::runs_in(&entry.rel, chars, 0..detail_end);
        CardRow {
            id: format!("ref-file-{index}"),
            label,
            detail,
            action: if entry.dir {
                CardAction::Drill(mention(&entry.rel, true))
            } else {
                CardAction::Insert(mention_with_range(&entry.rel, false, range))
            },
            label_runs,
            detail_runs,
        }
    }

    /// One session row: a title, its workspace, and the labelled mention.
    fn session_row(
        &self,
        index: usize,
        session: &crate::model::Session,
        chars: &[usize],
    ) -> CardRow {
        let label = session.title.clone();
        CardRow {
            id: format!("ref-session-{index}"),
            label_runs: fuzzy::runs_in(&label, chars, 0..label.len()),
            label,
            detail: self.workspace_name(&session.workspace),
            // A session is NOT a path: the reference serializes it as a
            // labelled mention, which the file guidance excludes from the
            // path rule.
            action: CardAction::Insert(session_mention(&session.title, &session.id)),
            detail_runs: Vec::new(),
        }
    }

    /// The display name of the workspace a row belongs to.
    fn workspace_name(&self, id: &str) -> String {
        self.state
            .settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| workspace.name.clone())
            .unwrap_or_default()
    }

    /// Escape on the reference card: close it and take its trigger token out of
    /// the draft.
    ///
    /// The picker owns the `@query` it was opened with, so dismissing it must
    /// leave no picker text in the composer: `hello @alp` becomes `hello`, and
    /// a lone `@alp` becomes empty. A `+`-opened card has no token to remove,
    /// and a `/` token belongs to the slash palette's own Escape, so only an
    /// `@` token is consumed.
    pub(crate) fn dismiss_command_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_menu.close();
        let text = self.composer_value.clone();
        let start = trigger_start(&text);
        if text[start..].starts_with('@') {
            let next = text[..start].trim_end().to_string();
            self.set_composer_text(&next, window, cx);
        }
        self.sync_file_index(cx);
        cx.notify();
    }

    /// Recompute the open card's rows from the current draft and index.
    ///
    /// This is the ONE place rows are built outside a test. The composer's
    /// change listener calls it, an installed index calls it, and a drill calls
    /// it; a frame and a key press never do.
    pub(crate) fn refresh_command_rows(&mut self, cx: &mut Context<Self>) {
        let Some(trigger) = self.command_menu.trigger else {
            self.command_menu.rows.clear();
            self.command_menu.matched = 0;
            self.command_menu.truncated = false;
            return;
        };
        let rows = self.card_rows(trigger);
        self.command_menu.install(rows);
        cx.notify();
    }

    /// Keep the reference card's workspace index and watcher in step with the
    /// draft.
    ///
    /// Starting the index is a SIDE EFFECT of the card opening, not of a
    /// frame: this runs from the composer's change listener, from the File
    /// row, from a drill, and from the index's own completion.
    pub(crate) fn sync_file_index(&mut self, cx: &mut Context<Self>) {
        let wanted =
            self.command_menu.open && self.command_menu.trigger == Some(Trigger::Reference);
        let Some(root) = self.state.workspace_root().filter(|_| wanted) else {
            self.stop_index_watch();
            return;
        };
        if let Some(generation) = self.file_index.begin_if_needed(&root) {
            self.start_index_build(root.clone(), generation, cx);
        }
        self.start_index_watch(root, cx);
    }

    /// Scan the workspace on a worker thread and install the result.
    ///
    /// The walk is blocking filesystem work, so it never runs on the UI thread
    /// or on GPUI's executor: a plain thread owns it and an async_channel
    /// carries the result back. The generation is checked on install, so a
    /// result for an older root or an older build cannot replace a newer one.
    fn start_index_build(&mut self, root: PathBuf, generation: u64, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.index_builds += 1;
        }
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(crate::workspace_index::scan(&root));
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(scan) => this.file_index.install(generation, scan),
                    Err(error) => this.file_index.fail(generation, error),
                }
                this.refresh_command_rows(cx);
            });
        });
        self.index_build = Some(task);
    }

    /// Watch the workspace while the reference card is open.
    ///
    /// The Files pane's watcher is the pattern: notify's own thread sends, a
    /// foreground task debounces and rescans. Only the card's lifetime pays for
    /// it, so a workspace the reader is not picking from is not watched.
    fn start_index_watch(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.index_watch.is_some() && self.index_watch_root.as_deref() == Some(root.as_path()) {
            return;
        }
        self.stop_index_watch();
        let (tx, rx) = async_channel::unbounded();
        let watch_root = root.clone();
        let Ok(mut watcher) =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                // Only components BELOW the workspace decide: a workspace that
                // happens to live under a folder called target is still watched.
                let relevant = event.paths.iter().any(|path| {
                    let relative = path.strip_prefix(&watch_root).unwrap_or(path);
                    !unindexed_path(relative)
                });
                if relevant {
                    let _ = tx.send_blocking(());
                }
            })
        else {
            return;
        };
        if watcher
            .watch(&root, notify::RecursiveMode::Recursive)
            .is_err()
        {
            return;
        }
        self.index_watch = Some(watcher);
        self.index_watch_root = Some(root);
        self.index_watch_generation = self.index_watch_generation.wrapping_add(1);
        let generation = self.index_watch_generation;
        let close_rx = rx.clone();
        let task = cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                while rx.try_recv().is_ok() {}
                let _ = this.update(cx, |this, cx| {
                    if this.index_watch_generation != generation {
                        return;
                    }
                    if !this.command_menu.open
                        || this.command_menu.trigger != Some(Trigger::Reference)
                    {
                        return;
                    }
                    let Some(root) = this.index_watch_root.clone() else {
                        return;
                    };
                    let generation = this.file_index.begin(&root);
                    this.start_index_build(root, generation, cx);
                });
            }
        });
        self.index_watch_task = Some(task);
        self.index_watch_close = Some(Box::new(move || {
            close_rx.close();
        }));
    }

    /// Stop the watcher and detach its channel before the task is cancelled.
    pub(crate) fn stop_index_watch(&mut self) {
        if let Some(close) = self.index_watch_close.take() {
            close();
        }
        self.index_watch_task = None;
        self.index_watch = None;
        self.index_watch_root = None;
    }
}
