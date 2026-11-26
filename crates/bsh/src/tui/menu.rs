//! The composer's completion popup: the `/` command list and the `@` file list.
//!
//! Both are the terminal's form of the desktop composer's two cards
//! (`ui::slash_palette` and `ui::command_menu`). The RULES are the desktop's
//! own, shared rather than re-derived: the same triggers, the same fuzzy
//! ranking ([`crate::fuzzy`]), the same mention grammar
//! ([`crate::reference::format_mention`]) and the same workspace index
//! ([`crate::workspace_index`]). Only the drawing differs, because a terminal
//! draws rows of text and a window draws a card.
//!
//! The list is a client-side affordance: it ranks names and inserts text, and
//! never sends a Command. Accepting a row leaves either completed text in the
//! composer or - for a command that runs with no arguments - the same line the
//! user would have typed, which [`super::app::App`] then submits.
//!
//! The `@` list is the desktop reference card's two blocks: the workspace's
//! files, then the sessions a reader can open. A file row inserts its mention;
//! a session row opens the conversation and takes the typed `@query` out of
//! the draft, exactly as the window's card does.
//!
//! A `/` list also carries the skills the Host advertises
//! ([`crate::runtime::contract::CatalogueView::skills`]), because the desktop
//! palette lists them beside its commands. The row inserts `/name ` and never
//! runs: the gesture is resolved by the Host at send, so a terminal turn loads
//! the same skill instructions a window's turn loads.
//!
//! The word a command's argument can be is the fourth list: `/plan ` offers `on`
//! and `off` before the reader has to remember them from `/help`, and `/goal p`
//! offers `pause` before anything else. Only a command that fixes its words gets
//! rows - a title, a path or a note is free text no catalogue can guess - and
//! the row replaces the word, not the line, so the argument is completed the
//! way a shell completes one word of a command line. The list closes once the
//! word is exactly one of the candidates: there is nothing left to choose, and
//! the next Enter belongs to the composer.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::fuzzy;
use crate::reference::format_mention;
use crate::runtime::contract::{PERMISSION_MODES, SkillView};
use crate::workspace_index::{self, Entry, STALE_AFTER};

/// The most rows one query keeps. Past this the highlight has nowhere useful
/// to go, and the desktop card cuts at the same order of magnitude.
const MAX_ROWS: usize = 50;

/// The rows a terminal window shows at once; the window follows the highlight.
pub(crate) const VISIBLE_ROWS: usize = 6;

/// The cap on one index build.
///
/// A terminal picker answers a typed prefix, so it never needs a whole
/// monorepo; the cap bounds the one blocking walk that opening the list costs.
const INDEX_CAP: usize = 50_000;

/// Which trigger opened the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The `/` that starts the composer's first token.
    Command,
    /// The first word of a command's argument, with the command it completes
    /// and the words that argument can be.
    Argument {
        /// The command whose argument is being typed, as it is drawn.
        name: &'static str,
        /// The words that argument can be.
        words: &'static [Word],
    },
    /// An `@` that starts a word.
    File,
}

/// The token the list belongs to: where it starts in the composer, and what
/// has been typed after the trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    source: Source,
    /// Byte offset of the trigger character.
    start: usize,
    /// The text typed after the trigger.
    query: String,
}

/// One row the reader can accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    /// The row's own name, as drawn.
    pub(crate) label: String,
    /// The one-line description, as drawn.
    pub(crate) detail: String,
    /// What accepting the row replaces the token with.
    pub(crate) insert: String,
    /// Whether accepting runs the line instead of leaving it in the composer.
    /// A command that needs arguments leaves the composer open; a file
    /// mention never runs anything.
    pub(crate) submit: bool,
    /// The session a row opens, when the row is a session rather than a
    /// mention. The desktop card's session rows switch the conversation;
    /// they do not insert text.
    pub(crate) session: Option<String>,
}

/// One session the `@` list offers, as a row a reader can open.
///
/// The terminal keeps the same fields the desktop card draws: the title it
/// ranks, the workspace it names, and the id the switch sends.
pub(crate) struct SessionChoice {
    /// Session identity, as the Host names it.
    pub(crate) id: String,
    /// The title the query ranks and the row draws.
    pub(crate) title: String,
    /// The workspace's display name, drawn as the row's detail.
    pub(crate) workspace: String,
}

/// What accepting a row did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Accepted {
    /// Text landed in the composer; `true` runs the line now.
    Insert(bool),
    /// The row named a session: the token is gone, and the conversation this
    /// id names should open.
    Session(String),
}

/// One word a command's argument can be, with what picking it does.
///
/// The gloss is drawn beside the word, the way the desktop's dialog explains a
/// permission beside its row: `danger-full-access` names nothing a reader can
/// weigh without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Word {
    /// The word as the parser accepts it, exactly.
    text: &'static str,
    /// The one-line gloss drawn beside it.
    detail: &'static str,
}

