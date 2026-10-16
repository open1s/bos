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

use super::shell::Shell;
use crate::theme;

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

/// The reference's `filterOptions`: a trimmed, lowercased substring test over
/// the row's own label and detail; a blank query keeps every row in order.
pub(crate) fn filter_indices(
    entries: &[CommandEntry],
    query: &str,
    lang: crate::i18n::Lang,
) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            query.is_empty()
                || lang.tr(entry.label).to_lowercase().contains(&query)
                || lang.tr(entry.detail).to_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
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
pub(crate) struct CardRow {
    /// Stable id, for the row's key and its debug selector.
    pub(crate) id: String,
    /// The row's label as drawn.
    pub(crate) label: String,
    /// The row's 11px detail as drawn.
    pub(crate) detail: String,
    /// What choosing the row does.
    pub(crate) action: CardAction,
}

/// What choosing a row does.
pub(crate) enum CardAction {
    /// Run a command row.
    Run(fn(&mut Shell, &mut Window, &mut Context<Shell>)),
    /// Replace the trigger token with this mention and close the card.
    Insert(String),
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
    let mut text = if name.contains(char::is_whitespace) {
        format!("@\"{name}\"")
    } else {
        format!("@{name}")
    };
    if directory {
        text.push('/');
    }
    text
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
}