/// The words a command's first argument can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arguments {
    /// The command takes no argument at all.
    None,
    /// Free text, or a name only the Host can enumerate: a title, a path, a
    /// note, a numeric id. There is nothing to rank, so no list opens.
    Free,
    /// A fixed vocabulary. Every word here is a whole argument - picking one
    /// completes the line, so the row runs it.
    Words(&'static [Word]),
}

/// One command the terminal backs, with the facts the list needs.
struct Command {
    /// The name as typed, slash included.
    name: &'static str,
    detail: &'static str,
    /// Whether the command does something useful with no arguments.
    bare: bool,
    /// What the command's first argument can be.
    arguments: Arguments,
}

/// The commands this build backs, in name order so a blank query is stable.
///
/// It is the TUI's own catalogue rather than a copy of the desktop's: every
/// row here is a line `SlashCommand::parse` accepts, so the list never offers
/// a command the terminal cannot run.
const COMMANDS: &[Command] = &[
    Command {
        name: "/approve",
        detail: "approve the approval a turn is waiting on · `^Y`",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/audit",
        detail: "audit this session's tool calls, and what was refused",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/browser",
        detail: "the page the agent's browser tools read",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/compact",
        detail: "fold older history into one checkpoint",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/copy",
        detail: "copy the newest answer, or `id` for the session's id, to the clipboard",
        bare: true,
        arguments: Arguments::Words(&[Word {
            text: "id",
            detail: "the session's id, to paste into a report",
        }]),
    },
    Command {
        name: "/fork",
        detail: "branch this session into a chat that opens with its transcript",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/detail",
        detail: "read one tool call's arguments and output",
        bare: true,
        arguments: Arguments::Words(&[Word {
            text: "close",
            detail: "back to the transcript",
        }]),
    },
    Command {
        name: "/diff",
        detail: "compare the working copy, or one file, with its baseline",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/deny",
        detail: "refuse it, with a reason the audit records · `^R`",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/editor",
        detail: "write the prompt in $EDITOR, and take the buffer back · `^X`",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/export",
        detail: "save this session as Markdown",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/extensions",
        detail: "list the installed extensions, or search them",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/federation",
        detail: "the node this build runs and the peers it trusts",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/feedback",
        detail: "record or read this session's note",
        bare: true,
        arguments: Arguments::Words(&[Word {
            text: "clear",
            detail: "drop the note",
        }]),
    },
    Command {
        name: "/files",
        detail: "browse the workspace, or read one file",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/find",
        detail: "reveal a previous row of this conversation by a word it carries",
        bare: false,
        arguments: Arguments::Free,
    },
    Command {
        name: "/goal",
        detail: "show or change the session goal",
        bare: false,
        arguments: Arguments::Words(&[
            Word {
                text: "clear",
                detail: "drop the goal",
            },
            Word {
                text: "pause",
                detail: "stop it, keeping it",
            },
            Word {
                text: "resume",
                detail: "arm it again",
            },
        ]),
    },
    Command {
        name: "/help",
        detail: "list the commands and keys",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/mcp",
        detail: "list the connected MCP servers, or one server's tools",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/memory",
        detail: "browse or search what this workspace remembers",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/model",
        detail: "pick the model from a list, or name `<provider> <model>`",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/new",
        detail: "create a session, titled if one is given",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/permission",
        detail: "pick the approval mode from a list",
        bare: true,
        // The contract's own three modes, in the order it names them: the
        // parser refuses any other word, so the list completes only these.
        arguments: Arguments::Words(&[
            Word {
                text: PERMISSION_MODES[0],
                detail: "reads, and asks before it writes",
            },
            Word {
                text: PERMISSION_MODES[1],
                detail: "writes inside the workspace",
            },
            Word {
                text: PERMISSION_MODES[2],
                detail: "runs without asking",
            },
        ]),
    },
    Command {
        name: "/skills",
        detail: "list the registered skills, or search them",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/plan",
        detail: "toggle plan mode",
        bare: true,
        arguments: Arguments::Words(&[
            Word {
                text: "on",
                detail: "plan before acting",
            },
            Word {
                text: "off",
                detail: "act directly",
            },
        ]),
    },
    Command {
        name: "/preset",
        detail: "pick the preset from a list",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/queue",
        detail: "the lines waiting for the running turn",
        bare: true,
        arguments: Arguments::Words(&[Word {
            text: "clear",
            detail: "drop the waiting lines",
        }]),
    },
    Command {
        name: "/restore",
        detail: "take one parked session back into the lists",
        bare: false,
        arguments: Arguments::Free,
    },
    Command {
        name: "/purge",
        detail: "drop one parked session for good",
        bare: false,
        arguments: Arguments::Free,
    },
    Command {
        name: "/rename",
        detail: "rename this session",
        bare: false,
        arguments: Arguments::Free,
    },
    Command {
        name: "/questions",
        detail: "bring back the model's unanswered question, or say there is none",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/review",
        detail: "the files the last turn changed, or one change's diff",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/sessions",
        detail: "open the session list, filter a chat by name, or pin/archive one by number",
        bare: true,
        // Only the word that stands alone: `pin`, `archive` and their inverses
        // take the number the listing printed, so a completed line would still
        // be half a command.
        arguments: Arguments::Words(&[Word {
            text: "archived",
            detail: "the chats archived here",
        }]),
    },
    Command {
        name: "/settings",
        detail: "the settings a turn resolves from, section by section",
        bare: true,
        arguments: Arguments::Free,
    },
    // The write side of the rail above: one field, addressed by the key its row
    // carries, so a terminal changes a setting the way the dialog does instead
    // of editing the document by hand.
    Command {
        name: "/set",
        detail: "write or clear one settings field the Host marks editable",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/subagents",
        detail: "the child runs this conversation started; a number opens one, `all` every run",
        bare: true,
        arguments: Arguments::Words(&[Word {
            text: "all",
            detail: "every child run the Host holds",
        }]),
    },
    Command {
        name: "/trajectory",
        detail: "this session's typed event log: turns, messages and calls in order",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/tasks",
        detail: "list the workspace's tasks, open one by id, or `graph` for the tree",
        bare: true,
        arguments: Arguments::Words(&[Word {
            text: "graph",
            detail: "the task tree",
        }]),
    },
    Command {
        name: "/terminal",
        detail: "open a shell at the workspace root; Ctrl-] closes it",
        bare: true,
        arguments: Arguments::None,
    },
    Command {
        name: "/trash",
        detail: "open the bin, or park a session by its /sessions number",
        bare: true,
        arguments: Arguments::Free,
    },
    Command {
        name: "/usage",
        detail: "tokens this session's turns reported",
        bare: true,
        arguments: Arguments::None,
    },
];

/// The rows this catalogue offers, so a test can prove the terminal runs every
/// row the list draws: the name as typed, and whether accepting it runs the
/// line outright.
///
/// Only tests read this: the list itself is drawn from [`COMMANDS`], and a row
/// the parser does not accept would be a promise the list cannot keep. The
/// `bare` half is what makes that check honest - a row that runs on accept has
/// to run when the same line is typed, while a row that leaves the composer
/// open for an argument is allowed to answer with its usage line.
#[cfg(test)]
pub(crate) fn command_rows_for_test() -> Vec<(&'static str, bool)> {
    COMMANDS
        .iter()
        .map(|command| (command.name, command.bare))
        .collect()
}

/// Every word a command's argument can be, as `(command, word)`, so a test can
/// prove the list completes only lines the parser runs: a completed word is
/// inserted and the line sends, so a word the parser refuses would be a row
/// that answers with its usage line.
#[cfg(test)]
pub(crate) fn command_arguments_for_test() -> Vec<(&'static str, &'static str)> {
    COMMANDS
        .iter()
        .flat_map(|command| {
            let Arguments::Words(words) = command.arguments else {
                return Vec::new();
            };
            words.iter().map(|word| (command.name, word.text)).collect()
        })
        .collect()
}

/// Every command name this build backs, in the catalogue's own order.
///
/// The popup, the `/help` line and the answer to a mistyped line all read this
/// one list, so a suggestion can never name a row the popup does not draw.
pub(crate) fn command_names() -> Vec<&'static str> {
    COMMANDS.iter().map(|command| command.name).collect()
}

/// One workspace's candidate list, with the walk that produced it.
struct Files {
    root: PathBuf,
    entries: Vec<Entry>,
    /// Whether the walk stopped at [`INDEX_CAP`], so the list is incomplete.
    truncated: bool,
    built: Instant,
}

/// The popup's state: whether it is open, on which token, and where the
/// highlight is.
#[derive(Default)]
pub(crate) struct Menu {
    open: Option<Token>,
    rows: Vec<Row>,
    active: usize,
    files: Option<Files>,
}

impl Menu {
    /// Follow the composer: open onto the token under the cursor, or close.
    ///
    /// The rows are computed here, once per edit, never in `render`: a frame
    /// must not walk a filesystem or rank a catalogue, the way the desktop's
    /// card also keeps its rows out of its draw call.
    pub(crate) fn sync(
        &mut self,
        input: &str,
        cursor: usize,
        root: Option<&Path>,
        skills: &[SkillView],
        sessions: &[SessionChoice],
    ) {
        let Some(token) = token(input, cursor) else {
            self.close();
            return;
        };
        let rows = match token.source {
            Source::Command => command_rows(&token.query, skills),
            Source::Argument { name, words } => argument_rows(&token.query, name, words),
            Source::File => file_rows(&token.query, root, &mut self.files, sessions),
        };
        // An empty list closes the popup rather than leaving a marker with no
        // candidate under it.
        if rows.is_empty() {
            self.close();
            return;
        }
        // A new token starts at the top; walking inside one keeps the highlight.
        if self.open.as_ref() != Some(&token) {
            self.active = 0;
        }
        self.open = Some(token);
        self.rows = rows;
    }

    /// Close the popup, dropping the highlight.
    pub(crate) fn close(&mut self) {
        self.open = None;
        self.rows.clear();
        self.active = 0;
    }