impl CommandMenu {
    /// The add control's own press, the reference's `onToggleCommandMenu`.
    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        self.trigger = self.open.then_some(Trigger::Command);
        self.active = 0;
    }

    /// Closing clears the highlight, so the next open parks on the top row the
    /// way a fresh open does.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.trigger = None;
        self.active = 0;
    }

    /// Follow the draft: the card shows the source whose trigger the draft's
    /// active token carries, and closes when there is none. The reference's
    /// trigger component opens on the character and closes with the token, so a
    /// `+`-opened card also closes once ordinary text is typed.
    pub(crate) fn sync(&mut self, text: &str) {
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
pub(crate) fn toggle_action(shell: &mut Shell, _window: &mut Window, cx: &mut Context<Shell>) {
    shell.command_menu.toggle();
    cx.notify();
}

impl Shell {
    /// The command card, drawn only while the menu is open. The empty search
    /// keeps every row and the first row starts active, as the reference's own
    /// open does.
    pub(crate) fn command_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let trigger = self.command_menu.trigger?;
        let lang = self.state.lang;
        let rows = self.card_rows(trigger);
        // A filter that shrank under the highlight parks it back on the top.
        let active = if self.command_menu.active >= rows.len() {
            0
        } else {
            self.command_menu.active
        };
        let mut viewport = div()
            .id("commands-viewport")
            .debug_selector(|| "commands-viewport".to_string())
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .overflow_y_scroll();
        if rows.is_empty() {
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
                                .text_ellipsis()
                                .child(SharedString::from(card_row.label.clone())),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(DETAIL_FONT))
                        .text_color(rgb(theme::faint()))
                        .text_ellipsis()
                        .child(SharedString::from(card_row.detail.clone())),
                );
            if selected {
                row_el = row_el.bg(rgb(theme::hover()));
            }
            viewport = viewport.child(row_el);
        }
        Some(
            div()
                .debug_selector(|| "composer-command-menu".to_string())
                .absolute()
                .bottom(px(CARD_GAP))
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
                .child(viewport)
                .into_any_element(),
        )
    }

    /// Enter over an open menu runs the highlighted row instead of sending, and
    /// a row that names a mention consumes its own trigger token the way the
    /// reference's command does.
    pub(crate) fn run_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(trigger) = self.command_menu.trigger else {
            return;
        };
        let rows = self.card_rows(trigger);
        let Some(card_row) = rows.into_iter().nth(row) else {
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
            CardAction::Drill(text) => {
                self.cut_command_token(&text, window, cx);
                self.command_menu.open = true;
                self.command_menu.trigger = Some(Trigger::Reference);
                self.command_menu.active = 0;
            }
        }
        cx.notify();
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
        self.composer_value = next.clone();
        self.composer
            .update(cx, |state, cx| state.set_value(next, window, cx));
    }

    /// The rows the active source produces for the draft's own query.
    fn card_rows(&self, trigger: Trigger) -> Vec<CardRow> {
        match trigger {
            Trigger::Command => {
                let entries = entries(self.state.any_streaming());
                let query = slash_query(&self.composer_value).unwrap_or("");
                filter_indices(&entries, query, self.state.lang)
                    .into_iter()
                    .map(|index| CardRow {
                        id: entries[index].id.to_string(),
                        label: self.state.lang.tr(entries[index].label).to_string(),
                        detail: self.state.lang.tr(entries[index].detail).to_string(),
                        action: CardAction::Run(entries[index].run),
                    })
                    .collect()
            }
            Trigger::Reference => {
                let query = at_query(&self.composer_value).unwrap_or("");
                self.reference_rows(query)
            }
        }
    }

    /// The reference source's candidates in the reference's own order - the file
    /// rows, then the session rows - with the session half dropped for a quoted
    /// query, which can only name a file.
    fn reference_rows(&self, query: &str) -> Vec<CardRow> {
        let quoted = query.starts_with('"');
        let raw = query.trim_start_matches('"');
        let needle = raw.to_lowercase();
        let mut rows: Vec<CardRow> = Vec::new();
        for (index, (path, directory)) in self.reference_files(raw).into_iter().enumerate() {
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            rows.push(CardRow {
                id: format!("ref-file-{index}"),
                label: if directory { format!("{name}/") } else { name },
                detail: path.clone(),
                action: if directory {
                    CardAction::Drill(mention(&path, true))
                } else {
                    CardAction::Insert(mention(&path, false))
                },
            });
        }
        if quoted {
            return rows;
        }
        for (index, session) in self
            .state
            .store
            .sessions
            .iter()
            .filter(|session| session.archived_at.is_none())
            .filter(|session| needle.is_empty() || session.title.to_lowercase().contains(&needle))
            .enumerate()
        {
            rows.push(CardRow {
                id: format!("ref-session-{index}"),
                label: session.title.clone(),
                detail: self.workspace_name(&session.workspace),
                action: CardAction::Insert(mention(&session.title, false)),
            });
        }
        rows
    }

    /// The active workspace's filesystem root, the root the agent's own tools are
    /// confined to (the reference asks its remote `fileReferences` for the same
    /// list).
    fn workspace_root(&self) -> Option<PathBuf> {
        let settings = &self.state.settings;
        settings
            .workspaces
            .iter()
            .find(|workspace| workspace.id == settings.active_workspace)
            .map(|workspace| PathBuf::from(&workspace.root))
    }

    /// The display name of the workspace a row belongs to.
    fn workspace_name(&self, id: &str) -> String {
        self.state
            .settings
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| workspace.name.clone())
            .unwrap_or_default()
    }

    /// The workspace files the query names, bounded: three levels under the root,
    /// 200 rows, the usual heavy directories skipped, and a query that names a
    /// directory walked as a prefix. The reference's lookup is remote and fuzzy;
    /// this is the local backing for it.
    fn reference_files(&self, query: &str) -> Vec<(String, bool)> {
        const MAX_ROWS: usize = 200;
        const MAX_DEPTH: usize = 2;
        const SKIP: [&str; 6] = [".git", ".jj", "target", "node_modules", "dist", "build"];
        let Some(root) = self.workspace_root() else {
            return Vec::new();
        };
        let (start, needle) = match query.rfind('/') {
            Some(index) => (
                root.join(query[..index].trim_matches('"')),
                query[index + 1..].to_lowercase(),
            ),
            None => (root.clone(), query.to_lowercase()),
        };
        let mut rows: Vec<(String, bool)> = Vec::new();
        let mut stack = vec![(start, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if rows.len() >= MAX_ROWS {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut found: Vec<(PathBuf, bool)> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    let name = path.file_name()?.to_string_lossy().to_string();
                    let directory = path.is_dir();
                    if name.starts_with('.') || (directory && SKIP.contains(&name.as_str())) {
                        return None;
                    }
                    Some((path, directory))
                })
                .collect();
            // Directories first, then by name, so a fresh open reads like a tree.
            found.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            for (path, directory) in found {
                if rows.len() >= MAX_ROWS {
                    break;
                }
                let Ok(relative) = path.strip_prefix(&root) else {
                    continue;
                };
                let Some(relative) = relative.to_str() else {
                    continue;
                };
                let relative = relative.replace('\\', "/");
                let lowered = relative.to_lowercase();
                if !needle.is_empty() && !lowered.contains(&needle) {
                    continue;
                }
                let descend = directory && depth < MAX_DEPTH && lowered.starts_with(&needle);
                rows.push((relative, directory));
                if descend {
                    stack.push((path, depth + 1));
                }
            }
        }
        rows
    }
}