    /// Whether a list is showing.
    pub(crate) fn open(&self) -> bool {
        self.open.is_some()
    }

    /// The rows a frame draws.
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The highlighted row.
    pub(crate) fn active(&self) -> usize {
        self.active
    }

    /// The slice of rows a frame draws, as `(start, visible)`.
    ///
    /// The window follows the highlight, keeping it on the last row while there
    /// are more candidates below, the way a scrolling menu does. A frame and a
    /// click both ask here, so the row a pointer names is the row that pointer
    /// saw.
    pub(crate) fn window(&self) -> (usize, usize) {
        let visible = self.rows.len().min(VISIBLE_ROWS);
        let start = self.active.saturating_sub(visible.saturating_sub(1));
        (start, visible)
    }

    /// Put the highlight on a row, and report whether the list has that row.
    ///
    /// A pointer can name any drawn row, so this is the walk's own clamp with
    /// the target chosen outright: a click on the last row lands where holding
    /// Down would have.
    pub(crate) fn select(&mut self, index: usize) -> bool {
        if index >= self.rows.len() {
            return false;
        }
        self.active = index;
        true
    }

    /// Move the highlight, clamped at both ends: the first press does not wrap
    /// to the last row the way a menu's wheel would.
    pub(crate) fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.active = self.active.saturating_add_signed(delta).min(last);
    }

    /// Whether the walk that filled the list was cut short.
    pub(crate) fn truncated(&self) -> bool {
        self.files.as_ref().is_some_and(|files| files.truncated)
    }

    /// Accept the highlighted row: replace the token with its text.
    ///
    /// Returns whether the completed line should run now, or `None` when there
    /// is no row to accept. The composer is edited in place, so the reader's
    /// cursor lands after the inserted text and the rest of the line survives.
    pub(crate) fn accept(&mut self, input: &mut String, cursor: &mut usize) -> Option<Accepted> {
        let token = self.open.clone()?;
        let row = self.rows.get(self.active)?.clone();
        let caret = (*cursor).min(input.len());
        if token.start > caret {
            return None;
        }
        // The token covers the word it belongs to, not the caret's column alone:
        // a reader who steps back into a half-typed word asks to replace that
        // word, so a completion never lands in its middle. A word ends where
        // whitespace begins, which is the rule that opened the token.
        let tail = &input[token.start..];
        let end = token.start + tail.find(char::is_whitespace).unwrap_or(tail.len());
        // A session is not a mention: the picker's query is a filter the
        // reader never meant to write, so it comes back out of the draft and
        // the conversation opens. The desktop's `consume_reference_token` cuts
        // everything from the `@` on - a tail typed after the picker belongs
        // to the picker too - and trims the whitespace it left behind, so the
        // draft keeps only the prose before it.
        if let Some(id) = row.session.clone() {
            let kept = input[..token.start].trim_end().len();
            input.truncate(kept);
            *cursor = kept;
            self.close();
            return Some(Accepted::Session(id));
        }
        input.replace_range(token.start..end, &row.insert);
        *cursor = token.start + row.insert.len();
        self.close();
        Some(Accepted::Insert(row.submit))
    }
}

/// The token under the cursor, if one is open.
///
/// The triggers are the desktop's: `/` only as the whole composer's first
/// character (a mid-draft slash is not a command), and `@` only where a word
/// starts, so an email address is not a mention. Whitespace ends both tokens.
fn token(input: &str, cursor: usize) -> Option<Token> {
    let head = &input[..cursor.min(input.len())];
    if let Some(query) = head.strip_prefix('/')
        && !query.contains(|c: char| c.is_whitespace())
    {
        return Some(Token {
            source: Source::Command,
            start: 0,
            query: query.to_string(),
        });
    }
    if let Some(token) = argument_token(head) {
        return Some(token);
    }
    let start = head.rfind('@')?;
    if !head[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
    {
        return None;
    }
    let query = &head[start + 1..];
    if query.contains(char::is_whitespace) {
        return None;
    }
    Some(Token {
        source: Source::File,
        start,
        query: query.to_string(),
    })
}

/// The word after a command whose argument comes from a fixed vocabulary.
///
/// Only the first argument word is completed: a second word is another decision
/// (`/goal edit <objective>`), and a command whose vocabulary has run out has
/// nothing left to offer. The `@` and `"` triggers belong to the file list and
/// to quoting, so a word starting with either is left to them.
fn argument_token(head: &str) -> Option<Token> {
    let (name, rest) = head.split_once(char::is_whitespace)?;
    let command = COMMANDS.iter().find(|command| command.name == name)?;
    let Arguments::Words(words) = command.arguments else {
        return None;
    };
    // `rest` starts at the whitespace that ended the name, so the word starts
    // after it - and a second word means the list has had its say.
    let word = rest.trim_start();
    if word.contains(char::is_whitespace) || word.starts_with(['@', '"']) {
        return None;
    }
    Some(Token {
        source: Source::Argument {
            name: command.name,
            words,
        },
        start: head.len() - word.len(),
        query: word.to_string(),
    })
}

/// The rows a command's argument vocabulary offers, narrowed by what is typed.
///
/// A word the reader has already typed in full is not a candidate: ranking it
/// would offer the line they just wrote, and Enter would pick it a second time
/// instead of sending the command.
fn argument_rows(query: &str, name: &'static str, words: &'static [Word]) -> Vec<Row> {
    if words.iter().any(|word| word.text == query) {
        return Vec::new();
    }
    let choices: Vec<&str> = words.iter().map(|word| word.text).collect();
    fuzzy::rank(query, choices)
        .into_iter()
        .filter_map(|hit| words.get(hit.index))
        .map(|word| Row {
            // The row draws the whole line it would run: a bare word says
            // nothing about which command it completes.
            label: format!("{name} {}", word.text),
            detail: word.detail.to_string(),
            insert: word.text.to_string(),
            // Picking a word completes the line, so the row runs it: the
            // argument was the last thing missing.
            submit: true,
            session: None,
        })
        .collect()
}

/// The command rows one query ranks: the build's own commands, then the
/// skills this workspace registered.
///
/// A skill's row is the desktop palette's: the name as typed (a leading slash),
/// its one-line description, and the user-only marker when its manifest carries
/// it. Accepting one inserts `/name ` and never runs the line, because a skill
/// gesture is the reader's words plus the skill's instructions - the send
/// resolves it, exactly as it does in a window.
fn command_rows(query: &str, skills: &[SkillView]) -> Vec<Row> {
    let choices: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
    let mut rows: Vec<Row> = fuzzy::rank(query, choices)
        .into_iter()
        .take(MAX_ROWS)
        .filter_map(|hit| {
            let command = COMMANDS.get(hit.index)?;
            Some(Row {
                label: command.name.to_string(),
                detail: command.detail.to_string(),
                session: None,
                // A bare command runs; one that takes an argument leaves a
                // space after its name so the reader types the argument.
                insert: if command.bare {
                    command.name.to_string()
                } else {
                    format!("{} ", command.name)
                },
                submit: command.bare,
            })
        })
        .collect();
    // Skills come after the commands, the order the desktop's card keeps:
    // Add, Commands, then Skills. The rank is over the `name` a reader types,
    // so a query matches the gesture rather than the slash that precedes it.
    let labels: Vec<String> = skills.iter().map(|skill| slash_name(&skill.name)).collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    rows.extend(
        fuzzy::rank(query, labels)
            .into_iter()
            .filter_map(|hit| skills.get(hit.index))
            .map(|skill| Row {
                session: None,
                label: slash_name(&skill.name),
                detail: if skill.user_only {
                    format!("{} (user-only)", skill.description)
                } else {
                    skill.description.clone()
                },
                insert: format!("{}{} ", SKILL_PREFIX, skill.name),
                submit: false,
            }),
    );
    rows.truncate(MAX_ROWS);
    rows
}

/// The slash that turns a skill name into the gesture a reader types.
const SKILL_PREFIX: char = '/';

/// One skill's drawn label.
fn slash_name(name: &str) -> String {
    format!("{SKILL_PREFIX}{name}")
}

/// The `@` rows one query ranks: the workspace's files, then the sessions a
/// reader can open.
///
/// The file block walks the workspace at most once per [`STALE_AFTER`] window.
/// The session block is the desktop card's own filter, applied to what the
/// client holds: a session is found by a partial title exactly as a path is
/// found by a partial path. Files come first so a blank `@` browses the
/// workspace, and a title only wins when its score is higher.
fn file_rows(
    query: &str,
    root: Option<&Path>,
    slot: &mut Option<Files>,
    sessions: &[SessionChoice],
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    if let Some(root) = root.filter(|root| root.is_dir()) {
        let entries = entries(slot, root);
        let choices: Vec<&str> = entries.iter().map(|entry| entry.rel.as_str()).collect();
        rows = fuzzy::rank(query, choices)
            .into_iter()
            .take(MAX_ROWS)
            .filter_map(|hit| {
                let entry = entries.get(hit.index)?;
                Some(Row {
                    label: entry.rel.clone(),
                    detail: if entry.dir {
                        "folder".to_string()
                    } else {
                        entry.parent().to_string()
                    },
                    // The mention grammar is the SEND's, not this list's: the
                    // text inserted here is the same `@path` the desktop
                    // inserts.
                    insert: format_mention(&entry.rel, entry.dir, None, false)?,
                    submit: false,
                    session: None,
                })
            })
            .collect();
    }
    // The session block ranks titles the way the desktop card does, and a
    // quoted query is a path the reader is quoting, so it never lists chats.
    if query.starts_with('"') {
        return rows;
    }
    let titles: Vec<&str> = sessions
        .iter()
        .map(|session| session.title.as_str())
        .collect();
    rows.extend(
        fuzzy::rank(query, titles)
            .into_iter()
            .filter_map(|hit| sessions.get(hit.index))
            .map(|session| Row {
                label: session.title.clone(),
                detail: session.workspace.clone(),
                // Nothing is inserted: accepting opens the conversation.
                insert: String::new(),
                submit: false,
                session: Some(session.id.clone()),
            }),
    );
    rows.truncate(MAX_ROWS);
    rows
}

/// The workspace entries, built or refreshed when `slot` is missing, stale or
/// for another root.
fn entries<'a>(slot: &'a mut Option<Files>, root: &Path) -> &'a [Entry] {
    let stale = match slot {
        Some(files) => files.root != root || files.built.elapsed() >= STALE_AFTER,
        None => true,
    };
    if stale {
        *slot = Some(match workspace_index::scan_limited(root, INDEX_CAP) {
            Ok(scan) => Files {
                root: root.to_path_buf(),
                entries: scan.entries,
                truncated: scan.truncated,
                built: Instant::now(),
            },
            // A failed walk leaves no candidates, and the popup then stays
            // closed - the same silence the desktop card has when its index is
            // unavailable. The reason is logged so a bug has somewhere to show.
            Err(error) => {
                log::warn!(
                    "workspace index unavailable for {}: {error}",
                    root.display()
                );
                Files {
                    root: root.to_path_buf(),
                    entries: Vec::new(),
                    truncated: false,
                    built: Instant::now(),
                }
            }
        });
    }
    slot.as_ref()
        .map(|files| files.entries.as_slice())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace with one file at each depth, so a query can be scoped.
    fn temp_workspace() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(temp.path().join("src")).expect("mkdir src");
        std::fs::write(temp.path().join("src/app.rs"), "fn main() {}\n").expect("write app");
        std::fs::write(temp.path().join("README.md"), "read me\n").expect("write readme");
        std::fs::write(temp.path().join("src/with space.rs"), "spaces\n").expect("write space");
        temp
    }

    #[test]
    fn a_slash_opens_only_as_the_first_character() {
        let mut menu = Menu::default();
        menu.sync("/co", 3, None, &[], &[]);
        assert!(menu.open(), "the first token opens the command list");
        assert_eq!(menu.rows()[menu.active()].label, "/compact");
        // A mid-draft slash is prose, not a command.
        menu.sync("hello /co", 9, None, &[], &[]);
        assert!(!menu.open());
        // Whitespace ends the token: the command is being typed its argument.
        menu.sync("/goal ship it", 12, None, &[], &[]);
        assert!(!menu.open());
    }

    #[test]
    fn an_at_opens_after_a_word_start_and_space_closes_it() {
        let temp = temp_workspace();
        let root = temp.path();
        let mut menu = Menu::default();
        menu.sync("review @src", 10, Some(root), &[], &[]);
        assert!(menu.open(), "an @ after whitespace opens the file list");
        // The desktop card ranks by score first, so the exact path "src" beats
        // the longer "src/app.rs"; a folder only wins a tie. The list therefore
        // offers the folder it is named after and the file beneath it.
        assert_eq!(menu.rows()[menu.active()].label, "src");
        assert!(
            menu.rows().iter().any(|row| row.label == "src/app.rs"),
            "the subtree is searched, not only the folder itself"
        );
        // An @ inside another word is an email address, not a mention.
        menu.sync("mail me@src", 11, Some(root), &[], &[]);
        assert!(!menu.open());
        // A space ends the mention.
        menu.sync("@src/app.rs please", 18, Some(root), &[], &[]);
        assert!(!menu.open());
    }

    /// A skill the Host advertised is a `/` row of its own: accepting it leaves
    /// the gesture in the composer and never sends the line.
    #[test]
    fn a_skill_row_inserts_its_gesture_after_the_commands() {
        let skills = vec![
            SkillView {
                name: "release".to_string(),
                description: "ship the release".to_string(),
                user_only: false,
            },
            // A name no command owns, so the row this asserts on is the skill's
            // own: `/audit` is a real command, and a command row would sit above
            // it in the list.
            SkillView {
                name: "log".to_string(),
                description: "read the log".to_string(),
                user_only: true,
            },
        ];
        let mut menu = Menu::default();
        menu.sync("/rel", 4, None, &skills, &[]);
        assert!(menu.open(), "a skill name opens the command list");
        assert_eq!(menu.rows()[menu.active()].label, "/release");
        assert_eq!(menu.rows()[menu.active()].detail, "ship the release");
        let mut input = "/rel".to_string();
        let mut cursor = input.len();
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(false))
        );
        assert_eq!(input, "/release ");
        assert_eq!(cursor, input.len());
        // A user-only manifest keeps its row and says so.
        menu.sync("/log", 4, None, &skills, &[]);
        assert_eq!(menu.rows()[menu.active()].label, "/log");
        assert_eq!(
            menu.rows()[menu.active()].detail,
            "read the log (user-only)"
        );
    }

    /// A command's fixed vocabulary is a fourth list: the words it can take,
    /// drawn with the line they complete, and one pick runs the line.
    #[test]
    fn an_argument_list_completes_the_word_and_runs_the_line() {
        let mut menu = Menu::default();
        menu.sync("/permission ", 12, None, &[], &[]);
        assert!(
            menu.open(),
            "the space after a worded command opens its list"
        );
        let labels: Vec<&str> = menu.rows().iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "/permission read-only",
                "/permission workspace-write",
                "/permission danger-full-access",
            ],
            "every mode the contract names, in its own order"
        );
        // The gloss is the reason the row is worth reading: the mode's name
        // alone does not say what it lets a turn do.
        assert_eq!(menu.rows()[0].detail, "reads, and asks before it writes");
        let (mut input, mut cursor) = ("/permission ".to_string(), 12);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(true)),
            "the word was the last thing missing, so the line runs"
        );
        assert_eq!(input, "/permission read-only");
        assert_eq!(cursor, input.len());

        // A prefix narrows the list, the way the command list narrows itself.
        let mut menu = Menu::default();
        menu.sync("/goal p", 7, None, &[], &[]);
        assert_eq!(menu.rows()[menu.active()].label, "/goal pause");

        // A word typed in full closes the list: there is nothing left to pick,
        // and the next Enter belongs to the composer rather than to a row that
        // would insert what is already there.
        let mut menu = Menu::default();
        menu.sync("/plan off", 9, None, &[], &[]);
        assert!(!menu.open());
        // A second word is another decision (`/goal edit <objective>`), and a
        // command with free text has no list at all.
        let mut menu = Menu::default();
        menu.sync("/goal edit ship it", 18, None, &[], &[]);
        assert!(!menu.open());
        menu.sync("/rename release notes", 19, None, &[], &[]);
        assert!(!menu.open());
        // An `@` in the argument still opens the file list: quoting a path is
        // the argument's own job, not the vocabulary's.
        let temp = temp_workspace();
        let mut menu = Menu::default();
        menu.sync("/plan @ap", 9, Some(temp.path()), &[], &[]);
        assert_eq!(menu.rows()[menu.active()].label, "src/app.rs");
    }

    #[test]
    fn a_command_row_runs_and_an_argument_row_leaves_a_space() {
        // The three picker commands run bare: the list they open is the
        // argument, so Enter on the row is one keystroke instead of a space
        // and a name retyped from memory.
        let mut menu = Menu::default();
        menu.sync("/mo", 3, None, &[], &[]);
        let (mut input, mut cursor) = ("/mo".to_string(), 3);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(true))
        );
        assert_eq!(input, "/model", "a picker command runs the line as typed");

        let mut menu = Menu::default();
        menu.sync("/go", 3, None, &[], &[]);
        let (mut input, mut cursor) = ("/go".to_string(), 3);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(false))
        );
        assert_eq!(input, "/goal ");
        assert_eq!(cursor, 6, "the cursor sits after the inserted space");

        let mut menu = Menu::default();
        menu.sync("/plan", 5, None, &[], &[]);
        let (mut input, mut cursor) = ("/plan".to_string(), 5);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(true))
        );
        assert_eq!(input, "/plan", "a bare command runs the line as typed");
    }

    #[test]
    fn accepting_a_file_inserts_the_mention_and_keeps_the_line() {
        let temp = temp_workspace();
        let root = temp.path();
        let mut menu = Menu::default();
        menu.sync("review @app", 11, Some(root), &[], &[]);
        let first = menu.rows()[menu.active()].label.clone();
        assert_eq!(first, "src/app.rs", "a short needle finds a nested file");
        let (mut input, mut cursor) = ("review @app".to_string(), 11);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(false))
        );
        assert_eq!(input, "review @src/app.rs");
        assert_eq!(cursor, input.len());
        assert!(!menu.open(), "accepting closes the list");

        // A name with spaces is quoted, the way the mention grammar writes it.
        let mut menu = Menu::default();
        menu.sync("@with", 5, Some(root), &[], &[]);
        assert_eq!(menu.rows()[menu.active()].label, "src/with space.rs");
        let (mut input, mut cursor) = ("@with".to_string(), 5);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(false))
        );
        assert_eq!(input, "@\"src/with space.rs\"");
    }

    #[test]
    fn a_directory_row_mentions_the_folder_and_rows_stay_bounded() {
        let temp = temp_workspace();
        let root = temp.path();
        let mut menu = Menu::default();
        menu.sync("@src", 4, Some(root), &[], &[]);
        let labels: Vec<&str> = menu.rows().iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, vec!["src", "src/app.rs", "src/with space.rs"]);
        let (mut input, mut cursor) = ("@src".to_string(), 4);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(false))
        );
        assert_eq!(
            input, "@src/",
            "a folder's mention keeps the drill trailing"
        );

        // Walking once per window: a second query reads the same index.
        let built = menu.files.as_ref().expect("index").built;
        menu.sync("@app", 4, Some(root), &[], &[]);
        assert_eq!(menu.files.as_ref().expect("index").built, built);
    }

    /// The `@` list's second block is the desktop reference card's sessions:
    /// a title is ranked beside the files, its row carries the workspace, and
    /// accepting it opens the chat instead of inserting a mention.
    #[test]
    fn a_session_row_opens_the_chat_and_takes_the_header_out_of_the_draft() {
        let temp = temp_workspace();
        let root = temp.path();
        let sessions = vec![SessionChoice {
            id: "6f9f2f18-2f34-4f52-9a0a-4b4c1a6d5e77".to_string(),
            title: "release checklist".to_string(),
            workspace: "bos".to_string(),
        }];
        let mut menu = Menu::default();
        // No file in the fixture carries this needle, so the only row is the
        // chat's.
        menu.sync("@check", 6, Some(root), &[], &sessions);
        assert!(menu.open(), "a title opens the reference list");
        assert_eq!(menu.rows()[menu.active()].label, "release checklist");
        assert_eq!(
            menu.rows()[menu.active()].detail,
            "bos",
            "the row names the workspace, as the window's card does"
        );
        // The desktop's `consume_reference_token` cuts the whole tail from the
        // `@` on: the picker's text never survives the switch.
        let (mut input, mut cursor) = ("open @checklist please".to_string(), 15);
        menu.sync(&input, cursor, Some(root), &[], &sessions);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Session(sessions[0].id.clone()))
        );
        assert_eq!(input, "open", "the prose before the token stays");
        assert_eq!(cursor, 4, "the cursor sits where the token began");
        assert!(!menu.open(), "accepting closes the list");

        // A blank `@` browses the workspace first and the chats after it, the
        // card's own order.
        let mut menu = Menu::default();
        menu.sync("@", 1, Some(root), &[], &sessions);
        assert_eq!(menu.rows().len(), 5, "three files, one folder, one chat");
        assert_eq!(
            menu.rows().last().map(|row| row.label.as_str()),
            Some("release checklist")
        );

        // A quoted query is a path the reader is quoting, so it never lists a
        // chat: the desktop's card makes the same cut.
        let mut menu = Menu::default();
        menu.sync("@\"check", 7, Some(root), &[], &sessions);
        assert!(
            menu.rows().iter().all(|row| row.session.is_none()),
            "no session row under a quoted file query"
        );
    }

    #[test]
    fn the_highlight_clamps_at_both_ends() {
        let mut menu = Menu::default();
        menu.sync("/", 1, None, &[], &[]);
        assert_eq!(menu.rows().len(), COMMANDS.len());
        menu.move_by(-1);
        assert_eq!(menu.active(), 0);
        menu.move_by(1000);
        assert_eq!(menu.active(), COMMANDS.len() - 1);
        menu.move_by(-1000);
        assert_eq!(menu.active(), 0);
    }

    #[test]
    fn a_closed_list_accepts_nothing() {
        let mut menu = Menu::default();
        let (mut input, mut cursor) = ("/co".to_string(), 3);
        assert_eq!(menu.accept(&mut input, &mut cursor), None);
        assert_eq!(input, "/co", "a closed list never edits the line");
    }

    #[test]
    fn a_click_names_the_row_the_window_drew() {
        // The popup has no header, so the row a pointer names is the slot it
        // stands in under the popup's top row - and the window is what put a
        // candidate in that slot. A frame and a click must read the same
        // arithmetic, or a pointer would commit a row the reader never saw.
        let mut menu = Menu::default();
        menu.sync("/", 1, None, &[], &[]);
        let rows = menu.rows().len();
        assert!(rows > VISIBLE_ROWS, "the catalogue outgrows one window");

        // At the top of the list the window starts at the first row, and the
        // rows it draws are the catalogue's own in order.
        let (start, visible) = menu.window();
        assert_eq!((start, visible), (0, VISIBLE_ROWS));
        let drawn: Vec<&str> = menu.rows()[start..start + visible]
            .iter()
            .map(|row| row.label.as_str())
            .collect();
        assert_eq!(drawn, first_commands(VISIBLE_ROWS));

        // Walking to the end scrolls the window, so the last drawn row is the
        // highlighted one rather than the list's last candidate.
        menu.move_by(isize::try_from(rows).expect("the catalogue fits an isize"));
        let (start, visible) = menu.window();
        assert_eq!(start + visible, rows, "the window ends on the last row");
        assert_eq!(start + visible - 1, menu.active());

        // Selecting the row a slot names is what a click does, and the accept
        // that follows reads the highlight rather than the pointer. The label
        // is taken before the accept, which closes the list.
        menu.select(start);
        assert_eq!(menu.active(), start);
        let named = menu.rows()[start].label.clone();
        let (mut input, mut cursor) = (String::new(), 0);
        assert_eq!(
            menu.accept(&mut input, &mut cursor),
            Some(Accepted::Insert(true)),
            "the row the pointer named is the row that runs"
        );
        assert_eq!(input, named);
    }

    /// The first `count` command names, the order a blank query ranks them in.
    fn first_commands(count: usize) -> Vec<&'static str> {
        COMMANDS
            .iter()
            .take(count)
            .map(|command| command.name)
            .collect()
    }
}
